# App Rule Suggestion Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Detect when a user has used the same `prompt_id` 5 / 10 / 20 / 40 times in a specific `(app, title)` context without having configured a rule, then show a floating overlay asking whether to add an Exact-match TitleRule into the corresponding AppProfile (auto-creating the profile if absent).

**Architecture:** A new `suggestion_engine` manager runs after each successful paste, queries `transcription_history` for triplet counts via SQL, checks existing AppProfile TitleRules for collision, consults a new `app_rule_suggestions` table for prior dismissals, and emits a `rule-suggestion-show` tauri event. The overlay (`RecordingOverlay.tsx`) listens, renders 3-button UI, and dispatches back via a new `respond_rule_suggestion` command that either writes a TitleRule + dismissal record or just dismissal.

**Tech Stack:** Rust + Tauri 2.x + rusqlite + rusqlite_migration; React 18 + Radix UI + tauri events; specta type generation; `bun` toolchain.

**Spec:** `docs/specs/2026-05-21-app-rule-suggestion-engine.spec.md`

---

## File Structure

**Create**

- `src-tauri/src/managers/suggestion_engine.rs` — engine module: `MIGRATION_SQL` constant, `SuggestionPayload` struct, `compute_suggestion()`, `record_decision()`, `apply_accepted_suggestion()`, plus `#[cfg(test)]` unit tests

**Modify**

- `src-tauri/src/settings.rs` — add `TitleMatchType::Exact` variant + roundtrip test
- `src-tauri/src/managers/history.rs:451` — register new migration after current #46
- `src-tauri/src/managers/mod.rs` — `pub mod suggestion_engine;`
- `src-tauri/src/shortcut/settings_cmds.rs` — new `respond_rule_suggestion` command
- `src-tauri/src/lib.rs` — register command in both `collect_commands![]` builders (lines ~493 and ~909) + bootstrap module
- `src-tauri/src/actions/transcribe.rs` — call `suggestion_engine::check_after_paste(...)` after each successful paste (existing call sites at ~2454, ~2767, ~2944)
- `src/overlay/RecordingOverlay.tsx` — listen `rule-suggestion-show` event, render 3-button UI
- `src/components/settings/post-processing/AppReviewPolicies.tsx` — add `Exact` choice to TitleMatchType SegmentedControl
- `src/bindings.ts` — regenerate via `bun tauri dev` (specta export runs at app start)
- `src/i18n/locales/en/translation.json` + `src/i18n/locales/zh/translation.json` — new overlay copy keys

**Out of scope (per spec §禁止)**

- Modifying existing `AppProfile` / `TitleRule` fields beyond adding the enum variant
- Modifying `routing.rs` / `extensions.rs` (`override_prompt_id` already supported)
- Modifying `AppProfilesManager` layout/structure
- Backfilling historical recordings on first launch

---

## Task 1: Add `TitleMatchType::Exact` variant + roundtrip test

**Files:**

- Modify: `src-tauri/src/settings.rs:521-535`

- [ ] **Step 1: Write the failing roundtrip test**

Append to the `#[cfg(test)] mod tests` block inside `settings.rs` (search for the existing `mod tests {` near the bottom — if none, create one at end of file):

```rust
#[cfg(test)]
mod title_match_type_tests {
    use super::TitleMatchType;
    use serde_json;

    #[test]
    fn roundtrip_text() {
        let v: TitleMatchType = serde_json::from_str("\"text\"").unwrap();
        assert_eq!(v, TitleMatchType::Text);
        assert_eq!(serde_json::to_string(&v).unwrap(), "\"text\"");
    }

    #[test]
    fn roundtrip_regex() {
        let v: TitleMatchType = serde_json::from_str("\"regex\"").unwrap();
        assert_eq!(v, TitleMatchType::Regex);
        assert_eq!(serde_json::to_string(&v).unwrap(), "\"regex\"");
    }

    #[test]
    fn roundtrip_exact_new_variant() {
        let v: TitleMatchType = serde_json::from_str("\"exact\"").unwrap();
        assert_eq!(v, TitleMatchType::Exact);
        assert_eq!(serde_json::to_string(&v).unwrap(), "\"exact\"");
    }
}
```

- [ ] **Step 2: Run test to verify it fails to compile**

```bash
rtk cargo test --manifest-path /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml -p votype title_match_type_tests 2>&1 | tail -10
```

Expected: compile error `no variant or associated item named 'Exact' found for enum 'TitleMatchType'`.

- [ ] **Step 3: Add the `Exact` variant**

In `src-tauri/src/settings.rs:521-528`, change the enum definition from:

```rust
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "lowercase")]
pub enum TitleMatchType {
    /// Simple text contains matching
    Text,
    /// Regular expression matching
    Regex,
}
```

to:

```rust
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "lowercase")]
pub enum TitleMatchType {
    /// Simple text contains matching (substring)
    Text,
    /// Regular expression matching
    Regex,
    /// Exact literal equality (used by auto-generated rules from the
    /// suggestion engine — see docs/specs/2026-05-21-app-rule-suggestion-engine.spec.md)
    Exact,
}
```

- [ ] **Step 4: Run test to verify all 3 pass**

```bash
rtk cargo test --manifest-path /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml -p votype title_match_type_tests 2>&1 | tail -10
```

Expected: `test result: ok. 3 passed`.

