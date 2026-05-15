pub mod audio;
#[allow(dead_code)]
pub mod daily_vocabulary;
pub mod free_models;
pub mod history;
pub mod hotword;
pub mod models;
pub mod summary;
pub mod task_clusters;
pub mod text;
pub mod transcription;
pub mod vocabulary;

use crate::{active_window, app_list, settings, utils::cancel_current_operation};
use tauri::{AppHandle, Manager};
use tauri_plugin_log::LogLevel;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub fn get_app_settings(app: AppHandle) -> settings::AppSettings {
    settings::get_settings(&app)
}

#[tauri::command]
pub fn cancel_operation(app: AppHandle) {
    cancel_current_operation(&app);
}

#[tauri::command]
pub fn get_app_dir_path(app: AppHandle) -> Result<String, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {}", e))?;

    Ok(app_data_dir.to_string_lossy().to_string())
}

#[tauri::command]
pub fn get_log_dir_path(app: AppHandle) -> Result<String, String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|e| format!("Failed to get log directory: {}", e))?;

    Ok(log_dir.to_string_lossy().to_string())
}

#[tauri::command]
pub fn set_log_level(app: AppHandle, level: LogLevel) -> Result<(), String> {
    let log_level: log::Level = level.clone().into();
    // Update the file log level atomic so the filter picks up the new level
    crate::FILE_LOG_LEVEL.store(
        log_level.to_level_filter() as u8,
        std::sync::atomic::Ordering::Relaxed,
    );

    let mut settings = settings::get_settings(&app);
    settings.log_level = level;
    settings::write_settings(&app, settings);

    Ok(())
}

#[tauri::command]
pub fn open_recordings_folder(app: AppHandle) -> Result<(), String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {}", e))?;

    let recordings_dir = app_data_dir.join("recordings");

    let path = recordings_dir.to_string_lossy().as_ref().to_string();
    app.opener()
        .open_path(path, None::<String>)
        .map_err(|e| format!("Failed to open recordings folder: {}", e))?;

    Ok(())
}

#[tauri::command]
pub fn get_recordings_folder_path(app: AppHandle) -> Result<String, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {}", e))?;

    let recordings_dir = app_data_dir.join("recordings");

    Ok(recordings_dir.to_string_lossy().to_string())
}

#[tauri::command]
pub fn open_log_dir(app: AppHandle) -> Result<(), String> {
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|e| format!("Failed to get log directory: {}", e))?;

    std::fs::create_dir_all(&log_dir)
        .map_err(|e| format!("Failed to create log directory: {}", e))?;

    let path = log_dir.to_string_lossy().as_ref().to_string();
    app.opener()
        .open_path(path, None::<String>)
        .map_err(|e| format!("Failed to open log directory: {}", e))?;

    Ok(())
}

#[tauri::command]
pub fn open_app_data_dir(app: AppHandle) -> Result<(), String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {}", e))?;

    let path = app_data_dir.to_string_lossy().as_ref().to_string();
    app.opener()
        .open_path(path, None::<String>)
        .map_err(|e| format!("Failed to open app data directory: {}", e))?;

    Ok(())
}

#[tauri::command]
pub fn get_active_window_info() -> Result<active_window::ActiveWindowInfo, String> {
    active_window::fetch_active_window()
}

#[tauri::command]
pub fn list_installed_apps(
    app: AppHandle,
    refresh: Option<bool>,
) -> Result<Vec<app_list::InstalledApp>, String> {
    let cache_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data directory: {}", e))?;
    Ok(app_list::list_installed_apps(
        &cache_dir,
        refresh.unwrap_or(false),
    ))
}

#[tauri::command]
pub fn get_cursor_position() -> Result<active_window::CursorPosition, String> {
    active_window::fetch_cursor_position()
}

#[tauri::command]
pub fn show_main_window(app: AppHandle) -> Result<(), String> {
    crate::utils::show_or_create_main_window(&app, Some("dashboard")).map(|_| ())
}

