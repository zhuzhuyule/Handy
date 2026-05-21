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
