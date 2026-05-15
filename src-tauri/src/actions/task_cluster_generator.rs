// Helpers consumed by T7's `generate_task_clusters` orchestrator; allow until that lands.
#![allow(dead_code)]

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::managers::task_clusters::TaskCluster;

/// Subset of fields the LLM returns; ids are filled, server-side fields (id, summary_id, etc.) added later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmClusterOutput {
    pub title: String,
    pub status: String,
    #[serde(default)]
    pub time_span: Option<String>,
    #[serde(default)]
    pub apps: Vec<String>,
    pub source_history_ids: Vec<i64>,
    #[serde(default)]
    pub total_duration_ms: i64,
    #[serde(default)]
    pub entry_count: i64,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub next_step: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ClusterableEntry {
    pub id: i64,
    pub timestamp_ms: i64,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub text: String,
    pub duration_ms: i64,
}

const ALLOWED_STATUSES: &[&str] = &["进行中", "完成", "卡住", "已搁置"];

/// Render the entries block for the prompt — chooses field strategy based on count.
pub fn render_entries_block(entries: &[ClusterableEntry]) -> String {
    let mut out = String::new();
    let truncate_each = entries.len() > 150;
    for e in entries {
        if truncate_each && e.text.chars().count() < 5 {
            continue;
        }
        let text = if truncate_each {
            e.text.chars().take(200).collect::<String>()
        } else {
            e.text.clone()
        };
        let window = e
            .window_title
            .as_deref()
            .map(|w| w.chars().take(60).collect::<String>())
            .unwrap_or_default();
        let ts = format_hhmm(e.timestamp_ms);
        let app = e.app_name.as_deref().unwrap_or("?");
        out.push_str(&format!(
            "[id={} | {} | {} | {} | \"{}\"]\n",
            e.id, ts, app, window, text
        ));
    }
    out
}

fn format_hhmm(ms: i64) -> String {
    let secs = ms / 1000;
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    format!("{:02}:{:02}", h, m)
}

/// Render the protected cluster summary block.
pub fn render_protected_block(protected: &[TaskCluster]) -> String {
    let mut out = String::new();
    for c in protected {
        out.push_str(&format!(
            "- \"{}\" ids={:?}\n",
            c.title, c.source_history_ids
        ));
    }
    out
}

/// Render recent negative feedback notes for prompt injection.
pub fn render_feedback_block(
    notes: &[(i64, String)], // (created_at_ms, note)
    now_ms: i64,
) -> String {
    let mut out = String::new();
    for (ts, note) in notes {
        let days_ago = ((now_ms - ts) / (24 * 3600 * 1000)).max(0);
        out.push_str(&format!("- {}d ago: \"{}\"\n", days_ago, note));
    }
    out
}

/// Strip ```json fences and surrounding prose if the model added them.
pub fn extract_json_array(raw: &str) -> &str {
    let trimmed = raw.trim();
    // Try fenced ```json ... ```
    if let Some(rest) = trimmed.strip_prefix("```json") {
        if let Some(end) = rest.rfind("```") {
            return rest[..end].trim();
        }
    }
    if let Some(rest) = trimmed.strip_prefix("```") {
        if let Some(end) = rest.rfind("```") {
            return rest[..end].trim();
        }
    }
    // Find the first `[` and the last `]`
    let bytes = trimmed.as_bytes();
    let start = bytes.iter().position(|&b| b == b'[');
    let end = bytes.iter().rposition(|&b| b == b']');
    if let (Some(s), Some(e)) = (start, end) {
        if e > s {
            return &trimmed[s..=e];
        }
    }
    trimmed
}

pub fn parse_llm_output(raw: &str) -> Result<Vec<LlmClusterOutput>> {
    let extracted = extract_json_array(raw);
    let parsed: Vec<LlmClusterOutput> = serde_json::from_str(extracted).map_err(|e| {
        anyhow::anyhow!(
            "failed to parse LLM JSON: {}; raw start: {:.120}",
            e,
            extracted
        )
    })?;
    Ok(parsed)
}