#[tauri::command]
pub fn get_first_history_entry(
    app: AppHandle,
) -> Result<Option<crate::managers::history::HistoryEntry>, String> {
    use crate::managers::history::HistoryManager;
    use std::sync::Arc;
    use tauri::async_runtime::block_on;

    // Get the history manager state
    let history_manager = app.state::<Arc<HistoryManager>>();

    // Get all history entries
    let entries = block_on(history_manager.get_history_entries()).map_err(|e| e.to_string())?;

    // Return the first non-deleted entry
    Ok(entries.into_iter().find(|e| !e.deleted))
}

#[tauri::command]
pub fn paste_text_to_active_window(app: AppHandle, text: String) -> Result<(), String> {
    crate::clipboard::paste(text, app)
}

/// Paste text to the previously active window (saved before review window was shown)
/// This first focuses the saved window, then pastes the text
#[tauri::command]
pub fn paste_to_previous_window(app: AppHandle, text: String) -> Result<(), String> {
    use std::time::Duration;

    // Focus the previously active window first
    if let Some(info) = crate::review_window::get_last_active_window() {
        if let Err(e) = crate::active_window::focus_app_by_pid(info.process_id) {
            log::warn!("Failed to focus previous app: {}", e);
        } else {
            std::thread::sleep(Duration::from_millis(120));
        }
    }

    // Then paste the text
    crate::clipboard::paste(text, app)
}

#[tauri::command]
pub fn log_to_console(msg: String, level: Option<String>) {
    // Default to info if no level provided
    let log_level = match level.as_deref() {
        Some("error") => log::Level::Error,
        Some("warn") => log::Level::Warn,
        Some("debug") => log::Level::Debug,
        Some("trace") => log::Level::Trace,
        _ => log::Level::Info,
    };

    log::log!(log_level, "[Frontend] {}", msg);
}

/// Request focus for overlay window so it can receive keyboard input
/// Called from frontend after skill confirmation UI is rendered
#[tauri::command]
pub fn focus_overlay(app: AppHandle) {
    use crate::ManagedPendingSkillConfirmation;
    use tauri::Manager;

    // When a skill confirmation is pending, the overlay carries interactive
    // Confirm/Cancel buttons that the user MUST be able to operate. Bypass the
    // usual "don't steal focus from Votype" guard in this case — without focus
    // the panel won't receive keyboard events and mouse clicks may not land on
    // the right window. The Votype-window skip below only protects the
    // passive recording overlay where focus theft would be intrusive.
    let has_pending_skill =
        if let Some(pending_state) = app.try_state::<ManagedPendingSkillConfirmation>() {
            pending_state
                .lock()
                .map(|g| g.skill_id.is_some())
                .unwrap_or(false)
        } else {
            false
        };

    if !has_pending_skill {
        let active_window = crate::active_window::fetch_active_window().ok();
        let votype_mode = crate::window_context::resolve_votype_input_mode(
            active_window.as_ref().map(|info| info.app_name.as_str()),
            active_window.as_ref().map(|info| info.title.as_str()),
            crate::review_window::is_review_editor_active(),
            false,
        );

        if matches!(
            votype_mode,
            crate::window_context::VotypeInputMode::MainPolishInput
                | crate::window_context::VotypeInputMode::MainSelectedEdit
                | crate::window_context::VotypeInputMode::ReviewRewrite
        ) {
            log::info!(
                "[Overlay] Skip focus_overlay while Votype window is active (mode={:?})",
                votype_mode
            );
            return;
        }
    }

    // Set UI visible flag so global ESC can skip its own handler
    if let Some(pending_state) = app.try_state::<ManagedPendingSkillConfirmation>() {
        if let Ok(mut guard) = pending_state.lock() {
            guard.is_ui_visible = true;
        }
    }

    crate::overlay::focus_recording_overlay(&app);
}

