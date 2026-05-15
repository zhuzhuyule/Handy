// Helpers and orchestrator consumed by T8 command layer; allow until callers land.
#![allow(dead_code)]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::actions::post_process::execute_llm_request_with_retry;
use crate::managers::cluster_feedback::ClusterFeedbackManager;
use crate::managers::prompt::{substitute_variables, PromptManager};
use crate::managers::task_clusters::{TaskCluster, TaskClustersManager};
use tauri::AppHandle;

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
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.format("%H:%M").to_string(),
        None => "??:??".to_string(),
    }
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
            let original_ids_len = o.source_history_ids.len();
            o.source_history_ids.retain(|id| valid_ids.contains(id));
            if o.source_history_ids.len() < original_ids_len {
                log::debug!(
                    "task_cluster: dropped {} unknown source_history_ids from cluster '{}'",
                    original_ids_len - o.source_history_ids.len(),
                    o.title
                );
            }
            if !ALLOWED_STATUSES.contains(&o.status.as_str()) {
                log::warn!(
                    "task_cluster: coerced unknown status '{}' -> '进行中' for cluster '{}'",
                    o.status,
                    o.title
                );
                o.status = "进行中".to_string();
            }
            o.entry_count = o.source_history_ids.len() as i64;
            o
        })
        .filter(|o| {
            if o.source_history_ids.is_empty() {
                log::warn!(
                    "task_cluster: dropped cluster '{}' — no valid source_history_ids after sanitization",
                    o.title
                );
                false
            } else {
                true
            }
        })
        .collect()
}

// =============================================================================
// Orchestrator
// =============================================================================

/// Input to `generate_task_clusters`. Caller (T8 command layer) resolves
/// `summary_id` and `entries` from history/summary managers.
pub struct GenerateClustersInput {
    pub date: String,
    pub summary_id: i64,
    pub entries: Vec<ClusterableEntry>,
    pub force: bool,
}

/// Result of `generate_task_clusters`.
pub struct GenerateClustersResult {
    pub clusters: Vec<TaskCluster>,
    pub skipped_reason: Option<String>,
    pub llm_called: bool,
}

const CACHE_TTL_MS: i64 = 60 * 60 * 1000; // 1h
const FEEDBACK_WINDOW_DAYS: i64 = 30;
const FEEDBACK_LIMIT: i64 = 5;
const CLUSTER_PROMPT_ID: &str = "system_task_clustering";