- [ ] **Step 5: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src-tauri/src/settings.rs
git -C /Users/zac/code/github/asr/Handy commit -m "Add TitleMatchType::Exact variant for literal equality"
```

---

## Task 2: Create suggestion_engine module skeleton + register migration

**Files:**

- Create: `src-tauri/src/managers/suggestion_engine.rs`
- Modify: `src-tauri/src/managers/mod.rs`
- Modify: `src-tauri/src/managers/history.rs:451-452`

- [ ] **Step 1: Create the new module file with migration SQL and types**

Write to `/Users/zac/code/github/asr/Handy/src-tauri/src/managers/suggestion_engine.rs`:

```rust
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
```

- [ ] **Step 2: Register the module in `mod.rs`**

Read `/Users/zac/code/github/asr/Handy/src-tauri/src/managers/mod.rs`. It's a short file listing `pub mod X;` lines. Add (in alphabetical position):

```rust
pub mod suggestion_engine;
```

- [ ] **Step 3: Register the migration in `history.rs`**

In `/Users/zac/code/github/asr/Handy/src-tauri/src/managers/history.rs`, find the line `M::up(crate::managers::cluster_feedback::MIGRATION_SQL),` (around line 451). Add immediately after:

```rust
    // Migration 47: app_rule_suggestions table for suggestion engine
    M::up(crate::managers::suggestion_engine::MIGRATION_SQL),
```

- [ ] **Step 4: Verify build**

```bash
rtk cargo check --manifest-path /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml -p votype 2>&1 | tail -10
```

Expected: clean. (Specta requires `Type` derive on `SuggestionPayload` and `SuggestionDecision` so they can later cross the IPC boundary.)

- [ ] **Step 5: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src-tauri/src/managers/suggestion_engine.rs src-tauri/src/managers/mod.rs src-tauri/src/managers/history.rs
git -C /Users/zac/code/github/asr/Handy commit -m "Scaffold suggestion_engine module with migration and payload types"
```

---

## Task 3: Implement `compute_suggestion()` with unit tests

**Files:**

- Modify: `src-tauri/src/managers/suggestion_engine.rs`

This is the load-bearing logic. TDD it.

- [ ] **Step 1: Add `#[cfg(test)] mod tests` block with the failing tests**

Append to `/Users/zac/code/github/asr/Handy/src-tauri/src/managers/suggestion_engine.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // Minimal transcription_history schema for the engine to query.
        conn.execute_batch(
            "CREATE TABLE transcription_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                app_name TEXT,
                window_title TEXT,
                post_process_prompt_id TEXT,
                deleted BOOLEAN NOT NULL DEFAULT 0
            );",
        ).unwrap();
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
        let id = insert_history(&conn, Some("Slack"), Some("Slack | #a"), Some("passthrough"));
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
        // Pre-seed a 'never_again' decision.
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

    fn has_matching_rule_never(_app: &str, _title: &str) -> bool {
        false
    }
}
```

- [ ] **Step 2: Run tests to confirm they fail to compile**

```bash
rtk cargo test --manifest-path /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml -p votype suggestion_engine::tests 2>&1 | tail -20
```

Expected: compile error `cannot find function 'compute_suggestion' in this scope`.

- [ ] **Step 3: Implement `compute_suggestion()` above the test module**

Add this just above the `#[cfg(test)]` block:

```rust
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

    // Check prior decision for this (app, title).
    let prior: Option<(i64, String)> = conn
        .query_row(
            "SELECT last_threshold, decision FROM app_rule_suggestions
                WHERE app_name = ? AND title = ?",
            rusqlite::params![&app, &title],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    if let Some((prior_threshold, decision)) = prior {
        if decision == "never_again" {
            return Ok(None);
        }
        if decision == "accepted" {
            // Should be caught by has_matching_rule but guard anyway.
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
```

- [ ] **Step 4: Run tests**

```bash
rtk cargo test --manifest-path /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml -p votype suggestion_engine::tests 2>&1 | tail -15
```

Expected: `test result: ok. 11 passed`.

- [ ] **Step 5: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src-tauri/src/managers/suggestion_engine.rs
git -C /Users/zac/code/github/asr/Handy commit -m "Implement compute_suggestion with 11 unit tests"
```

---

## Task 4: Implement `record_decision()` + `apply_accepted_suggestion()`

**Files:**

- Modify: `src-tauri/src/managers/suggestion_engine.rs`

- [ ] **Step 1: Append failing tests for `record_decision`**

Add to the `#[cfg(test)] mod tests` block (before the helper `has_matching_rule_never`):

```rust
    #[test]
    fn record_decision_inserts_new_row() {
        let conn = fresh_db();
        record_decision(&conn, "Slack", "title", 5, SuggestionDecision::Dismissed, 100).unwrap();
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
        record_decision(&conn, "Slack", "title", 5, SuggestionDecision::Dismissed, 100).unwrap();
        record_decision(&conn, "Slack", "title", 10, SuggestionDecision::NeverAgain, 200).unwrap();
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
            .query_row("SELECT COUNT(*) FROM app_rule_suggestions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "row should be updated in place, not duplicated");
    }
```

- [ ] **Step 2: Run tests to confirm they fail**

```bash
rtk cargo test --manifest-path /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml -p votype suggestion_engine::tests 2>&1 | tail -10
```

Expected: compile error `cannot find function 'record_decision'`.

- [ ] **Step 3: Add `record_decision()` above the test module**

```rust
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
```

- [ ] **Step 4: Run tests**

```bash
rtk cargo test --manifest-path /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml -p votype suggestion_engine::tests 2>&1 | tail -10
```

