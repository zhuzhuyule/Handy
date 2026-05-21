//! App rule suggestion engine.
//!
//! Spec: docs/specs/2026-05-21-app-rule-suggestion-engine.spec.md
//!
//! After each successful paste, the engine inspects the just-inserted
//! `transcription_history` row, counts how many times its
//! `(app_name, window_title, post_process_prompt_id)` triplet has been used,
//! and (when the count crosses 5/10/20/40 and no AppProfile TitleRule
//! already matches the title) emits a `rule-suggestion-show` event so the
//! frontend overlay can ask the user whether to record a rule.

use serde::{Deserialize, Serialize};
use specta::Type;

/// SQLite migration: tracks suggestion decisions per (app, title) so we
/// don't re-prompt at thresholds the user has already declined.
pub const MIGRATION_SQL: &str = "
CREATE TABLE IF NOT EXISTS app_rule_suggestions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    app_name TEXT NOT NULL,
    title TEXT NOT NULL,
    last_threshold INTEGER NOT NULL,
    decision TEXT NOT NULL,
    decision_at INTEGER NOT NULL,
    UNIQUE(app_name, title)
);
CREATE INDEX IF NOT EXISTS idx_ars_app_title
    ON app_rule_suggestions(app_name, title);
";

/// Threshold ladder (count >= threshold triggers a suggestion at that level).
pub const THRESHOLDS: &[i64] = &[5, 10, 20, 40];

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SuggestionPayload {
    pub app_name: String,
    pub title: String,
    pub prompt_id: String,
    pub prompt_name: String,
    pub count: i64,
    pub threshold: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionDecision {
    Accepted,
    Dismissed,
    NeverAgain,
}

use rusqlite::Connection;

/// Compute whether the just-inserted history row should trigger a suggestion.
///
/// `has_matching_rule` is a closure: `(app_name, title) -> bool`, returning
/// true if any existing TitleRule in any AppProfile matches the title under
/// its own match_type. The caller is responsible for evaluating Text /
/// Regex / Exact match semantics against settings.app_profiles.
pub fn compute_suggestion<F>(
    conn: &Connection,
    history_id: i64,
    has_matching_rule: &F,
) -> rusqlite::Result<Option<SuggestionPayload>>
where
    F: Fn(&str, &str) -> bool,
{
    let mut stmt = conn.prepare(
        "SELECT app_name, window_title, post_process_prompt_id FROM transcription_history WHERE id = ?",
    )?;
    let row = stmt.query_row([history_id], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    });
    let (app_opt, title_opt, prompt_opt) = match row {
        Ok(t) => t,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e),
    };
    let (app, title, prompt_id) = match (app_opt, title_opt, prompt_opt) {
        (Some(a), Some(t), Some(p)) if !a.is_empty() && !t.is_empty() && !p.is_empty() => (a, t, p),
        _ => return Ok(None),
    };

    if has_matching_rule(&app, &title) {
        return Ok(None);
    }

    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM transcription_history
            WHERE app_name = ? AND window_title = ? AND post_process_prompt_id = ?
              AND COALESCE(deleted, 0) = 0",
        rusqlite::params![&app, &title, &prompt_id],
        |r| r.get(0),
    )?;

    // Highest threshold <= count.
    let threshold = THRESHOLDS.iter().rev().find(|&&t| count >= t).copied();
    let threshold = match threshold {
        Some(t) => t,
        None => return Ok(None),
    };

    // Check prior decision for this (app, title). Only NoRows is benign;
    // other errors (DB lock, schema mismatch) must propagate.
    let prior: Option<(i64, String)> = match conn.query_row(
        "SELECT last_threshold, decision FROM app_rule_suggestions
            WHERE app_name = ? AND title = ?",
        rusqlite::params![&app, &title],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ) {
        Ok(t) => Some(t),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(e) => return Err(e),
    };
    if let Some((prior_threshold, decision)) = prior {
        if decision == "never_again" {
            return Ok(None);
        }
        if decision == "accepted" {
            return Ok(None);
        }
        if decision == "dismissed" && prior_threshold >= threshold {
            return Ok(None);
        }
    }

    Ok(Some(SuggestionPayload {
        app_name: app,
        title,
        prompt_id: prompt_id.clone(),
        prompt_name: prompt_id, // Caller upgrades via PromptManager resolution if needed
        count,
        threshold,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE transcription_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                app_name TEXT,
                window_title TEXT,
                post_process_prompt_id TEXT,
                deleted BOOLEAN NOT NULL DEFAULT 0
            );",
        )
        .unwrap();
        conn.execute_batch(MIGRATION_SQL).unwrap();
        conn
    }

    fn insert_history(
        conn: &Connection,
        app: Option<&str>,
        title: Option<&str>,
        prompt_id: Option<&str>,
    ) -> i64 {
        conn.execute(
            "INSERT INTO transcription_history (app_name, window_title, post_process_prompt_id) VALUES (?, ?, ?)",
            rusqlite::params![app, title, prompt_id],
        ).unwrap();
        conn.last_insert_rowid()
    }

    fn has_matching_rule_never(_app: &str, _title: &str) -> bool {
        false
    }

    #[test]
    fn returns_none_when_count_below_first_threshold() {
        let conn = fresh_db();
        let id = insert_history(&conn, Some("Slack"), Some("Slack | #a"), Some("polish"));
        let result = compute_suggestion(&conn, id, &has_matching_rule_never).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn returns_payload_at_first_threshold_5() {
        let conn = fresh_db();
        for _ in 0..4 {
            insert_history(&conn, Some("Slack"), Some("Slack | #a"), Some("polish"));
        }
        let id = insert_history(&conn, Some("Slack"), Some("Slack | #a"), Some("polish"));
        let result = compute_suggestion(&conn, id, &has_matching_rule_never).unwrap();
        let payload = result.expect("should return suggestion at 5");
        assert_eq!(payload.app_name, "Slack");
        assert_eq!(payload.title, "Slack | #a");
        assert_eq!(payload.prompt_id, "polish");
        assert_eq!(payload.count, 5);
        assert_eq!(payload.threshold, 5);
    }

    #[test]
    fn picks_highest_threshold_when_count_crosses_multiple() {
        let conn = fresh_db();
        for _ in 0..10 {
            insert_history(&conn, Some("Slack"), Some("Slack | #a"), Some("polish"));
        }
        let id = insert_history(&conn, Some("Slack"), Some("Slack | #a"), Some("polish")); // count = 11
        let result = compute_suggestion(&conn, id, &has_matching_rule_never).unwrap();
        let payload = result.expect("count 11 crosses 5 and 10");
        assert_eq!(payload.threshold, 10);
        assert_eq!(payload.count, 11);
    }

    #[test]
    fn mixed_prompts_do_not_aggregate() {
        let conn = fresh_db();
        for _ in 0..4 {
            insert_history(&conn, Some("Slack"), Some("Slack | #a"), Some("polish"));
        }
        let id = insert_history(
            &conn,
            Some("Slack"),
            Some("Slack | #a"),
            Some("passthrough"),
        );
        // polish count = 4, passthrough count = 1 — neither crosses 5
        let result = compute_suggestion(&conn, id, &has_matching_rule_never).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn empty_title_returns_none() {
        let conn = fresh_db();
        let id = insert_history(&conn, Some("Slack"), Some(""), Some("polish"));
        let result = compute_suggestion(&conn, id, &has_matching_rule_never).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn null_app_returns_none() {
        let conn = fresh_db();
        let id = insert_history(&conn, None, Some("a title"), Some("polish"));
        let result = compute_suggestion(&conn, id, &has_matching_rule_never).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn null_prompt_id_returns_none() {
        let conn = fresh_db();
        let id = insert_history(&conn, Some("Slack"), Some("Slack | #a"), None);
        let result = compute_suggestion(&conn, id, &has_matching_rule_never).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn existing_rule_skips_suggestion() {
        let conn = fresh_db();
        for _ in 0..4 {
            insert_history(&conn, Some("Slack"), Some("Slack | #a"), Some("polish"));
        }
        let id = insert_history(&conn, Some("Slack"), Some("Slack | #a"), Some("polish"));
        // User already has a TitleRule matching this title.
        let already_matches = |_app: &str, _title: &str| true;
        let result = compute_suggestion(&conn, id, &already_matches).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn never_again_silences_at_all_thresholds() {
        let conn = fresh_db();
        for _ in 0..4 {
            insert_history(&conn, Some("Slack"), Some("Slack | #a"), Some("polish"));
        }
        let id = insert_history(&conn, Some("Slack"), Some("Slack | #a"), Some("polish"));
        conn.execute(
            "INSERT INTO app_rule_suggestions (app_name, title, last_threshold, decision, decision_at) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params!["Slack", "Slack | #a", 5_i64, "never_again", 0_i64],
        ).unwrap();
        let result = compute_suggestion(&conn, id, &has_matching_rule_never).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn dismissed_at_lower_threshold_allows_higher() {
        let conn = fresh_db();
        for _ in 0..9 {
            insert_history(&conn, Some("Slack"), Some("Slack | #a"), Some("polish"));
        }
        let id = insert_history(&conn, Some("Slack"), Some("Slack | #a"), Some("polish")); // count = 10
        conn.execute(
            "INSERT INTO app_rule_suggestions (app_name, title, last_threshold, decision, decision_at) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params!["Slack", "Slack | #a", 5_i64, "dismissed", 0_i64],
        ).unwrap();
        let result = compute_suggestion(&conn, id, &has_matching_rule_never).unwrap();
        let payload = result.expect("threshold 10 > dismissed level 5");
        assert_eq!(payload.threshold, 10);
    }

    #[test]
    fn dismissed_at_same_threshold_skips() {
        let conn = fresh_db();
        for _ in 0..4 {
            insert_history(&conn, Some("Slack"), Some("Slack | #a"), Some("polish"));
        }
        let id = insert_history(&conn, Some("Slack"), Some("Slack | #a"), Some("polish")); // count = 5
        conn.execute(
            "INSERT INTO app_rule_suggestions (app_name, title, last_threshold, decision, decision_at) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params!["Slack", "Slack | #a", 5_i64, "dismissed", 0_i64],
        ).unwrap();
        let result = compute_suggestion(&conn, id, &has_matching_rule_never).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn accepted_prior_decision_still_skips() {
        // Defensive coverage: an "accepted" decision row should suppress
        // a suggestion even when has_matching_rule returns false (e.g., the
        // user accepted, the rule was written, but the rule was then
        // manually deleted). The persisted decision keeps history quiet.
        let conn = fresh_db();
        for _ in 0..4 {
            insert_history(&conn, Some("Slack"), Some("Slack | #a"), Some("polish"));
        }
        let id = insert_history(&conn, Some("Slack"), Some("Slack | #a"), Some("polish"));
        conn.execute(
            "INSERT INTO app_rule_suggestions (app_name, title, last_threshold, decision, decision_at) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params!["Slack", "Slack | #a", 5_i64, "accepted", 0_i64],
        )
        .unwrap();
        let result = compute_suggestion(&conn, id, &has_matching_rule_never).unwrap();
        assert!(
            result.is_none(),
            "accepted decision should suppress re-prompt"
        );
    }
}
