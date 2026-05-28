#![allow(dead_code)]

use log::error;
use rusqlite::{params, Connection};
use std::path::PathBuf;

/// Accumulator for pipeline step data, written at the end of unified_post_process.
#[derive(Debug, Default)]
pub struct PipelineDecisionRecord {
    pub history_id: Option<i64>,
    pub input_length: u32,

    // Step 1
    pub history_hit: bool,
    pub history_elapsed_ms: Option<u64>,

    // Step 2
    pub intent_action: Option<String>,
    pub intent_needs_hotword: Option<bool>,
    pub intent_language: Option<String>,
    pub intent_model_id: Option<String>,
    pub intent_provider_id: Option<String>,
    pub intent_elapsed_ms: Option<u64>,
    pub intent_overridden: bool,
    pub intent_override_reason: Option<String>,

    // Step 3
    pub model_selection: Option<String>,
    pub selected_model_id: Option<String>,
    pub selected_provider_id: Option<String>,
    pub is_multi_model: bool,

    // Step 4
    pub result_type: String,
    pub total_elapsed_ms: u64,
    pub error_type: Option<String>,
    pub error_detail: Option<String>,

    // Context
    pub app_name: Option<String>,
    pub smart_routing_enabled: bool,
    pub bypass_reason: Option<String>,
    pub has_cursor_context: bool,
}

pub struct PipelineLogManager {
    db_path: PathBuf,
}

impl PipelineLogManager {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    fn get_connection(&self) -> Result<Connection, rusqlite::Error> {
        let conn = Connection::open(&self.db_path)?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        Ok(conn)
    }

    pub fn log_decision(&self, record: &PipelineDecisionRecord) {
        if let Err(e) = self.log_decision_inner(record) {
            error!("[PipelineLog] Failed to log decision: {}", e);
        }
    }

    fn log_decision_inner(&self, r: &PipelineDecisionRecord) -> Result<(), rusqlite::Error> {
        let conn = self.get_connection()?;
        let now = chrono::Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO pipeline_decisions (
                history_id, timestamp, input_length,
                history_hit, history_elapsed_ms,
                intent_action, intent_needs_hotword, intent_language,
                intent_model_id, intent_provider_id, intent_elapsed_ms,
                intent_overridden, intent_override_reason,
                model_selection, selected_model_id, selected_provider_id,
                is_multi_model,
                result_type, total_elapsed_ms, error_type, error_detail,
                app_name, smart_routing_enabled, bypass_reason,
                has_cursor_context
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
                ?21, ?22, ?23, ?24, ?25
            )",
            params![
                r.history_id,
                now,
                r.input_length,
                r.history_hit,
                r.history_elapsed_ms,
                r.intent_action,
                r.intent_needs_hotword,
                r.intent_language,
                r.intent_model_id,
                r.intent_provider_id,
                r.intent_elapsed_ms,
                r.intent_overridden,
                r.intent_override_reason,
                r.model_selection,
                r.selected_model_id,
                r.selected_provider_id,
                r.is_multi_model,
                r.result_type,
                r.total_elapsed_ms,
                r.error_type,
                r.error_detail,
                r.app_name,
                r.smart_routing_enabled,
                r.bypass_reason,
                r.has_cursor_context,
            ],
        )?;
        Ok(())
    }
}
