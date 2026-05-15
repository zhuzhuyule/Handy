use std::sync::Arc;
use tauri::{AppHandle, State};

use crate::managers::cluster_feedback::{ClusterFeedback, ClusterFeedbackManager};

#[tauri::command]
pub async fn add_cluster_feedback(
    _app: AppHandle,
    manager: State<'_, Arc<ClusterFeedbackManager>>,
    cluster_id: String,
    thumb: String,
    note: Option<String>,
) -> Result<i64, String> {
    manager
        .add(&cluster_id, &thumb, note.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_cluster_feedback(
    _app: AppHandle,
    manager: State<'_, Arc<ClusterFeedbackManager>>,
    cluster_id: String,
) -> Result<Vec<ClusterFeedback>, String> {
    manager
        .list_for_cluster(&cluster_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_recent_negative_cluster_feedback(
    _app: AppHandle,
    manager: State<'_, Arc<ClusterFeedbackManager>>,
    days: i64,
    limit: i64,
) -> Result<Vec<ClusterFeedback>, String> {
    manager
        .list_recent_negative_with_notes(days, limit)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_cluster_feedback(
    _app: AppHandle,
    manager: State<'_, Arc<ClusterFeedbackManager>>,
    id: i64,
) -> Result<(), String> {
    manager.delete(id).map_err(|e| e.to_string())
}
