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

use crate::settings::{AppProfile, AppReviewPolicy, TitleMatchType, TitleRule};

/// Window label used for the rule-suggestion webview.
const SUGGESTION_WINDOW_LABEL: &str = "rule_suggestion";

/// Per-window latch — once a decision has been processed (accept / never /
/// dismiss / close), further `respond_rule_suggestion` calls become no-ops.
/// Reset every time a fresh window is created.
static DECISION_APPLIED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Returns `true` if this caller is the first to claim the decision slot.
/// Used by `respond_rule_suggestion` to make button-click and window-close
/// idempotent.
pub fn mark_decision_applied() -> bool {
    !DECISION_APPLIED.swap(true, std::sync::atomic::Ordering::SeqCst)
}

fn show_suggestion_dialog(app: &tauri::AppHandle, payload: SuggestionPayload, prompt_name: String) {
    use tauri::Manager;
    use tauri::WebviewUrl;
    use tauri::WebviewWindowBuilder;

    // Reset the per-window latch.
    DECISION_APPLIED.store(false, std::sync::atomic::Ordering::SeqCst);

    // If a prior window is still around (shouldn't happen but defensive), close it first.
    if let Some(existing) = app.get_webview_window(SUGGESTION_WINDOW_LABEL) {
        let _ = existing.close();
    }

    let app_name = payload.app_name.clone();
    let title = payload.title.clone();
    let prompt_id_for_close = payload.prompt_id.clone();
    let threshold = payload.threshold;

    // Minimal percent-encoding so colons/slashes/spaces survive URL query transport.
    fn enc(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for ch in s.chars() {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~') {
                out.push(ch);
            } else {
                let mut buf = [0u8; 4];
                let bytes = ch.encode_utf8(&mut buf).as_bytes();
                for b in bytes.iter() {
                    out.push_str(&format!("%{:02X}", b));
                }
            }
        }
        out
    }

    let url_path = format!(
        "src/rule_suggestion/index.html?app={}&title={}&prompt={}&pid={}&count={}&threshold={}",
        enc(&payload.app_name),
        enc(&payload.title),
        enc(&prompt_name),
        enc(&payload.prompt_id),
        payload.count,
        payload.threshold,
    );

    let result = WebviewWindowBuilder::new(
        app,
        SUGGESTION_WINDOW_LABEL,
        WebviewUrl::App(url_path.into()),
    )
    .title("Votype 规则建议")
    .inner_size(440.0, 200.0)
    .min_inner_size(380.0, 160.0)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .closable(true)
    .decorations(true)
    .always_on_top(true)
    .skip_taskbar(true)
    // KEY: do NOT focus the window. Keyboard input keeps going to whatever
    // the user's keystrokes were targeting before the dialog appeared.
    .focused(false)
    .accept_first_mouse(true)
    .build();

    let window = match result {
        Ok(w) => w,
        Err(e) => {
            log::warn!("[SuggestionEngine] failed to build dialog window: {}", e);
            return;
        }
    };

    // Handler: if the window is destroyed before any button click,
    // record the suggestion as Dismissed (best-effort).
    let app_for_destroyed = app.clone();
    let _ = window.on_window_event(move |event| {
        if let tauri::WindowEvent::Destroyed = event {
            if !mark_decision_applied() {
                return; // decision already applied via respond_rule_suggestion
            }
            log::info!("[SuggestionEngine] window closed without decision — recording dismissed");
            if let Some(hm) = app_for_destroyed
                .try_state::<std::sync::Arc<crate::managers::history::HistoryManager>>()
            {
                if let Ok(conn) = rusqlite::Connection::open(&hm.db_path) {
                    let _ = conn.busy_timeout(std::time::Duration::from_millis(5000));
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    if let Err(e) = record_decision(
                        &conn,
                        &app_name,
                        &title,
                        threshold,
                        SuggestionDecision::Dismissed,
                        now,
                    ) {
                        log::warn!("[SuggestionEngine] record_decision on close failed: {}", e);
                    }
                }
            }
            let _ = &prompt_id_for_close; // silence unused warning
        }
    });
}

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