/// Orchestrate AI clustering for a date:
///   1. Cache freshness check (skip if all clusters <1h old, unless force)
///   2. Protected clusters are preserved verbatim
///   3. Candidate entries (non-protected) are sent to LLM
///   4. JSON parse with one strict-mode retry on failure
///   5. Sanitize against valid entry ids
///   6. Delete unmodified clusters for date, upsert new ones
///   7. Return combined (protected + new), ordered by duration desc
pub async fn generate_task_clusters(
    app_handle: &AppHandle,
    task_clusters_manager: Arc<TaskClustersManager>,
    cluster_feedback_manager: Arc<ClusterFeedbackManager>,
    prompt_manager: Arc<PromptManager>,
    input: GenerateClustersInput,
) -> Result<GenerateClustersResult> {
    let GenerateClustersInput {
        date,
        summary_id,
        entries,
        force,
    } = input;

    // --- Step 1: Cache freshness check --------------------------------------
    if !force {
        let existing = task_clusters_manager.get_by_date(&date)?;
        if !existing.is_empty() {
            let now = chrono::Utc::now().timestamp_millis();
            let all_fresh = existing.iter().all(|c| now - c.created_at < CACHE_TTL_MS);
            if all_fresh {
                log::debug!(
                    "task_cluster: cache hit for date={} ({} clusters)",
                    date,
                    existing.len()
                );
                return Ok(GenerateClustersResult {
                    clusters: existing,
                    skipped_reason: Some("cache_hit".into()),
                    llm_called: false,
                });
            }
        }
    }

    // --- Step 2: Empty-day short-circuit ------------------------------------
    if entries.is_empty() {
        log::debug!("task_cluster: no entries for date={}, skipping", date);
        return Ok(GenerateClustersResult {
            clusters: vec![],
            skipped_reason: Some("no_entries".into()),
            llm_called: false,
        });
    }

    // --- Step 3: Protected clusters + candidate filtering -------------------
    let protected = task_clusters_manager.get_protected_clusters_for_date(&date)?;
    let protected_ids: std::collections::HashSet<i64> = protected
        .iter()
        .flat_map(|c| c.source_history_ids.iter().copied())
        .collect();

    let candidate_entries: Vec<ClusterableEntry> = entries
        .iter()
        .filter(|e| !protected_ids.contains(&e.id))
        .cloned()
        .collect();

    if candidate_entries.is_empty() {
        log::debug!(
            "task_cluster: all {} entries are protected for date={}",
            entries.len(),
            date
        );
        return Ok(GenerateClustersResult {
            clusters: protected,
            skipped_reason: Some("all_protected".into()),
            llm_called: false,
        });
    }

    // --- Step 4: Render prompt ----------------------------------------------
    let template = prompt_manager
        .get_prompt(app_handle, CLUSTER_PROMPT_ID)
        .map_err(|e| anyhow::anyhow!("failed to load clustering prompt: {}", e))?;

    let feedback_entries = cluster_feedback_manager
        .list_recent_negative_with_notes(FEEDBACK_WINDOW_DAYS, FEEDBACK_LIMIT)?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let feedback_lines: Vec<(i64, String)> = feedback_entries
        .iter()
        .filter_map(|f| f.note.as_ref().map(|n| (f.created_at, n.clone())))
        .collect();

    let entries_block = render_entries_block(&candidate_entries);
    let protected_block = render_protected_block(&protected);
    let feedback_block = render_feedback_block(&feedback_lines, now_ms);

    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("date", date.clone());
    vars.insert("entry_count", candidate_entries.len().to_string());
    vars.insert("entries", entries_block);
    vars.insert("protected_clusters", protected_block.clone());
    vars.insert("user_feedback", feedback_block.clone());

    let rendered = substitute_variables(&template, &vars);
    let rendered = strip_conditional_block(
        &rendered,
        "protected_clusters_block",
        protected_block.is_empty(),
    );
    let rendered =
        strip_conditional_block(&rendered, "user_feedback_block", feedback_block.is_empty());

    // --- Step 5: Resolve settings, provider, model --------------------------
    let settings = crate::settings::get_settings(app_handle);
    let (provider, model, cached_model_id) = resolve_clustering_target(&settings)?;

    // --- Step 6: Call LLM with retry ----------------------------------------
    let system_prompts = vec![rendered];
    let llm_result = execute_llm_request_with_retry(
        app_handle,
        &settings,
        &provider,
        &model,
        cached_model_id.as_deref(),
        &system_prompts,
        None,
        None,
        None,
    )
    .await;

    let raw_text = match llm_result {
        Ok(resp) => resp.text,
        Err(e) => anyhow::bail!("LLM clustering call failed: {}", e),
    };

    // --- Step 7: Parse with strict-mode retry on failure --------------------
    let parsed = match parse_llm_output(&raw_text) {
        Ok(p) => p,
        Err(first_err) => {
            log::warn!(
                "task_cluster: initial JSON parse failed ({}), retrying with strict instruction",
                first_err
            );
            let mut strict_system = system_prompts.clone();
            strict_system
                .push("Return ONLY a valid JSON array. No prose, no markdown fences.".into());
            let retry_resp = execute_llm_request_with_retry(
                app_handle,
                &settings,
                &provider,
                &model,
                cached_model_id.as_deref(),
                &strict_system,
                None,
                None,
                None,
            )
            .await
            .map_err(|e| anyhow::anyhow!("LLM strict-retry call failed: {}", e))?;
            parse_llm_output(&retry_resp.text)?
        }
    };

    // --- Step 8: Sanitize against valid candidate ids -----------------------
    let valid_ids: std::collections::HashSet<i64> =
        candidate_entries.iter().map(|e| e.id).collect();
    let sanitized = sanitize_outputs(parsed, &valid_ids);

    // --- Step 9: Replace AI-generated clusters for the date -----------------
    task_clusters_manager.delete_unmodified_for_date(&date)?;

    let upsert_ts = chrono::Utc::now().timestamp_millis();
    let mut inserted: Vec<TaskCluster> = Vec::with_capacity(sanitized.len());
    for o in sanitized {
        let total_duration_ms =
            compute_total_duration_ms(&o.source_history_ids, &candidate_entries);
        let cluster = TaskCluster {
            id: TaskClustersManager::new_cluster_id(),
            summary_id,
            date: date.clone(),
            title: o.title,
            status: o.status,
            time_span: o.time_span,
            apps: o.apps,
            source_history_ids: o.source_history_ids.clone(),
            total_duration_ms,
            entry_count: o.source_history_ids.len() as i64,
            summary: o.summary,
            blockers: o.blockers,
            next_step: o.next_step,
            keywords: o.keywords,
            is_user_modified: false,
            user_modified_fields: vec![],
            created_at: upsert_ts,
            updated_at: upsert_ts,
        };
        task_clusters_manager.upsert(&cluster)?;
        inserted.push(cluster);
    }

    // --- Step 10: Combine protected + new, sort by duration desc ------------
    let mut combined = protected;
    combined.extend(inserted);
    combined.sort_by(|a, b| b.total_duration_ms.cmp(&a.total_duration_ms));

    log::info!(
        "task_cluster: generated {} clusters for date={} ({} protected, {} new)",
        combined.len(),
        date,
        combined.iter().filter(|c| c.is_user_modified).count(),
        combined.iter().filter(|c| !c.is_user_modified).count(),
    );

    Ok(GenerateClustersResult {
        clusters: combined,
        skipped_reason: None,
        llm_called: true,
    })
}