Expected: 13 passed (11 existing + 2 new).

- [ ] **Step 5: Add `apply_accepted_suggestion()`**

This one is integration-y (touches `settings::AppProfile`) so we test it via the integration command in Task 6 rather than here. Add the function above the test module:

```rust
use crate::settings::{AppProfile, AppReviewPolicy, TitleMatchType, TitleRule};

/// Find the AppProfile matching `app_name`, or create a new one. Append a
/// TitleRule with `match_type = Exact` for the given title/prompt_id.
/// Returns the modified Vec<AppProfile> (caller persists via save_settings).
pub fn apply_accepted_suggestion(
    profiles: &mut Vec<AppProfile>,
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

    if let Some(existing) = profiles.iter_mut().find(|p| p.name == app_name) {
        existing.rules.push(new_rule);
    } else {
        profiles.push(AppProfile {
            id: uuid::Uuid::new_v4().to_string(),
            name: app_name.to_string(),
            policy: AppReviewPolicy::Auto,
            prompt_id: None,
            icon: None,
            translate_to_english_on_insert: false,
            disable_selection_clipboard_fallback: false,
            rules: vec![new_rule],
        });
    }
}
```

Check `Cargo.toml` already includes `uuid = { ... features = ["v4"] }` — search the codebase for existing `uuid::Uuid::new_v4` usage:

```bash
rtk grep "uuid::Uuid::new_v4" /Users/zac/code/github/asr/Handy/src-tauri/src --include="*.rs" | head -3
```

If yes (it should be — `AppProfilesManager` uses uuid IDs), nothing to change. If no, `cargo add uuid -F v4` in `src-tauri/`.

- [ ] **Step 6: Add a test for `apply_accepted_suggestion()` (no DB needed)**

```rust
    #[test]
    fn apply_creates_new_profile_when_app_absent() {
        let mut profiles = Vec::new();
        apply_accepted_suggestion(&mut profiles, "Slack", "Slack | #a", "polish");
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "Slack");
        assert_eq!(profiles[0].rules.len(), 1);
        assert_eq!(profiles[0].rules[0].pattern, "Slack | #a");
        assert_eq!(profiles[0].rules[0].match_type, crate::settings::TitleMatchType::Exact);
        assert_eq!(profiles[0].rules[0].prompt_id.as_deref(), Some("polish"));
    }

    #[test]
    fn apply_appends_to_existing_profile() {
        use crate::settings::{AppProfile, AppReviewPolicy};
        let mut profiles = vec![AppProfile {
            id: "existing".to_string(),
            name: "Slack".to_string(),
            policy: AppReviewPolicy::Auto,
            prompt_id: None,
            icon: None,
            translate_to_english_on_insert: false,
            disable_selection_clipboard_fallback: false,
            rules: vec![],
        }];
        apply_accepted_suggestion(&mut profiles, "Slack", "Slack | #a", "polish");
        assert_eq!(profiles.len(), 1, "should not create a duplicate profile");
        assert_eq!(profiles[0].rules.len(), 1);
    }
```

- [ ] **Step 7: Run all tests**

```bash
rtk cargo test --manifest-path /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml -p votype suggestion_engine 2>&1 | tail -10
```

Expected: 15 passed.

- [ ] **Step 8: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src-tauri/src/managers/suggestion_engine.rs
git -C /Users/zac/code/github/asr/Handy commit -m "Add record_decision and apply_accepted_suggestion helpers"
```

---

## Task 5: Implement the `has_matching_rule` evaluator + integration helper

**Files:**

- Modify: `src-tauri/src/managers/suggestion_engine.rs`

The `compute_suggestion()` API takes a closure to test "does any TitleRule match this title?" — we need an actual evaluator that the caller (`check_after_paste`) will use. It must understand all 3 match types.

- [ ] **Step 1: Add the evaluator above the test module**

```rust
/// Test whether any TitleRule in any AppProfile would match the given title
/// (for the given app_name). All match_types are evaluated: Exact = literal
/// equality, Text = case-sensitive substring, Regex = regex compile + match.
/// Invalid regex patterns are treated as non-matching.
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
```

Verify `regex` crate is already a dependency:

```bash
rtk grep "^regex" /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml | head -3
```

If absent, run `cargo add regex` inside `src-tauri/`. (It very likely IS present — hotword.rs and others use it.)

- [ ] **Step 2: Add tests for `any_rule_matches`**

```rust
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
        let profiles = vec![make_profile("Slack", vec![make_rule("Slack | #a", TitleMatchType::Exact)])];
        assert!(any_rule_matches(&profiles, "Slack", "Slack | #a"));
        assert!(!any_rule_matches(&profiles, "Slack", "Slack | #b"));
        assert!(!any_rule_matches(&profiles, "Chrome", "Slack | #a"));
    }

    #[test]
    fn any_rule_matches_text_substring() {
        let profiles = vec![make_profile("Slack", vec![make_rule("Slack", TitleMatchType::Text)])];
        assert!(any_rule_matches(&profiles, "Slack", "Slack | #a"));
        assert!(any_rule_matches(&profiles, "Slack", "Slack | #b"));
    }

    #[test]
    fn any_rule_matches_regex() {
        let profiles = vec![make_profile("Slack", vec![make_rule(r"^Slack \| #.+", TitleMatchType::Regex)])];
        assert!(any_rule_matches(&profiles, "Slack", "Slack | #anything"));
        assert!(!any_rule_matches(&profiles, "Slack", "Slack"));
    }

    #[test]
    fn any_rule_matches_invalid_regex_returns_false() {
        let profiles = vec![make_profile("Slack", vec![make_rule(r"[invalid(regex", TitleMatchType::Regex)])];
        assert!(!any_rule_matches(&profiles, "Slack", "Slack | #a"));
    }