/// Write (or update) the user's decision for a given (app_name, title) pair.
///
/// Uses an UPSERT so repeated calls overwrite the previous decision rather
/// than inserting duplicate rows.
pub fn record_decision(
    conn: &Connection,
    app_name: &str,
    title: &str,
    threshold: i64,
    decision: SuggestionDecision,
    decision_at: i64,
) -> rusqlite::Result<()> {
    let decision_str = match decision {
        SuggestionDecision::Accepted => "accepted",
        SuggestionDecision::Dismissed => "dismissed",
        SuggestionDecision::NeverAgain => "never_again",
    };
    conn.execute(
        "INSERT INTO app_rule_suggestions (app_name, title, last_threshold, decision, decision_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(app_name, title) DO UPDATE SET
            last_threshold = excluded.last_threshold,
            decision = excluded.decision,
            decision_at = excluded.decision_at",
        rusqlite::params![app_name, title, threshold, decision_str, decision_at],
    )?;
    Ok(())
}

/// Test whether any TitleRule in any AppProfile would match the given title
/// for `app_name`. All match_types are evaluated: Exact = literal equality,
/// Text = case-sensitive substring, Regex = regex compile + match. Invalid
/// regex patterns are treated as non-matching.
pub fn any_rule_matches(profiles: &[AppProfile], app_name: &str, title: &str) -> bool {
    for profile in profiles {
        if profile.name != app_name {
            continue;
        }
        for rule in &profile.rules {
            let hit = match rule.match_type {
                TitleMatchType::Exact => rule.pattern == title,
                TitleMatchType::Text => title.contains(&rule.pattern),
                TitleMatchType::Regex => regex::Regex::new(&rule.pattern)
                    .map(|re| re.is_match(title))
                    .unwrap_or(false),
            };
            if hit {
                return true;
            }
        }
    }
    false
}

/// Find the AppProfile matching `app_name`, or create a new one. Append a
/// TitleRule with `match_type = Exact` for the given title/prompt_id, and
/// ensure `app_to_profile` maps `app_name` to the resolved profile id so
/// routing can find it. Caller persists the mutated settings.
pub fn apply_accepted_suggestion(
    settings: &mut crate::settings::AppSettings,
    app_name: &str,
    title: &str,
    prompt_id: &str,
) {
    let new_rule = TitleRule {
        id: uuid::Uuid::new_v4().to_string(),
        pattern: title.to_string(),
        match_type: TitleMatchType::Exact,
        policy: AppReviewPolicy::Auto,
        prompt_id: Some(prompt_id.to_string()),
    };

    // Prefer an existing profile already linked via app_to_profile (handles
    // renamed profiles), else match by name, else create.
    let linked_id = settings.app_to_profile.get(app_name).cloned();
    let target_id = if let Some(id) = linked_id.as_ref() {
        if settings.app_profiles.iter().any(|p| &p.id == id) {
            Some(id.clone())
        } else {
            None
        }
    } else {
        None
    };

    let resolved_id = if let Some(id) = target_id {
        if let Some(existing) = settings.app_profiles.iter_mut().find(|p| p.id == id) {
            existing.rules.push(new_rule);
        }
        id
    } else if let Some(existing) = settings
        .app_profiles
        .iter_mut()
        .find(|p| p.name == app_name)
    {
        existing.rules.push(new_rule);
        existing.id.clone()
    } else {
        let new_profile = AppProfile {
            id: uuid::Uuid::new_v4().to_string(),
            name: app_name.to_string(),
            policy: AppReviewPolicy::Auto,
            prompt_id: None,
            icon: None,
            translate_to_english_on_insert: false,
            disable_selection_clipboard_fallback: false,
            rules: vec![new_rule],
        };
        let id = new_profile.id.clone();
        settings.app_profiles.push(new_profile);
        id
    };

    // Always (re)bind app_name -> profile id so routing finds it.
    settings
        .app_to_profile
        .insert(app_name.to_string(), resolved_id);
}

/// Per-stop dispatch latch — set when an emission has already happened for the
/// current `TranscribeAction::stop()` run. Prevents stacking multiple overlay
/// popups when a single paste crosses several thresholds.
///
/// `reset_emission_latch()` is called from `TranscribeAction::start()`.
static EMISSION_LATCH_THIS_STOP: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn reset_emission_latch() {
    EMISSION_LATCH_THIS_STOP.store(false, std::sync::atomic::Ordering::Relaxed);
}