/// Resolve provider + model + optional cached_model_id for the clustering call.
/// Reuses the same selection logic as the rest of the post-process pipeline:
///   1. Active post-process provider (from settings.post_process_provider_id, etc.)
///   2. Model: selected_prompt_model → cached_models lookup → post_process_models[provider.id]
fn resolve_clustering_target(
    settings: &crate::settings::AppSettings,
) -> Result<(crate::settings::PostProcessProvider, String, Option<String>)> {
    let provider = settings
        .active_post_process_provider()
        .ok_or_else(|| anyhow::anyhow!("no active post-process provider configured"))?
        .clone();

    let selected_cached = settings
        .selected_prompt_model
        .as_ref()
        .map(|c| &c.primary_id)
        .and_then(|id| {
            settings
                .cached_models
                .iter()
                .find(|m| m.id == *id && m.provider_id == provider.id)
        });

    let model = selected_cached
        .map(|m| m.model_id.clone())
        .or_else(|| settings.post_process_models.get(&provider.id).cloned())
        .ok_or_else(|| anyhow::anyhow!("no model configured for provider '{}'", provider.id))?;

    let cached_model_id = selected_cached.map(|m| m.id.clone());
    Ok((provider, model, cached_model_id))
}

/// Remove (or keep, stripping markers only) a `{{#name}}...{{/name}}` block in the template.
/// If `is_empty` is true, removes the whole block. Otherwise just strips the open/close markers.
fn strip_conditional_block(rendered: &str, block_name: &str, is_empty: bool) -> String {
    let open = format!("{{{{#{}}}}}", block_name);
    let close = format!("{{{{/{}}}}}", block_name);
    if !is_empty {
        rendered.replace(&open, "").replace(&close, "")
    } else if let (Some(s), Some(e)) = (rendered.find(&open), rendered.find(&close)) {
        let mut out = String::with_capacity(rendered.len());
        out.push_str(&rendered[..s]);
        out.push_str(&rendered[e + close.len()..]);
        out
    } else {
        rendered.to_string()
    }
}

/// Sum duration_ms for entries whose ids appear in `ids`.
fn compute_total_duration_ms(ids: &[i64], entries: &[ClusterableEntry]) -> i64 {
    let id_set: std::collections::HashSet<i64> = ids.iter().copied().collect();
    entries
        .iter()
        .filter(|e| id_set.contains(&e.id))
        .map(|e| e.duration_ms)
        .sum()
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
    fn test_format_hhmm_uses_local_time_and_pads() {
        use chrono::TimeZone;
        // 1970-01-01 00:00:00 UTC — under non-UTC offset will not be 00:00.
        // Lock the format/zero-pad behavior using a known offset-from-noon.
        // Choose a timestamp that should map to local 12:34 (use Local::now-derived to be tz-portable).
        let now = chrono::Local::now();
        let target = now.date_naive().and_hms_opt(12, 34, 0).unwrap();
        let target_ms = chrono::Local
            .from_local_datetime(&target)
            .single()
            .unwrap()
            .timestamp_millis();
        assert_eq!(format_hhmm(target_ms), "12:34");

        // Same-day 01:05
        let target = now.date_naive().and_hms_opt(1, 5, 0).unwrap();
        let target_ms = chrono::Local
            .from_local_datetime(&target)
            .single()
            .unwrap()
            .timestamp_millis();
        assert_eq!(format_hhmm(target_ms), "01:05");
    }

    #[test]
    fn test_strip_conditional_block_keeps_when_not_empty() {
        let t = "before {{#x_block}}content{{/x_block}} after";
        let out = strip_conditional_block(t, "x_block", false);
        assert_eq!(out, "before content after");
    }

    #[test]
    fn test_strip_conditional_block_removes_when_empty() {
        let t = "before {{#x_block}}content{{/x_block}} after";
        let out = strip_conditional_block(t, "x_block", true);
        assert_eq!(out, "before  after");
    }

    #[test]
    fn test_compute_total_duration_ms_sums_matching_ids() {
        let entries = vec![
            ClusterableEntry {
                id: 1,
                timestamp_ms: 0,
                app_name: None,
                window_title: None,
                text: "".into(),
                duration_ms: 100,
            },
            ClusterableEntry {
                id: 2,
                timestamp_ms: 0,
                app_name: None,
                window_title: None,
                text: "".into(),
                duration_ms: 200,
            },
            ClusterableEntry {
                id: 3,
                timestamp_ms: 0,
                app_name: None,
                window_title: None,
                text: "".into(),
                duration_ms: 300,
            },
        ];
        assert_eq!(compute_total_duration_ms(&[1, 3], &entries), 400);
        assert_eq!(compute_total_duration_ms(&[], &entries), 0);
        // duplicate ids should not double-count
        assert_eq!(compute_total_duration_ms(&[1, 1], &entries), 100);
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