/// Handle user response to skill confirmation prompt
#[tauri::command]
pub async fn confirm_skill(app: AppHandle, skill_id: String, accepted: bool) -> Result<(), String> {
    use crate::ManagedPendingSkillConfirmation;
    use tauri::Manager;

    // Get pending confirmation state
    let pending_state = app.state::<ManagedPendingSkillConfirmation>();
    let pending = {
        let guard = pending_state.lock().map_err(|e| e.to_string())?;
        guard.clone()
    };

    // Clear the pending state
    {
        let mut guard = pending_state.lock().map_err(|e| e.to_string())?;
        *guard = crate::PendingSkillConfirmation::default();
    }

    // Verify skill_id matches
    if pending.skill_id.as_ref() != Some(&skill_id) {
        return Err("Skill confirmation mismatch".to_string());
    }

    let transcription = pending
        .transcription
        .ok_or("No transcription in pending state")?;
    let settings = crate::settings::get_settings(&app);
    // Clone app for use after paste (which takes ownership)
    let app_for_cleanup = app.clone();

    // Restore focus to original window before pasting
    if let Some(pid) = pending.process_id {
        log::info!("[SkillConfirmation] Restoring focus to PID: {}", pid);
        if let Err(e) = crate::active_window::focus_app_by_pid(pid) {
            log::warn!("[SkillConfirmation] Failed to restore focus: {}", e);
        }
        // Small delay to ensure focus is restored before paste
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    if accepted {
        // User confirmed - execute the skill
        log::info!("[SkillConfirmation] User accepted skill: {}", skill_id);

        // First, save the polish result to history (polish always runs in parallel)
        if let Some(polish_result) = &pending.polish_result {
            if let Some(history_id) = pending.history_id {
                use crate::managers::history::HistoryManager;
                use std::sync::Arc;

                if let Some(hm) = app_for_cleanup.try_state::<Arc<HistoryManager>>() {
                    // Get polish prompt info for history: priority override_prompt_id > selected_prompt > first
                    let polish_prompt_id = pending
                        .override_prompt_id
                        .as_deref()
                        .or(settings.post_process_selected_prompt_id.as_deref());

                    let polish_prompt = settings
                        .post_process_prompts
                        .iter()
                        .find(|p| polish_prompt_id == Some(p.id.as_str()))
                        .or_else(|| settings.post_process_prompts.first());

                    if let Some(prompt) = polish_prompt {
                        let _ = hm
                            .update_transcription_post_processing(
                                history_id,
                                polish_result.clone(),
                                prompt.instructions.clone(),
                                prompt.name.clone(),
                                Some(prompt.id.clone()),
                                settings
                                    .resolve_model_for_provider(&settings.post_process_provider_id),
                                None,
                                Some(1),
                            )
                            .await;
                        log::info!(
                            "[SkillConfirmation] Saved polish result to history entry {}",
                            history_id
                        );
                    }
                }
            }
        }

        // Get skill's output_mode to determine how to display result
        let output_mode = settings
            .post_process_prompts
            .iter()
            .find(|p| p.id == skill_id)
            .map(|p| p.output_mode)
            .unwrap_or_default();

        // Execute skill with:
        // - transcription: polished text if available, otherwise raw ASR
        // - secondary_output: raw ASR text when polished is used (for ${streaming_output})
        // - selected_text: the text user selected (for ${select} variable)
        // Determine primary input based on Skill Router's input_source decision
        let skill_input_owned: String;
        let secondary_output: Option<&str>;

        match pending.input_source.as_deref() {
            Some("select") => {
                // Instruction targets selected text — selected text is primary input
                skill_input_owned = pending
                    .selected_text
                    .clone()
                    .unwrap_or_else(|| transcription.clone());
                secondary_output = Some(&transcription);
            }
            Some("extract") => {
                // Speech contains both instruction and content — use extracted portion
                skill_input_owned = pending
                    .extracted_content
                    .clone()
                    .unwrap_or_else(|| transcription.clone());
                secondary_output = Some(&transcription);
            }
            _ => {
                // "output" or unspecified — use polished transcription (current behavior)
                let polished_text = pending
                    .polish_result
                    .clone()
                    .filter(|text| !text.trim().is_empty());
                skill_input_owned = polished_text.unwrap_or_else(|| transcription.clone());
                secondary_output = None;
            }
        };
        let skill_input = skill_input_owned.as_str();

        // Switch overlay to LLM processing state and notify UI about the specific skill
        crate::overlay::show_llm_processing_overlay(&app);
        use tauri::Emitter;
        if let Some(skill_prompt) = settings
            .post_process_prompts
            .iter()
            .find(|p| p.id == skill_id)
        {
            app.emit("post-process-status", format!("{}...", skill_prompt.name))
                .ok();
        }

        let (result, model, prompt_id, _err, _error_message, _token_count, _call_count, _, _, _) =
            crate::actions::post_process::maybe_post_process_transcription(
                &app,
                &settings,
                skill_input,
                secondary_output,
                false,
                Some(skill_id.clone()),
                pending.app_name.clone(),
                pending.window_title.clone(),
                None,
                None,
                pending.history_id,
                true, // skill_mode
                false,
                pending.selected_text.clone(),
                None,
                None, // cursor_context
                true, // skip_smart_routing: skill already identified, execute directly
            )
            .await;

        // Save skill result to history
        if let Some(skill_result) = &result {
            if let Some(history_id) = pending.history_id {
                use crate::managers::history::HistoryManager;
                use std::sync::Arc;

                if let Some(hm) = app_for_cleanup.try_state::<Arc<HistoryManager>>() {
                    let skill_prompt = settings
                        .post_process_prompts
                        .iter()
                        .find(|p| p.id == skill_id);

                    if let Some(prompt) = skill_prompt {
                        let _ = hm
                            .update_transcription_post_processing(
                                history_id,
                                skill_result.clone(),
                                prompt.instructions.clone(),
                                prompt.name.clone(),
                                prompt_id.clone(),
                                model.clone(),
                                None,
                                Some(1),
                            )
                            .await;
                        log::info!(
                            "[SkillConfirmation] Saved skill result to history entry {}",
                            history_id
                        );
                    }
                }
            }
        }

        // Handle result based on output_mode
        if let Some(text) = result {
            match output_mode {
                crate::settings::SkillOutputMode::Chat => {
                    // Chat mode: show Review Window with the result
                    log::info!("[SkillConfirmation] Chat mode - showing Review Window");
                    let source_text = pending
                        .selected_text
                        .unwrap_or_else(|| transcription.clone());
                    // Get skill name for display in Review Window
                    let skill_name = pending.skill_name.clone();
                    crate::review_window::show_review_window(
                        &app_for_cleanup,
                        source_text,
                        text,
                        0, // No change percent for skill results
                        pending.history_id,
                        None,
                        output_mode,
                        skill_name,
                        prompt_id.clone(),
                        model.clone(),
                    );
                }
                crate::settings::SkillOutputMode::Polish => {
                    // Polish mode: paste directly
                    log::info!("[SkillConfirmation] Polish mode - pasting result");
                    crate::clipboard::paste(text, app)?;
                }
                crate::settings::SkillOutputMode::Silent
                | crate::settings::SkillOutputMode::Replace
                | crate::settings::SkillOutputMode::Append
                | crate::settings::SkillOutputMode::Overlay => {
                    // Silent or not-yet-implemented-in-this-branch modes: do nothing visible
                    log::info!("[SkillConfirmation] Mode {:?} - no output", output_mode);
                }
            }
        }
    } else {
        // User rejected skill — fall back to normal polish flow,
        // respecting app review policy (Always → review window, Never → direct paste).
        log::info!("[SkillConfirmation] User rejected skill, falling back to normal polish flow");

        // Determine the final polish text
        let final_text = if let Some(cached_result) = pending.polish_result {
            log::info!(
                "[SkillConfirmation] Using cached polish result (len: {})",
                cached_result.len()
            );
            cached_result
        } else {
            // Fallback: execute default polish (should not happen with parallel requests)
            log::warn!("[SkillConfirmation] No cached polish result, executing default polish");
            let (result, _, _, _, _, _, _, _, _, _) =
                crate::actions::post_process::maybe_post_process_transcription(
                    &app,
                    &settings,
                    &transcription,
                    None,
                    false,
                    pending.override_prompt_id.clone(),
                    pending.app_name.clone(),
                    pending.window_title.clone(),
                    None,
                    None,
                    pending.history_id,
                    false,
                    false,
                    None,
                    None,
                    None, // cursor_context
                    true, // skip_smart_routing: already done in parallel
                )
                .await;
            match result {
                Some(text) => text,
                None => transcription.clone(),
            }
        };

        // Save polish result to history
        if let Some(history_id) = pending.history_id {
            use crate::managers::history::HistoryManager;
            use std::sync::Arc;

            if let Some(hm) = app_for_cleanup.try_state::<Arc<HistoryManager>>() {
                let polish_prompt_id = pending
                    .override_prompt_id
                    .as_deref()
                    .or(settings.post_process_selected_prompt_id.as_deref());
                let polish_prompt = settings
                    .post_process_prompts
                    .iter()
                    .find(|p| polish_prompt_id == Some(p.id.as_str()))
                    .or_else(|| settings.post_process_prompts.first());

                if let Some(prompt) = polish_prompt {
                    let _ = hm
                        .update_transcription_post_processing(
                            history_id,
                            final_text.clone(),
                            prompt.instructions.clone(),
                            prompt.name.clone(),
                            Some(prompt.id.clone()),
                            settings.resolve_model_for_provider(&settings.post_process_provider_id),
                            None,
                            Some(1),
                        )
                        .await;
                }
            }
        }

        // Reuse the policy resolved during the original transcription flow.
        let app_policy = pending
            .app_review_policy
            .unwrap_or(crate::settings::AppReviewPolicy::Never);

        let change_percent = crate::actions::compute_change_percent(&transcription, &final_text);

        let should_review = match app_policy {
            crate::settings::AppReviewPolicy::Never => false,
            crate::settings::AppReviewPolicy::Always => true,
            crate::settings::AppReviewPolicy::Auto => true, // Default: show review
        };

        log::info!(
            "[SkillConfirmation] Reject fallback: app_policy={:?}, change_percent={}, should_review={}",
            app_policy, change_percent, should_review
        );

        if should_review {
            crate::review_window::set_last_active_window(pending.active_window.clone());
            crate::review_window::show_review_window(
                &app_for_cleanup,
                transcription,
                final_text,
                change_percent,
                pending.history_id,
                None,
                crate::settings::SkillOutputMode::Polish,
                None,
                None,
                None,
            );
        } else {
            if let Some(pid) = pending.process_id {
                if let Err(e) = crate::active_window::focus_app_by_pid(pid) {
                    log::warn!(
                        "[SkillConfirmation] Failed to restore focus for direct paste: {}",
                        e
                    );
                } else {
                    tokio::time::sleep(tokio::time::Duration::from_millis(120)).await;
                }
            }
            crate::clipboard::paste(final_text, app)?;
        }
    }

    // Hide overlay and reset tray icon after confirmation is handled
    crate::overlay::hide_recording_overlay(&app_for_cleanup);
    crate::tray::change_tray_icon(&app_for_cleanup, crate::tray::TrayIconState::Idle);

    Ok(())
}

/// Handle user response to ASR online timeout prompt
#[tauri::command]
pub fn respond_asr_timeout(app: AppHandle, action: String) -> Result<(), String> {
    let sender = {
        let state = app.state::<crate::AsrTimeoutResponseSender>();
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        guard.take()
    };
    if let Some(tx) = sender {
        let _ = tx.send(action);
        Ok(())
    } else {
        Err("No pending ASR timeout".to_string())
    }
}

/// Marker state to track if shortcuts have been initialized.
pub struct ShortcutsInitialized;

/// Initialize keyboard shortcuts.
/// On macOS, this should be called after accessibility permissions are granted.
/// This is idempotent - calling it multiple times is safe.
#[tauri::command]
pub fn initialize_shortcuts(app: AppHandle) -> Result<(), String> {
    // Check if already initialized
    if app.try_state::<ShortcutsInitialized>().is_some() {
        log::debug!("Shortcuts already initialized");
        return Ok(());
    }

    // Initialize shortcuts
    crate::shortcut::init_shortcuts(&app);

    // Mark as initialized
    app.manage(ShortcutsInitialized);

    log::info!("Shortcuts initialized successfully");
    Ok(())
}