/// Called after a paste finishes successfully. Queries the just-inserted
/// history row, decides whether to surface a suggestion, and if so opens
/// a native confirmation dialog. The user's choice is persisted directly
/// from the dialog callback (no frontend round-trip).
pub fn check_after_paste(app: &tauri::AppHandle, history_id: i64) {
    use tauri::Manager;

    if EMISSION_LATCH_THIS_STOP.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    let settings = crate::settings::get_settings(app);
    let hm = match app.try_state::<std::sync::Arc<crate::managers::history::HistoryManager>>() {
        Some(h) => h,
        None => return,
    };
    let conn = match rusqlite::Connection::open(&hm.db_path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[SuggestionEngine] open DB failed: {}", e);
            return;
        }
    };
    let _ = conn.busy_timeout(std::time::Duration::from_millis(5000));

    let profiles_snapshot = settings.app_profiles.clone();
    let matcher = move |a: &str, t: &str| any_rule_matches(&profiles_snapshot, a, t);

    let payload = match compute_suggestion(&conn, history_id, &matcher) {
        Ok(Some(p)) => p,
        Ok(None) => return,
        Err(e) => {
            log::warn!("[SuggestionEngine] compute_suggestion failed: {}", e);
            return;
        }
    };

    let prompt_name = resolve_prompt_name(&settings, &payload.prompt_id);

    // Drop the connection before invoking the dialog so it doesn't get held
    // across the async callback.
    drop(conn);

    show_suggestion_dialog(app, payload, prompt_name);
}

