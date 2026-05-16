use crate::managers::history::{
    HistoryDashboardStats, HistoryEntry, HistoryManager, PaginatedHistoryResult,
};
use crate::managers::transcription::TranscriptionManager;
use log::{debug, error, info};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

fn extract_online_asr_status(detail: &str) -> Option<u16> {
    let marker = "remote transcription failed (";
    let start = detail.find(marker)? + marker.len();
    let digits: String = detail[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    if digits.len() == 3 {
        digits.parse::<u16>().ok()
    } else {
        None
    }
}

fn classify_online_asr_attempt_error(detail: &str) -> crate::provider_gateway::AttemptError {
    use crate::provider_gateway::{AttemptError, AttemptErrorKind};

    if let Some(status) = extract_online_asr_status(detail) {
        return match status {
            401 | 403 => AttemptError::Fatal {
                status: Some(status),
                detail: detail.to_string(),
                kind: AttemptErrorKind::Http,
            },
            429 | 500..=599 => AttemptError::Retryable {
                status: Some(status),
                detail: detail.to_string(),
                kind: AttemptErrorKind::Http,
            },
            _ => AttemptError::Fatal {
                status: Some(status),
                detail: detail.to_string(),
                kind: AttemptErrorKind::Http,
            },
        };
    }

    let lower = detail.to_ascii_lowercase();
    if lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("failed to send transcription request")
        || lower.contains("connection")
    {
        return AttemptError::Retryable {
            status: None,
            detail: detail.to_string(),
            kind: AttemptErrorKind::Network,
        };
    }

    AttemptError::Retryable {
        status: None,
        detail: detail.to_string(),
        kind: AttemptErrorKind::Other,
    }
}

#[tauri::command]
pub async fn get_history_entries(
    _app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
) -> Result<Vec<HistoryEntry>, String> {
    history_manager
        .get_history_entries()
        .await
        .map_err(|e| e.to_string())
}

/// Get paginated history entries with optional timestamp filtering
#[tauri::command]
pub async fn get_history_entries_paginated(
    _app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    offset: usize,
    limit: usize,
    start_timestamp: Option<i64>,
    end_timestamp: Option<i64>,
) -> Result<PaginatedHistoryResult, String> {
    let (entries, total_count) = history_manager
        .get_history_entries_paginated(offset, limit, start_timestamp, end_timestamp)
        .await
        .map_err(|e| e.to_string())?;

    Ok(PaginatedHistoryResult {
        entries,
        total_count,
        offset,
        limit,
    })
}

/// Batch lookup of history entries by id. Order matches the DB (id ascending);
/// callers should reorder by timestamp if they want chronological output.
#[tauri::command]
pub async fn get_history_entries_by_ids(
    _app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    ids: Vec<i64>,
) -> Result<Vec<HistoryEntry>, String> {
    history_manager
        .get_entries_by_ids(&ids)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn toggle_history_entry_saved(
    _app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    id: i64,
) -> Result<(), String> {
    history_manager
        .toggle_saved_status(id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_audio_file_path(
    _app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    file_name: String,
) -> Result<String, String> {
    let path = history_manager.get_audio_file_path(&file_name);
    path.to_str()
        .ok_or_else(|| "Invalid file path".to_string())
        .map(|s| s.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn get_audio_path_by_history_id(
    _app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    history_id: i64,
) -> Result<String, String> {
    let entry = history_manager
        .get_entry_by_id(history_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("History entry not found: {}", history_id))?;
    let path = history_manager.get_audio_file_path(&entry.file_name);
    path.to_str()
        .ok_or_else(|| "Invalid file path".to_string())
        .map(|s| s.to_string())
}

#[tauri::command]
pub async fn delete_history_entry(
    _app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    task_clusters: State<'_, Arc<crate::managers::task_clusters::TaskClustersManager>>,
    id: i64,
) -> Result<(), String> {
    // Capture duration before deletion so we can subtract it from any task
    // clusters that referenced this history row.
    let duration_ms = history_manager
        .get_entry_by_id(id)
        .await
        .map_err(|e| e.to_string())?
        .and_then(|entry| entry.duration_ms)
        .unwrap_or(0);

    history_manager
        .delete_entry(id)
        .await
        .map_err(|e| e.to_string())?;

    // Cascade: drop the deleted id from every task cluster's
    // source_history_ids and decrement their counts/durations. Failures here
    // are non-fatal — they leave stale ids in cluster JSON that the next
    // regenerate will overwrite. Only log; never propagate.
    if let Err(e) = task_clusters.remove_history_id_from_all_clusters(id, duration_ms) {
        log::warn!(
            "[cascade] Failed to clean task_clusters for deleted history id={}: {}",
            id,
            e
        );
    }

    Ok(())
}

#[tauri::command]
pub async fn get_history_dashboard_stats(
    _app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    days: Option<u32>,
) -> Result<HistoryDashboardStats, String> {
    history_manager
        .get_dashboard_stats(days.unwrap_or(30))
        .await
        .map_err(|e| e.to_string())
}

/// Update a history entry's text field (transcription_text, streaming_text, post_processed_text)
/// or a specific step in post_process_history (when field is "post_process_history_step")
/// The app_name is used to scope vocabulary corrections to specific applications.
#[tauri::command]
pub async fn update_history_entry_text(
    app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    id: i64,
    field: String,
    text: String,
    step_index: Option<usize>,
    app_name: Option<String>,
) -> Result<(), String> {
    history_manager
        .update_history_entry_text(id, &field, text, step_index, app_name)
        .await
        .map_err(|e| e.to_string())?;

    // Emit event to refresh UI
    let _ = app.emit("history-updated", ());

    Ok(())
}

#[tauri::command]
pub async fn update_history_limit(
    app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    limit: usize,
) -> Result<(), String> {
    let mut settings = crate::settings::get_settings(&app);
    settings.history_limit = limit;
    crate::settings::write_settings(&app, settings);

    history_manager
        .cleanup_old_entries()
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn update_recording_retention_period(
    app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    period: String,
) -> Result<(), String> {
    use crate::settings::RecordingRetentionPeriod;

    let retention_period = match period.as_str() {
        "never" => RecordingRetentionPeriod::Never,
        "preserve_limit" => RecordingRetentionPeriod::PreserveLimit,
        "days3" => RecordingRetentionPeriod::Days3,
        "weeks2" => RecordingRetentionPeriod::Weeks2,
        "months3" => RecordingRetentionPeriod::Months3,
        _ => return Err(format!("Invalid retention period: {}", period)),
    };

    let mut settings = crate::settings::get_settings(&app);
    settings.recording_retention_period = retention_period;
    crate::settings::write_settings(&app, settings);

    history_manager
        .cleanup_old_entries()
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn retranscribe_history_entry(
    app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    transcription_manager: State<'_, Arc<TranscriptionManager>>,
    id: i64,
) -> Result<(), String> {
    use crate::audio_toolkit::read_wav_file;
    use crate::settings::get_settings;

    info!("[retranscribe] Starting retranscribe for entry id={}", id);

    let entries = history_manager.get_history_entries().await.map_err(|e| {
        error!("[retranscribe] Failed to get history entries: {}", e);
        e.to_string()
    })?;

    let entry = entries.into_iter().find(|e| e.id == id).ok_or_else(|| {
        error!("[retranscribe] History entry not found: {}", id);
        format!("History entry not found: {}", id)
    })?;

    let file_path = history_manager.get_audio_file_path(&entry.file_name);
    info!(
        "[retranscribe] Audio file: {:?}, exists: {}",
        file_path,
        file_path.exists()
    );
    let samples = read_wav_file(&file_path).map_err(|e| {
        error!(
            "[retranscribe] Failed to read WAV file {:?}: {}",
            file_path, e
        );
        e.to_string()
    })?;
    info!(
        "[retranscribe] Loaded {} samples ({:.1}s)",
        samples.len(),
        samples.len() as f64 / 16000.0
    );

    // Ensure we have a valid duration even if missing in original entry
    let duration_ms = (samples.len() as f64 / 16000.0 * 1000.0) as i64;

    let settings = get_settings(&app);
    let model_id = if settings.online_asr_enabled {
        settings
            .selected_asr_model
            .as_ref()
            .map(|c| c.primary_id.clone())
            .unwrap_or_else(|| "online".to_string())
    } else {
        settings.selected_model.clone()
    };
    info!(
        "[retranscribe] online_asr_enabled={}, model_id={}",
        settings.online_asr_enabled, model_id
    );

    let start_time = std::time::Instant::now();
    let transcription_text = if settings.online_asr_enabled {
        let cached_model = settings
            .cached_models
            .iter()
            .find(|m| m.id == model_id)
            .ok_or_else(|| format!("Online ASR model not found in cached models: {}", model_id))?;
        let provider = settings
            .post_process_providers
            .iter()
            .find(|p| p.id == cached_model.provider_id)
            .cloned()
            .ok_or_else(|| format!("Provider not found: {}", cached_model.provider_id))?;
        let language = settings.selected_language.clone();
        let max_attempts = settings
            .post_process_api_keys
            .get(&provider.id)
            .map(|keys| {
                keys.iter()
                    .filter(|entry| entry.enabled && !entry.key.is_empty())
                    .count()
                    .clamp(1, 3)
            })
            .unwrap_or(1);

        let outcome = crate::provider_gateway::execute_with_failover(
            &app,
            &settings,
            crate::provider_gateway::ExecutionPlan {
                provider_id: provider.id.clone(),
                cached_model_id: cached_model.id.clone(),
                remote_model_id: cached_model.model_id.clone(),
                max_attempts,
            },
            {
                let provider = provider.clone();
                let remote_model_id = cached_model.model_id.clone();
                let samples = Arc::new(samples);
                move |api_key| {
                    let provider = provider.clone();
                    let remote_model_id = remote_model_id.clone();
                    let samples = samples.clone();
                    let language = language.clone();
                    let api_key = api_key.to_string();

                    async move {
                        let result = tokio::task::spawn_blocking(move || {
                            let client = crate::online_asr::OnlineAsrClient::new(
                                16000,
                                std::time::Duration::from_secs(120),
                            );
                            let lang = if language == "auto" {
                                None
                            } else {
                                Some(language.as_str())
                            };
                            client.transcribe(
                                &provider,
                                Some(api_key),
                                &remote_model_id,
                                lang,
                                samples.as_slice(),
                            )
                        })
                        .await;

                        match result {
                            Ok(Ok(text)) => Ok(text),
                            Ok(Err(err)) => {
                                Err(classify_online_asr_attempt_error(&err.to_string()))
                            }
                            Err(err) => Err(crate::provider_gateway::AttemptError::Retryable {
                                status: None,
                                detail: format!("Online ASR task failed: {}", err),
                                kind: crate::provider_gateway::AttemptErrorKind::Other,
                            }),
                        }
                    }
                }
            },
        )
        .await;

        match outcome {
            crate::provider_gateway::ExecutionOutcome::Success(text) => {
                // Apply hotword force replacements on online-ASR retranscribe
                // output before any downstream use (DB update, UI display),
                // matching the convention used for the local engine path.
                crate::managers::hotword::apply_force_replacements_via_state(&app, text)
            }
            crate::provider_gateway::ExecutionOutcome::Fatal { detail, .. } => {
                return Err(detail);
            }
            crate::provider_gateway::ExecutionOutcome::Exhausted { last_error, .. } => {
                let detail = match last_error {
                    crate::provider_gateway::AttemptError::Retryable { detail, .. }
                    | crate::provider_gateway::AttemptError::Fatal { detail, .. } => detail,
                };
                return Err(detail);
            }
        }
    } else {
        // Use local transcription manager
        info!("[retranscribe] Loading local model: {}", model_id);
        transcription_manager.load_model(&model_id).map_err(|e| {
            error!(
                "[retranscribe] Failed to load local model '{}': {}",
                model_id, e
            );
            e.to_string()
        })?;
        info!("[retranscribe] Model loaded, starting transcription...");
        let result = transcription_manager.transcribe(samples).map_err(|e| {
            error!("[retranscribe] Local transcription failed: {}", e);
            e.to_string()
        })?;
        info!(
            "[retranscribe] Local transcription succeeded: {} chars",
            result.chars().count()
        );
        result
    };
    let elapsed = start_time.elapsed().as_millis() as i64;
    info!(
        "[retranscribe] Transcription completed in {}ms, text length={}",
        elapsed,
        transcription_text.len()
    );

    let char_count = transcription_text.chars().count() as i64;

    // Update the existing entry using the manager method
    history_manager
        .update_transcription_content(
            id,
            transcription_text.clone(),
            model_id,
            settings.selected_language.clone(),
            duration_ms,
            elapsed,
            char_count,
        )
        .await
        .map_err(|e| e.to_string())?;

    // Emit event to refresh UI for the raw transcription
    let _ = app.emit("history-updated", ());

    // --- Post-processing ---
    if settings.post_process_enabled {
        use crate::actions::post_process::{
            maybe_convert_chinese_variant, maybe_post_process_transcription,
        };

        // 1. Try Chinese variant conversion first
        if let Some(converted) = maybe_convert_chinese_variant(&settings, &transcription_text).await
        {
            history_manager
                .update_transcription_post_processing(
                    id,
                    converted,
                    String::new(),
                    "OpenCC".to_string(),
                    None,
                    Some("OpenCC".to_string()),
                    None,
                    None,
                )
                .await
                .map_err(|e| e.to_string())?;
        } else {
            // 2. Try LLM post-processing
            // For re-transcription, we don't have a separate streaming result, so we pass None.
            let (llm_result, used_model, prompt_id, _, _, _, _, _, _, _) =
                maybe_post_process_transcription(
                    &app,
                    &settings,
                    &transcription_text,
                    None,
                    false,
                    None,
                    entry.app_name,
                    entry.window_title,
                    None,
                    None,
                    Some(id),
                    false, // skill_mode
                    false, // review_editor_active
                    None,  // selected_text
                    None,  // review_document_text
                    None,  // cursor_context
                    false, // skip_smart_routing
                )
                .await;

            if let Some(processed) = llm_result {
                let mut post_process_prompt_text = String::new();
                let mut post_process_prompt_name = String::new();

                // Find the prompt text to save
                if let Some(pid) = &prompt_id {
                    if let Some(prompt) =
                        settings.post_process_prompts.iter().find(|p| &p.id == pid)
                    {
                        post_process_prompt_text = prompt.instructions.clone();
                        post_process_prompt_name = prompt.name.clone();
                    }
                }

                history_manager
                    .update_transcription_post_processing(
                        id,
                        processed,
                        post_process_prompt_text,
                        post_process_prompt_name,
                        prompt_id,
                        used_model,
                        None,
                        Some(1),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn reprocess_history_entry(
    app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    id: i64,
    prompt_id: String,
    input_text: Option<String>,
) -> Result<(), String> {
    use crate::actions::post_process::post_process_text_with_prompt;
    use crate::settings::get_settings;

    debug!(
        "Command reprocess_history_entry called for ID: {}, Prompt: {}",
        id, prompt_id
    );

    let entry = history_manager
        .get_entry_by_id(id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("History entry not found: {}", id))?;

    let settings = get_settings(&app);

    // Find the specified prompt
    let prompt = settings
        .post_process_prompts
        .iter()
        .find(|p| p.id == prompt_id)
        .ok_or_else(|| format!("Prompt not found: {}", prompt_id))?;

    // Use provided input_text or fallback to the original transcription
    let text_to_process = input_text
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| entry.transcription_text.clone());

    let (llm_result, used_model, used_prompt_id, _, _) = post_process_text_with_prompt(
        &app,
        &settings,
        &text_to_process,
        entry.streaming_text.as_deref(),
        prompt,
        false,
        None,
        None,
        Some(id),
    )
    .await;

    if let Some(text) = llm_result {
        history_manager
            .update_transcription_post_processing(
                id,
                text,
                prompt.instructions.clone(),
                prompt.name.clone(),
                used_prompt_id,
                used_model,
                None,
                Some(1),
            )
            .await
            .map_err(|e| e.to_string())?;

        // CRITICAL: Emit event to refresh the UI
        let _ = app.emit("history-updated", ());
        info!(
            "History entry {} successfully reprocessed and UI updated",
            id
        );
    } else {
        return Err(
            "Post-processing failed to return a result. Check logs for API errors.".to_string(),
        );
    }

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn reject_post_process_result(
    _app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    id: i64,
) -> Result<(), String> {
    history_manager
        .reject_post_process_result(id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn cascade_reject_post_process(
    _app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    transcription_text: String,
    post_processed_text: String,
) -> Result<usize, String> {
    history_manager
        .cascade_reject_post_process(&transcription_text, &post_processed_text)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_model_speed_stats(
    _app: AppHandle,
    llm_metrics: State<'_, Arc<crate::managers::llm_metrics::LlmMetricsManager>>,
) -> Result<Vec<crate::managers::llm_metrics::ModelSpeedStats>, String> {
    llm_metrics
        .get_all_model_speed_stats()
        .map_err(|e| e.to_string())
}

/// Get aggregated LLM usage stats (call count + token total).
/// Optional `since_timestamp` filters to entries after that unix timestamp.
#[tauri::command]
pub async fn get_llm_usage_stats(
    _app: AppHandle,
    llm_metrics: State<'_, Arc<crate::managers::llm_metrics::LlmMetricsManager>>,
    since_timestamp: Option<i64>,
    until_timestamp: Option<i64>,
) -> Result<crate::managers::llm_metrics::LlmUsageStats, String> {
    llm_metrics
        .get_usage_stats(since_timestamp, until_timestamp)
        .map_err(|e| e.to_string())
}