/// Drop ids that do not exist in `valid_ids`. Coerce status to fallback if invalid.
pub fn sanitize_outputs(
    outputs: Vec<LlmClusterOutput>,
    valid_ids: &std::collections::HashSet<i64>,
) -> Vec<LlmClusterOutput> {
    outputs
        .into_iter()
        .map(|mut o| {
            o.source_history_ids.retain(|id| valid_ids.contains(id));
            if !ALLOWED_STATUSES.contains(&o.status.as_str()) {
                o.status = "进行中".to_string();
            }
            o.entry_count = o.source_history_ids.len() as i64;
            o
        })
        .filter(|o| !o.source_history_ids.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_array_plain() {
        let raw = "[{\"title\":\"a\"}]";
        assert_eq!(extract_json_array(raw), "[{\"title\":\"a\"}]");
    }

    #[test]
    fn test_extract_json_array_strips_fence() {
        let raw = "```json\n[{\"x\":1}]\n```";
        assert_eq!(extract_json_array(raw), "[{\"x\":1}]");
    }

    #[test]
    fn test_extract_json_array_strips_prose() {
        let raw = "Sure! Here is the result:\n[{\"x\":1}]\nLet me know.";
        assert_eq!(extract_json_array(raw), "[{\"x\":1}]");
    }

    #[test]
    fn test_parse_llm_output_minimal() {
        let raw = r#"[{"title":"OAuth","status":"进行中","source_history_ids":[1,2]}]"#;
        let parsed = parse_llm_output(raw).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].title, "OAuth");
        assert_eq!(parsed[0].source_history_ids, vec![1, 2]);
    }

    #[test]
    fn test_parse_llm_output_invalid_returns_err() {
        let raw = r#"[{"title":"x" missing colon}]"#;
        assert!(parse_llm_output(raw).is_err());
    }

    #[test]
    fn test_sanitize_drops_unknown_ids() {
        let valid: std::collections::HashSet<i64> = [1_i64, 2, 3].into_iter().collect();
        let inp = vec![LlmClusterOutput {
            title: "x".into(),
            status: "进行中".into(),
            time_span: None,
            apps: vec![],
            source_history_ids: vec![1, 99, 2],
            total_duration_ms: 0,
            entry_count: 3,
            summary: None,
            blockers: vec![],
            next_step: None,
            keywords: vec![],
        }];
        let out = sanitize_outputs(inp, &valid);
        assert_eq!(out[0].source_history_ids, vec![1, 2]);
        assert_eq!(out[0].entry_count, 2);
    }

    #[test]
    fn test_sanitize_drops_empty_clusters() {
        let valid: std::collections::HashSet<i64> = [1, 2].into_iter().collect();
        let inp = vec![LlmClusterOutput {
            title: "x".into(),
            status: "进行中".into(),
            time_span: None,
            apps: vec![],
            source_history_ids: vec![99],
            total_duration_ms: 0,
            entry_count: 1,
            summary: None,
            blockers: vec![],
            next_step: None,
            keywords: vec![],
        }];
        let out = sanitize_outputs(inp, &valid);
        assert!(out.is_empty());
    }

    #[test]
    fn test_sanitize_coerces_bad_status() {
        let valid: std::collections::HashSet<i64> = [1].into_iter().collect();
        let inp = vec![LlmClusterOutput {
            title: "x".into(),
            status: "weird".into(),
            time_span: None,
            apps: vec![],
            source_history_ids: vec![1],
            total_duration_ms: 0,
            entry_count: 1,
            summary: None,
            blockers: vec![],
            next_step: None,
            keywords: vec![],
        }];
        let out = sanitize_outputs(inp, &valid);
        assert_eq!(out[0].status, "进行中");
    }

    #[test]
    fn test_render_entries_truncates_when_many() {
        let mut entries = Vec::new();
        for i in 0..200 {
            entries.push(ClusterableEntry {
                id: i,
                timestamp_ms: i * 60_000,
                app_name: Some("App".into()),
                window_title: Some("win".into()),
                text: "x".repeat(500),
                duration_ms: 1000,
            });
        }
        let out = render_entries_block(&entries);
        // each line should contain the truncated text (200 chars max)
        let first_line = out.lines().next().unwrap();
        assert!(first_line.contains(&"x".repeat(200)));
        assert!(!first_line.contains(&"x".repeat(201)));
    }
}