fn resolve_prompt_name(settings: &crate::settings::AppSettings, prompt_id: &str) -> String {
    match prompt_id {
        "__PASS_THROUGH__" => "无需润色".to_string(),
        "__LITE_POLISH__" => "轻量润色".to_string(),
        id => settings
            .post_process_prompts
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| id.to_string()),
    }
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

    #[test]
    fn record_decision_inserts_new_row() {
        let conn = fresh_db();
        record_decision(
            &conn,
            "Slack",
            "title",
            5,
            SuggestionDecision::Dismissed,
            100,
        )
        .unwrap();
        let (t, d, at): (i64, String, i64) = conn
            .query_row(
                "SELECT last_threshold, decision, decision_at FROM app_rule_suggestions WHERE app_name=? AND title=?",
                rusqlite::params!["Slack", "title"],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(t, 5);
        assert_eq!(d, "dismissed");
        assert_eq!(at, 100);
    }

    #[test]
    fn record_decision_upserts_existing_row() {
        let conn = fresh_db();
        record_decision(
            &conn,
            "Slack",
            "title",
            5,
            SuggestionDecision::Dismissed,
            100,
        )
        .unwrap();
        record_decision(
            &conn,
            "Slack",
            "title",
            10,
            SuggestionDecision::NeverAgain,
            200,
        )
        .unwrap();
        let (t, d, at): (i64, String, i64) = conn
            .query_row(
                "SELECT last_threshold, decision, decision_at FROM app_rule_suggestions WHERE app_name=? AND title=?",
                rusqlite::params!["Slack", "title"],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(t, 10);
        assert_eq!(d, "never_again");
        assert_eq!(at, 200);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM app_rule_suggestions", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 1, "row should be updated in place, not duplicated");
    }

    fn test_settings() -> crate::settings::AppSettings {
        crate::settings::get_default_settings()
    }

    #[test]
    fn apply_creates_new_profile_when_app_absent() {
        let mut settings = test_settings();
        apply_accepted_suggestion(&mut settings, "Slack", "Slack | #a", "polish");
        assert_eq!(settings.app_profiles.len(), 1);
        assert_eq!(settings.app_profiles[0].name, "Slack");
        assert_eq!(settings.app_profiles[0].rules.len(), 1);
        assert_eq!(settings.app_profiles[0].rules[0].pattern, "Slack | #a");
        assert_eq!(
            settings.app_profiles[0].rules[0].match_type,
            crate::settings::TitleMatchType::Exact
        );
        assert_eq!(
            settings.app_profiles[0].rules[0].prompt_id.as_deref(),
            Some("polish")
        );
        assert_eq!(
            settings.app_to_profile.get("Slack"),
            Some(&settings.app_profiles[0].id.clone()),
        );
    }

    #[test]
    fn apply_appends_to_existing_profile() {
        use crate::settings::{AppProfile, AppReviewPolicy};
        let mut settings = test_settings();
        settings.app_profiles.push(AppProfile {
            id: "existing".to_string(),
            name: "Slack".to_string(),
            policy: AppReviewPolicy::Auto,
            prompt_id: None,
            icon: None,
            translate_to_english_on_insert: false,
            disable_selection_clipboard_fallback: false,
            rules: vec![],
        });
        apply_accepted_suggestion(&mut settings, "Slack", "Slack | #a", "polish");
        assert_eq!(
            settings.app_profiles.len(),
            1,
            "should not create a duplicate profile"
        );
        assert_eq!(settings.app_profiles[0].rules.len(), 1);
        assert_eq!(
            settings.app_to_profile.get("Slack"),
            Some(&"existing".to_string()),
        );
    }

    #[test]
    fn apply_respects_linked_renamed_profile() {
        use crate::settings::{AppProfile, AppReviewPolicy};
        let mut settings = test_settings();
        settings.app_profiles.push(AppProfile {
            id: "renamed-id".to_string(),
            name: "Slack (Work)".to_string(), // user renamed
            policy: AppReviewPolicy::Auto,
            prompt_id: None,
            icon: None,
            translate_to_english_on_insert: false,
            disable_selection_clipboard_fallback: false,
            rules: vec![],
        });
        settings
            .app_to_profile
            .insert("Slack".to_string(), "renamed-id".to_string());

        apply_accepted_suggestion(&mut settings, "Slack", "Slack | #a", "polish");

        assert_eq!(settings.app_profiles.len(), 1, "no duplicate profile");
        assert_eq!(settings.app_profiles[0].rules.len(), 1);
        assert_eq!(
            settings.app_to_profile.get("Slack"),
            Some(&"renamed-id".to_string()),
        );
    }

    fn make_profile(name: &str, rules: Vec<TitleRule>) -> AppProfile {
        AppProfile {
            id: "p1".into(),
            name: name.into(),
            policy: AppReviewPolicy::Auto,
            prompt_id: None,
            icon: None,
            translate_to_english_on_insert: false,
            disable_selection_clipboard_fallback: false,
            rules,
        }
    }

    fn make_rule(pattern: &str, mt: TitleMatchType) -> TitleRule {
        TitleRule {
            id: "r1".into(),
            pattern: pattern.into(),
            match_type: mt,
            policy: AppReviewPolicy::Auto,
            prompt_id: None,
        }
    }

    #[test]
    fn any_rule_matches_exact() {
        let profiles = vec![make_profile(
            "Slack",
            vec![make_rule("Slack | #a", TitleMatchType::Exact)],
        )];
        assert!(any_rule_matches(&profiles, "Slack", "Slack | #a"));
        assert!(!any_rule_matches(&profiles, "Slack", "Slack | #b"));
        assert!(!any_rule_matches(&profiles, "Chrome", "Slack | #a"));
    }

    #[test]
    fn any_rule_matches_text_substring() {
        let profiles = vec![make_profile(
            "Slack",
            vec![make_rule("Slack", TitleMatchType::Text)],
        )];
        assert!(any_rule_matches(&profiles, "Slack", "Slack | #a"));
        assert!(any_rule_matches(&profiles, "Slack", "Slack | #b"));
    }

    #[test]
    fn any_rule_matches_regex() {
        let profiles = vec![make_profile(
            "Slack",
            vec![make_rule(r"^Slack \| #.+", TitleMatchType::Regex)],
        )];
        assert!(any_rule_matches(&profiles, "Slack", "Slack | #anything"));
        assert!(!any_rule_matches(&profiles, "Slack", "Slack"));
    }

    #[test]
    fn any_rule_matches_invalid_regex_returns_false() {
        let profiles = vec![make_profile(
            "Slack",
            vec![make_rule(r"[invalid(regex", TitleMatchType::Regex)],
        )];
        assert!(!any_rule_matches(&profiles, "Slack", "Slack | #a"));
    }
}
