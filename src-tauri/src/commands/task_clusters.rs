use std::sync::Arc;
use tauri::{AppHandle, State};

use crate::actions::task_cluster_generator::{
    generate_task_clusters as generator_run, ClusterableEntry, GenerateClustersInput,
};
use crate::managers::cluster_feedback::ClusterFeedbackManager;
use crate::managers::history::HistoryManager;
use crate::managers::prompt::PromptManager;
use crate::managers::summary::SummaryManager;
use crate::managers::task_clusters::{TaskCluster, TaskClustersManager};

/// Return all task clusters stored for a given `YYYY-MM-DD` date, sorted by
/// `total_duration_ms` desc.
#[tauri::command]
pub async fn get_task_clusters_by_date(
    _app: AppHandle,
    manager: State<'_, Arc<TaskClustersManager>>,
    date: String,
) -> Result<Vec<TaskCluster>, String> {
    manager.get_by_date(&date).map_err(|e| e.to_string())
}

/// Run the LLM-driven clustering pipeline for a date and return the resulting
/// clusters (protected + freshly generated).
#[tauri::command]
pub async fn generate_task_clusters(
    app: AppHandle,
    task_clusters_manager: State<'_, Arc<TaskClustersManager>>,
    cluster_feedback_manager: State<'_, Arc<ClusterFeedbackManager>>,
    prompt_manager: State<'_, Arc<PromptManager>>,
    history_manager: State<'_, Arc<HistoryManager>>,
    summary_manager: State<'_, Arc<SummaryManager>>,
    date: String,
    force: bool,
) -> Result<Vec<TaskCluster>, String> {
    // Resolve summary_id for the date (creates a placeholder summary if missing)
    let summary_id = summary_manager
        .get_or_create_summary_id_for_date(&date)
        .await
        .map_err(|e| format!("failed to resolve summary: {}", e))?;

    // Pull entries for the day
    let entries_raw = history_manager
        .get_entries_for_date(&date)
        .await
        .map_err(|e| format!("failed to load history: {}", e))?;
    let entries: Vec<ClusterableEntry> = entries_raw
        .into_iter()
        .map(|e| ClusterableEntry {
            id: e.id,
            timestamp_ms: e.timestamp,
            app_name: e.app_name,
            window_title: e.window_title,
            text: e.post_processed_text.unwrap_or(e.transcription_text),
            duration_ms: e.duration_ms.unwrap_or(0),
        })
        .collect();

    let input = GenerateClustersInput {
        date,
        summary_id,
        entries,
        force,
    };

    let result = generator_run(
        &app,
        task_clusters_manager.inner().clone(),
        cluster_feedback_manager.inner().clone(),
        prompt_manager.inner().clone(),
        input,
    )
    .await
    .map_err(|e| e.to_string())?;

    Ok(result.clusters)
}

/// Update a single user-editable field on a cluster (`title`, `status`,
/// `next_step`). Marks the cluster as user-modified so future generations
/// preserve it.
#[tauri::command]
pub async fn update_task_cluster_field(
    _app: AppHandle,
    manager: State<'_, Arc<TaskClustersManager>>,
    cluster_id: String,
    field: String,
    value: String,
) -> Result<(), String> {
    manager
        .update_field(&cluster_id, &field, &value)
        .map_err(|e| e.to_string())
}

/// Extract some history rows from an existing cluster into a brand-new
/// cluster, transferring `extracted_duration_ms` of the parent's duration.
/// Returns the new cluster's id.
#[tauri::command]
pub async fn split_task_cluster(
    _app: AppHandle,
    manager: State<'_, Arc<TaskClustersManager>>,
    cluster_id: String,
    extract_ids: Vec<i64>,
    new_title: String,
    extracted_duration_ms: i64,
) -> Result<String, String> {
    manager
        .split(&cluster_id, &extract_ids, &new_title, extracted_duration_ms)
        .map_err(|e| e.to_string())
}

/// Merge `source_cluster_ids` into `target_cluster_id`. Sources are deleted
/// and their history ids / apps / keywords / blockers are unioned into the
/// target.
#[tauri::command]
pub async fn merge_task_clusters(
    _app: AppHandle,
    manager: State<'_, Arc<TaskClustersManager>>,
    target_cluster_id: String,
    source_cluster_ids: Vec<String>,
) -> Result<(), String> {
    manager
        .merge(&target_cluster_id, &source_cluster_ids)
        .map_err(|e| e.to_string())
}

/// Permanently delete a cluster row. History rows referenced by the cluster
/// are not touched.
#[tauri::command]
pub async fn delete_task_cluster(
    _app: AppHandle,
    manager: State<'_, Arc<TaskClustersManager>>,
    cluster_id: String,
) -> Result<(), String> {
    manager.delete(&cluster_id).map_err(|e| e.to_string())
}