```

- [ ] **Step 3: Run tests**

```bash
rtk cargo test --manifest-path /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml -p votype suggestion_engine 2>&1 | tail -10
```

Expected: 19 passed (15 prior + 4 new).

- [ ] **Step 4: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src-tauri/src/managers/suggestion_engine.rs
git -C /Users/zac/code/github/asr/Handy commit -m "Add any_rule_matches evaluator supporting Exact/Text/Regex"
```

---

## Task 6: `respond_rule_suggestion` Tauri command + register

**Files:**

- Modify: `src-tauri/src/shortcut/settings_cmds.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add the command in `settings_cmds.rs`**

Find a good location near the existing `upsert_app_profile` function (around line 260). Add:

```rust
#[tauri::command]
#[specta::specta]
pub async fn respond_rule_suggestion(
    app: AppHandle,
    decision: crate::managers::suggestion_engine::SuggestionDecision,
    app_name: String,
    title: String,
    prompt_id: String,
    threshold: i64,
) -> Result<(), String> {
    use crate::managers::suggestion_engine::{
        apply_accepted_suggestion, record_decision, SuggestionDecision,
    };

    let mut settings = settings::get_settings(&app);

    if matches!(decision, SuggestionDecision::Accepted) {
        apply_accepted_suggestion(&mut settings.app_profiles, &app_name, &title, &prompt_id);
        settings::save_settings(&app, &settings).map_err(|e| e.to_string())?;
    }

    let history = app.state::<std::sync::Arc<crate::managers::history::HistoryManager>>();
    let conn_guard = history.connection();
    let conn = conn_guard.lock().map_err(|e| e.to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    record_decision(&conn, &app_name, &title, threshold, decision, now)
        .map_err(|e| e.to_string())?;

    Ok(())
}
```

**Verify integration points** before relying on the signatures above:

```bash
rtk grep "HistoryManager\|connection" /Users/zac/code/github/asr/Handy/src-tauri/src/managers/history.rs | head -10
rtk grep "fn save_settings\|pub fn get_settings" /Users/zac/code/github/asr/Handy/src-tauri/src/settings.rs | head -5
```

The `HistoryManager::connection()` accessor signature and `save_settings` signature may differ slightly. Adjust:

- If `HistoryManager` exposes `pub fn connection(&self) -> &Mutex<Connection>`, use `.lock()`.
- If `save_settings(app, &settings) -> Result<(), String>`, the call above is correct.
- If `save_settings(app, settings)` (owned), pass `settings` directly.

- [ ] **Step 2: Register the command in `lib.rs`**

There are TWO `collect_commands![ ... ]` builders in lib.rs (around lines 463 and 879 — exact numbers may shift). Add `shortcut::settings_cmds::respond_rule_suggestion,` to **both**. Place it alphabetically next to `upsert_app_profile` to match the file's existing ordering pattern.

Verify locations:

```bash
rtk grep "upsert_app_profile" /Users/zac/code/github/asr/Handy/src-tauri/src/lib.rs | head -5
```

Add the new line right below each `upsert_app_profile` line.

- [ ] **Step 3: Verify build**

```bash
rtk cargo check --manifest-path /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml -p votype 2>&1 | tail -15
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src-tauri/src/shortcut/settings_cmds.rs src-tauri/src/lib.rs
git -C /Users/zac/code/github/asr/Handy commit -m "Add respond_rule_suggestion command"
```

---

## Task 7: `check_after_paste` engine entrypoint + paste-site hooks

**Files:**

- Modify: `src-tauri/src/managers/suggestion_engine.rs`
- Modify: `src-tauri/src/actions/transcribe.rs:2454, 2767, 2944`

- [ ] **Step 1: Add `check_after_paste` to the engine module**

Append (above the `#[cfg(test)]` block):

```rust
/// Per-stop dispatch latch — set when an emission has already happened for the
/// current `TranscribeAction::stop()` run. Prevents stacking multiple overlay
/// popups when a single paste crosses several thresholds.
///
/// The implementer in Task 7 wires reset of this latch into `TranscribeAction::start()`.
static EMISSION_LATCH_THIS_STOP: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn reset_emission_latch() {
    EMISSION_LATCH_THIS_STOP.store(false, std::sync::atomic::Ordering::Relaxed);
}

pub fn check_after_paste(app: &tauri::AppHandle, history_id: i64) {
    use tauri::{Emitter, Manager};

    // Single-fire-per-stop latch
    if EMISSION_LATCH_THIS_STOP.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    let settings = crate::settings::get_settings(app);
    let history = match app.try_state::<std::sync::Arc<crate::managers::history::HistoryManager>>() {
        Some(h) => h,
        None => return,
    };
    let conn_guard = history.connection();
    let conn = match conn_guard.lock() {
        Ok(c) => c,
        Err(_) => return,
    };

    let profiles_snapshot = settings.app_profiles.clone();
    let matcher = move |app_name: &str, title: &str| {
        any_rule_matches(&profiles_snapshot, app_name, title)
    };

    let payload = match compute_suggestion(&conn, history_id, &matcher) {
        Ok(Some(p)) => p,
        Ok(None) => return,
        Err(e) => {
            log::warn!("[SuggestionEngine] DB error in compute_suggestion: {}", e);
            return;
        }
    };

    // Resolve human-readable prompt_name (sentinels and Skill ids both).
    let prompt_name = resolve_prompt_name(&settings, &payload.prompt_id);

    let emit_payload = SuggestionPayload {
        prompt_name,
        ..payload
    };

    if let Err(e) = app.emit("rule-suggestion-show", &emit_payload) {
        log::warn!("[SuggestionEngine] Failed to emit rule-suggestion-show: {}", e);
    }
}

fn resolve_prompt_name(settings: &crate::settings::AppSettings, prompt_id: &str) -> String {
    match prompt_id {
        "__PASS_THROUGH__" => "无需润色".to_string(),
        "__LITE_POLISH__" => "轻量润色".to_string(),
        id => settings
            .skills
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| id.to_string()),
    }
}
```

**Verify**:

- `crate::settings::AppSettings` is the correct settings struct name (likely yes, but search to confirm).
- `settings.skills` is the correct field name (Skill list lives in settings).

```bash
rtk grep "pub skills:\|pub struct AppSettings" /Users/zac/code/github/asr/Handy/src-tauri/src/settings.rs | head -5
```

If `skills` is named differently (e.g. `prompts` or `skill_list`), adjust the lookup.

- [ ] **Step 2: Hook `reset_emission_latch()` into recording start**

In `src-tauri/src/actions/transcribe.rs`, find `impl ShortcutAction for TranscribeAction` block, specifically the `fn start(...)` method (around line 776). Add at the top of the method body:

```rust
        crate::managers::suggestion_engine::reset_emission_latch();
```

- [ ] **Step 3: Hook `check_after_paste()` into the 3 paste sites**

Inspect each existing paste call. The pattern in transcribe.rs around line 2454 (and 2767, 2944) currently looks roughly like:

```rust
if let Err(e) = utils::paste(paste_text, ah_inner) {
    error!("Failed to paste multi-model result: {}", e);
}
```

After each one, insert the check (using the appropriate `app_handle` / `ah_inner` variable name in scope):

```rust
if let Err(e) = utils::paste(paste_text, ah_inner) {
    error!("Failed to paste multi-model result: {}", e);
} else if let Some(hid) = history_id_for_suggestion {
    crate::managers::suggestion_engine::check_after_paste(&ah_inner, hid);
}
```

The variable `history_id_for_suggestion` must be the `i64` history_id known at this point. Trace back from each site to find the existing `presave_history_id` or `history_id` binding. Use the actual variable name — `history_id_for_suggestion` here is a placeholder shorthand.

**Read each site carefully** with a 30-line window around it:

```bash
rtk read /Users/zac/code/github/asr/Handy/src-tauri/src/actions/transcribe.rs --offset 2440 --limit 30
rtk read /Users/zac/code/github/asr/Handy/src-tauri/src/actions/transcribe.rs --offset 2750 --limit 30
rtk read /Users/zac/code/github/asr/Handy/src-tauri/src/actions/transcribe.rs --offset 2930 --limit 30
```

At each site, the history row's id should be available via a previously-bound variable (e.g. `presave_history_id`, `history_id`, or `new_history_id`). Bind a local `let hid_for_suggest: Option<i64> = ...` capturing it before the paste call if not already present.

- [ ] **Step 4: Run build to make sure nothing breaks**

```bash
rtk cargo build --manifest-path /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml -p votype 2>&1 | tail -10
```

Expected: clean. Common pitfalls:

- AppHandle clone — `check_after_paste` takes `&AppHandle`, so pass `&ah_inner` not `ah_inner.clone()`.
- Move semantics — if `ah_inner` is moved into a closure right after paste, store `let ah_for_check = ah_inner.clone()` before paste.

- [ ] **Step 5: Run engine tests (unchanged, but smoke test)**

```bash
rtk cargo test --manifest-path /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml -p votype suggestion_engine 2>&1 | tail -10
```

Expected: 19 passed.

- [ ] **Step 6: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src-tauri/src/managers/suggestion_engine.rs src-tauri/src/actions/transcribe.rs
git -C /Users/zac/code/github/asr/Handy commit -m "Wire check_after_paste into all three paste sites with single-fire latch"
```

---

## Task 8: Regenerate `src/bindings.ts`

**Files:**

- Modify (generated): `src/bindings.ts`

- [ ] **Step 1: Run tauri dev briefly to trigger specta export**

Specta export runs in `pub fn run()` under `#[cfg(debug_assertions)]`. The plain `cargo build` doesn't execute it.

```bash
cd /Users/zac/code/github/asr/Handy && bun tauri dev > /tmp/votype-bindings-gen.log 2>&1 &
sleep 25
pkill -f "tauri dev" ; pkill -f "votype" ; pkill -f "cargo-tauri"
sleep 2
ps aux | grep -E "tauri|votype" | grep -v grep
```

Expected: no leftover processes after kill.

- [ ] **Step 2: Verify the new command + payload appear in bindings**

```bash
rtk grep "respondRuleSuggestion\|RuleSuggestion\|SuggestionPayload\|SuggestionDecision" /Users/zac/code/github/asr/Handy/src/bindings.ts | head -10
```

Expected: at least one match per identifier.

- [ ] **Step 3: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src/bindings.ts
git -C /Users/zac/code/github/asr/Handy commit -m "Regenerate bindings for respond_rule_suggestion command"
```

---

## Task 9: Frontend overlay listener + 3-button UI

**Files:**

- Modify: `src/overlay/RecordingOverlay.tsx`
- Modify: `src/overlay/RecordingOverlay.css`

- [ ] **Step 1: Read the existing overlay structure**

```bash
rtk read /Users/zac/code/github/asr/Handy/src/overlay/RecordingOverlay.tsx --offset 25 --limit 100
rtk read /Users/zac/code/github/asr/Handy/src/overlay/RecordingOverlay.tsx --offset 460 --limit 50
```

Note: the file already has a `SkillConfirmationEvent` listener pattern at line ~464. Mirror its structure.

- [ ] **Step 2: Add the new event type + state**

Near the existing `SkillConfirmationEvent` type (around line 30), add:

```typescript
type RuleSuggestionEvent = {
  app_name: string;
  title: string;
  prompt_id: string;
  prompt_name: string;
  count: number;
  threshold: number;
};

type SuggestionDecision = "accepted" | "dismissed" | "never_again";
```

Near the existing `skillConfirmation` state (around line 124), add:

```typescript
const [ruleSuggestion, setRuleSuggestion] =
  useState<RuleSuggestionEvent | null>(null);
```

- [ ] **Step 3: Add the listener**

Near the existing skill confirmation listener registration (around line 463), add:

```typescript
const unlistenRuleSuggestion = await listen<RuleSuggestionEvent>(
  "rule-suggestion-show",
  (event) => {
    console.log("[RuleSuggestion] Received event:", event.payload);
    setRuleSuggestion(event.payload);
  },
);
```

In the cleanup return block (search for `unlistenSkillConfirmation()`), add:

```typescript
unlistenRuleSuggestion();
```

- [ ] **Step 4: Add the response handler**

Place near the top of the component, alongside other handlers:

```typescript
const respondToRuleSuggestion = useCallback(
  async (decision: SuggestionDecision) => {
    if (!ruleSuggestion) return;
    try {
      await invoke("respond_rule_suggestion", {
        decision,
        appName: ruleSuggestion.app_name,
        title: ruleSuggestion.title,
        promptId: ruleSuggestion.prompt_id,
        threshold: ruleSuggestion.threshold,
      });
    } catch (e) {
      console.error("[RuleSuggestion] respond failed:", e);
    } finally {
      setRuleSuggestion(null);
    }
  },
  [ruleSuggestion],
);
```

Confirm `invoke` is already imported (existing skill-confirmation flow imports it).

- [ ] **Step 5: Render the UI panel**

In the JSX return — find the existing `skillConfirmation && (...)` render block and add a sibling block BELOW it (the overlay can render either, but not both simultaneously; if both are non-null, the later one wins visually):

```tsx
{
  ruleSuggestion && !skillConfirmation && (
    <div className="rule-suggestion-card">
      <div className="rule-suggestion-text">
        {t(
          "overlay.ruleSuggestion.message",
          "You've used '{{promptName}}' {{count}} times in {{appName}} ({{title}}). Add a rule for this window?",
          {
            promptName: ruleSuggestion.prompt_name,
            count: ruleSuggestion.count,
            appName: ruleSuggestion.app_name,
            title:
              ruleSuggestion.title.length > 40
                ? ruleSuggestion.title.slice(0, 40) + "…"
                : ruleSuggestion.title,
          },
        )}
      </div>
      <div className="rule-suggestion-actions">
        <button
          className="rule-suggestion-accept"
          onClick={() => respondToRuleSuggestion("accepted")}
        >
          {t("overlay.ruleSuggestion.accept", "Add rule")}
        </button>
        <button
          className="rule-suggestion-dismiss"
          onClick={() => respondToRuleSuggestion("dismissed")}
        >
          {t("overlay.ruleSuggestion.dismiss", "Not now")}
        </button>
        <button
          className="rule-suggestion-never"
          onClick={() => respondToRuleSuggestion("never_again")}
        >
          {t("overlay.ruleSuggestion.never", "Never ask")}
        </button>
      </div>
    </div>
  );
}
```

Confirm `t` from `useTranslation` is in scope (it should be — the existing overlay uses it).

- [ ] **Step 6: Add minimal styling**

Append to `/Users/zac/code/github/asr/Handy/src/overlay/RecordingOverlay.css`:

```css
.rule-suggestion-card {
  position: fixed;
  bottom: 24px;
  right: 24px;
  max-width: 360px;
  padding: 12px 16px;
  border-radius: 8px;
  background: rgba(20, 20, 24, 0.92);
  color: #fff;
  font-size: 13px;
  line-height: 1.5;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.3);
  z-index: 9999;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.rule-suggestion-text {
  word-break: break-word;
}

.rule-suggestion-actions {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}

.rule-suggestion-actions button {
  flex: 1 1 auto;
  padding: 6px 10px;
  font-size: 12px;
  border: 1px solid rgba(255, 255, 255, 0.15);
  border-radius: 4px;
  cursor: pointer;
  background: transparent;
  color: inherit;
}

.rule-suggestion-accept {
  background: rgba(80, 160, 255, 0.25) !important;
  border-color: rgba(80, 160, 255, 0.5) !important;
}

.rule-suggestion-never {
  opacity: 0.6;
}
```

- [ ] **Step 7: Build to verify**

```bash
cd /Users/zac/code/github/asr/Handy && rtk bun run build 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 8: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src/overlay/RecordingOverlay.tsx src/overlay/RecordingOverlay.css
git -C /Users/zac/code/github/asr/Handy commit -m "Render rule-suggestion overlay with 3-action card"
```

---

## Task 10: Add `Exact` option to TitleMatchType SegmentedControl

**Files:**

- Modify: `src/components/settings/post-processing/AppReviewPolicies.tsx`

- [ ] **Step 1: Locate the TitleMatchType UI**

```bash
rtk grep "TitleMatchType\|match_type\|matchType" /Users/zac/code/github/asr/Handy/src/components/settings/post-processing/AppReviewPolicies.tsx | head -10
```

Find the `SegmentedControl` (or equivalent) where the user picks `text` / `regex` for a TitleRule.

- [ ] **Step 2: Add the `Exact` option**

In the SegmentedControl block, add an `Item` for `exact` next to `text` and `regex`. Example shape (the actual JSX uses Radix `SegmentedControl.Item`):

```tsx
<SegmentedControl.Item value="exact">
  {t("settings.postProcessing.appRules.matchType.exact", "Exact")}
</SegmentedControl.Item>
```

Ensure the parent `SegmentedControl.Root`'s `onValueChange` handler still maps the string back to `TitleMatchType.Exact` — most likely no change is needed because the value `"exact"` matches the `#[serde(rename_all = "lowercase")]` representation of `TitleMatchType::Exact`.

- [ ] **Step 3: Verify build**

```bash
cd /Users/zac/code/github/asr/Handy && rtk bun run build 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src/components/settings/post-processing/AppReviewPolicies.tsx
git -C /Users/zac/code/github/asr/Handy commit -m "Expose TitleMatchType::Exact in AppRules editor"
```

---

## Task 11: i18n keys (en + zh)

**Files:**

- Modify: `src/i18n/locales/en/translation.json`
- Modify: `src/i18n/locales/zh/translation.json`

- [ ] **Step 1: Add the keys to both files**

For `src/i18n/locales/en/translation.json`, add (preserving the existing JSON structure — usually under `"overlay": { ... }` and `"settings": { "postProcessing": { "appRules": { ... } } }`):

```json
{
  "overlay": {
    "ruleSuggestion": {
      "message": "You've used '{{promptName}}' {{count}} times in {{appName}} ({{title}}). Add a rule for this window?",
      "accept": "Add rule",
      "dismiss": "Not now",
      "never": "Never ask"
    }
  },
  "settings": {
    "postProcessing": {
      "appRules": {
        "matchType": {
          "exact": "Exact"
        }
      }
    }
  }
}
```

For `src/i18n/locales/zh/translation.json`, add:

```json
{
  "overlay": {
    "ruleSuggestion": {
      "message": "你在 {{appName}}（{{title}}）已经用「{{promptName}}」{{count}} 次。要为这个窗口加规则吗？",
      "accept": "添加规则",
      "dismiss": "这次不要",
      "never": "别再问"
    }
  },
  "settings": {
    "postProcessing": {
      "appRules": {
        "matchType": {
          "exact": "全量匹配"
        }
      }
    }
  }
}
```

**Important**: do NOT clobber existing keys at those paths. Open the file, find the existing nested structure, merge the new keys in. If `"overlay"` already exists, add `"ruleSuggestion"` inside it; if not, add the whole `"overlay"` object.

- [ ] **Step 2: Verify JSON parses**

```bash
rtk read /Users/zac/code/github/asr/Handy/src/i18n/locales/en/translation.json | python3 -m json.tool > /dev/null && echo "en OK"
rtk read /Users/zac/code/github/asr/Handy/src/i18n/locales/zh/translation.json | python3 -m json.tool > /dev/null && echo "zh OK"
```

Expected: both "OK".

- [ ] **Step 3: Verify build (catches missing-key warnings if i18n is configured to)**

```bash
cd /Users/zac/code/github/asr/Handy && rtk bun run build 2>&1 | tail -8
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src/i18n/locales/en/translation.json src/i18n/locales/zh/translation.json
git -C /Users/zac/code/github/asr/Handy commit -m "Add i18n keys for rule-suggestion overlay and Exact match type"
```

---

## Task 12: Manual BDD verification

**Files:** None (manual)

Walk through the 9 acceptance scenarios from `docs/specs/2026-05-21-app-rule-suggestion-engine.spec.md`. Run once:

```bash
cd /Users/zac/code/github/asr/Handy && bun tauri dev
```

- [ ] **Scenario 1 — happy_path_suggest_and_accept**: In Slack with a fixed title, record 5 times using the same prompt. After the 5th paste, confirm overlay appears at bottom-right. Click "Add rule". Verify Settings → App Rules now has a new TitleRule under Slack with `Exact` match type.

- [ ] **Scenario 2 — mixed_prompts_no_trigger**: Same context, but use 4 Polish + 1 PassThrough in some order. No overlay should appear.

- [ ] **Scenario 3 — existing_rule_skip**: Manually create any TitleRule under Slack that matches the title (Text="Slack" works as a substring rule). Then record 5+ times — no overlay.

- [ ] **Scenario 4 — dismiss_then_retrigger**: Get the overlay at 5, click "Not now". Continue to 10. Overlay reappears at the 10 threshold (text shows "10 times").

- [ ] **Scenario 5 — never_again_silenced**: Get the overlay at 5, click "Never ask". Continue to 10, 20, 40, 80 — no overlay ever.

- [ ] **Scenario 6 — empty_title_skip**: Use an app/window where `fetch_active_window()` returns empty title (e.g. fullscreen presentation, Wayland fallback). Record many times — no overlay.

- [ ] **Scenario 7 — single_dispatch_per_stop**: Hard to reproduce manually — verify code path: in transcribe.rs, the `EMISSION_LATCH_THIS_STOP` flag is checked first thing in `check_after_paste`. Reading the latch implementation in `suggestion_engine.rs` confirms only first call per stop emits.

- [ ] **Scenario 8 — auto_create_app_profile**: Delete any AppProfile for "Slack" first (via Settings → App Rules → trash icon). Then trigger the suggestion and accept. Verify a new AppProfile "Slack" was created with one TitleRule.

- [ ] **Scenario 9 — title_match_type_migration**: Open `~/Library/Application Support/votype/settings.json` (or platform-equivalent). Find an existing TitleRule with `"match_type": "text"`. Restart the app. Verify settings still load (no migration error). Optionally edit a TitleRule to use `"exact"` directly in the JSON and verify it appears as Exact in the UI.

- [ ] **Step**: After verification, commit any incidental tweaks discovered. If a scenario revealed a bug, file it as a fix-commit with a descriptive message.

---

## Task 13: Clippy + warnings sweep + format

**Files:** any touched during impl

Per CLAUDE.md + `feedback_fix_all_warnings.md`: zero new warnings introduced by this feature.

- [ ] **Step 1: Backend clippy (filter to changed files)**

```bash
cd /Users/zac/code/github/asr/Handy && rtk cargo clippy --manifest-path src-tauri/Cargo.toml -p votype --all-targets 2>&1 | grep -E "suggestion_engine|settings_cmds|transcribe|settings\.rs|lib\.rs|managers/mod" | head -30
```

Expected: no warning lines mentioning any of the modified files. Pre-existing warnings in `pipeline.rs`, `core.rs`, `routing.rs`, etc., are out of scope.

- [ ] **Step 2: Backend tests final**

```bash
rtk cargo test --manifest-path src-tauri/Cargo.toml -p votype suggestion_engine 2>&1 | tail -5
rtk cargo test --manifest-path src-tauri/Cargo.toml -p votype title_match_type_tests 2>&1 | tail -5
```

Expected: 19 + 3 = 22 passed in total across the two test groups.

- [ ] **Step 3: Frontend build + tsc**

```bash
cd /Users/zac/code/github/asr/Handy && rtk bun run build 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 4: Format (prettier + cargo fmt)**

```bash
cd /Users/zac/code/github/asr/Handy && bun format
```

- [ ] **Step 5: Commit any format / warning fixes**

```bash
cd /Users/zac/code/github/asr/Handy && git add -A && git diff --cached --stat
git -C /Users/zac/code/github/asr/Handy commit -m "Polish formatting and warnings for rule suggestion engine" || true
```

(The `|| true` is so the script doesn't error if there's nothing to commit.)

---

## Task 14: Append Implementation Deviations to spec

**Files:**

- Modify: `docs/specs/2026-05-21-app-rule-suggestion-engine.spec.md` (the `## 实施偏差` table at the bottom)

- [ ] **Step 1: Fill the deviations table**

Open the spec. Find the `## 实施偏差` table. Replace the placeholder row with each real deviation observed during impl. If none, write a single row: "None observed — implementation matched spec exactly."

Format:

```markdown
| 原计划                                                        | 实际实现                                                                                   | 原因                                                                                            |
| ------------------------------------------------------------- | ------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------- |
| Single-fire latch is a static AtomicBool in suggestion_engine | Same, but `reset_emission_latch()` exposed publicly so transcribe.rs::start() can clear it | Encapsulation tradeoff — static AtomicBool keeps state isolated but `start()` needs to reset it |
| ...                                                           | ...                                                                                        | ...                                                                                             |
```

- [ ] **Step 2: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add docs/specs/2026-05-21-app-rule-suggestion-engine.spec.md
git -C /Users/zac/code/github/asr/Handy commit -m "Record implementation deviations for suggestion engine"
```

---

## Self-Review Mapping (spec → tasks)

- **Spec §约束 "信号源只用 transcription_history 现有列"** → Task 3 (`compute_suggestion` queries those columns only)
- **Spec §约束 "三元组计数严格"** → Task 3 tests 2, 4 (`mixed_prompts_do_not_aggregate`)
- **Spec §约束 "自动写入用 Exact"** → Task 1 + Task 4 `apply_accepted_suggestion`
- **Spec §约束 "触发时机：paste 之后"** → Task 7 hook sites
- **Spec §约束 "复用现有 overlay"** → Task 9 (RecordingOverlay listener)
- **Spec §约束 "单次 stop() 最多 1 弹窗"** → Task 7 `EMISSION_LATCH_THIS_STOP`
- **Spec §决策 "5/10/20/40 倍增"** → Task 2 `THRESHOLDS` constant + Task 3 tests
- **Spec §决策 "TitleMatchType::Exact 新增 + serde 兼容"** → Task 1
- **Spec §决策 "app_rule_suggestions SQLite 表"** → Task 2 `MIGRATION_SQL`
- **Spec §决策 "auto-create AppProfile if absent"** → Task 4 `apply_accepted_suggestion` + Task 4 unit test
- **Spec §决策 "已存在任一 TitleRule 命中即跳过"** → Task 5 `any_rule_matches` (handles Exact/Text/Regex)
- **Spec §验收场景 1-9** → Task 12 sub-tasks 1-9
- **Spec §约束 "消除所有 warning"** → Task 13
