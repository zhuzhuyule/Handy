use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;

use crate::settings;
use crate::tray::{self, ManagedTrayIconState, TrayIconState};

// Group: Post-processing Settings
#[tauri::command]
#[specta::specta]
pub fn change_post_process_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.post_process_enabled = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_post_process_context_enabled_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.post_process_context_enabled = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_post_process_context_limit_setting(app: AppHandle, limit: u8) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.post_process_context_limit = limit;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_post_process_streaming_output_enabled_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.post_process_streaming_output_enabled = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_post_process_hotword_injection_enabled_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.post_process_hotword_injection_enabled = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_openai_compatible_api_enabled_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.openai_compatible_api_enabled = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_openai_compatible_api_access_key_setting(
    app: AppHandle,
    access_key: String,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.openai_compatible_api_access_key = access_key.trim().to_string();
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_openai_compatible_api_allow_lan_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.openai_compatible_api_allow_lan = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_openai_compatible_api_port_setting(app: AppHandle, port: u16) -> Result<(), String> {
    if port == 0 {
        return Err("Port must be greater than 0".to_string());
    }

    let mut settings = settings::get_settings(&app);
    settings.openai_compatible_api_port = port;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_openai_compatible_api_base_path_setting(
    app: AppHandle,
    base_path: String,
) -> Result<(), String> {
    let normalized = base_path.trim();
    if normalized.is_empty() {
        return Err("Base path cannot be empty".to_string());
    }

    let mut settings = settings::get_settings(&app);
    settings.openai_compatible_api_base_path = normalized.to_string();
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_post_process_base_url_setting(
    app: AppHandle,
    provider_id: String,
    base_url: String,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    super::validate_provider_exists(&settings, &provider_id)?;
    if let Some(provider) = settings.post_process_provider_mut(&provider_id) {
        provider.base_url = crate::utils::normalize_base_url(&base_url);
        settings::write_settings(&app, settings);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_post_process_api_key_setting(
    app: AppHandle,
    provider_id: String,
    api_key: String,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    super::validate_provider_exists(&settings, &provider_id)?;
    settings.post_process_api_keys.insert(
        provider_id.clone(),
        vec![crate::settings::KeyEntry {
            key: api_key,
            enabled: true,
            label: None,
        }],
    );
    settings::write_settings(&app, settings);
    if let Some(selector) = app.try_state::<crate::key_selector::KeySelector>() {
        selector.reset(&provider_id);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_post_process_model_setting(
    app: AppHandle,
    provider_id: String,
    model: String,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    super::validate_provider_exists(&settings, &provider_id)?;
    settings.post_process_models.insert(provider_id, model);
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn set_post_process_provider(app: AppHandle, provider_id: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    super::validate_provider_exists(&settings, &provider_id)?;
    settings.post_process_provider_id = provider_id;
    settings::write_settings(&app, settings);
    Ok(())
}

// Group: ASR Settings
#[tauri::command]
#[specta::specta]
pub fn toggle_online_asr(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.online_asr_enabled = enabled;
    settings::write_settings(&app, settings);
    if enabled {
        if let Some(tm) =
            app.try_state::<std::sync::Arc<crate::managers::transcription::TranscriptionManager>>()
        {
            let _ = tm.unload_model();
        }
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn select_asr_model(app: AppHandle, model_id: Option<String>) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.selected_asr_model = match model_id {
        Some(id) => {
            // Preserve existing fallback_id and strategy when switching primary
            let existing = settings.selected_asr_model.as_ref();
            Some(crate::fallback::ModelChain {
                primary_id: id,
                fallback_id: existing.and_then(|c| c.fallback_id.clone()),
                strategy: existing.map(|c| c.strategy.clone()).unwrap_or_default(),
            })
        }
        None => None,
    };
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn select_post_process_model(app: AppHandle, model_id: Option<String>) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.selected_prompt_model = match model_id {
        Some(id) => {
            let existing = settings.selected_prompt_model.as_ref();
            Some(crate::fallback::ModelChain {
                primary_id: id,
                fallback_id: existing.and_then(|c| c.fallback_id.clone()),
                strategy: existing.map(|c| c.strategy.clone()).unwrap_or_default(),
            })
        }
        None => None,
    };
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn set_post_process_selected_prompt(app: AppHandle, id: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.post_process_selected_prompt_id = Some(id);
    settings::write_settings(&app, settings);
    Ok(())
}

// Group: App Profiles
#[tauri::command]
#[specta::specta]
pub fn upsert_app_profile(app: AppHandle, profile: settings::AppProfile) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    if let Some(existing) = settings
        .app_profiles
        .iter_mut()
        .find(|p| p.id == profile.id)
    {
        *existing = profile;
    } else {
        settings.app_profiles.push(profile);
    }
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn remove_app_profile(app: AppHandle, id: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.app_profiles.retain(|p| p.id != id);
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn assign_app_to_profile(
    app: AppHandle,
    app_id: String,
    profile_id: Option<String>,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    if let Some(pid) = profile_id {
        settings.app_to_profile.insert(app_id, pid);
    } else {
        settings.app_to_profile.remove(&app_id);
    }
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn set_app_profiles(app: AppHandle, profiles: Vec<settings::AppProfile>) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.app_profiles = profiles;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn respond_rule_suggestion(
    app: AppHandle,
    decision: crate::managers::suggestion_engine::SuggestionDecision,
    app_name: String,
    title: String,
    prompt_id: String,
    threshold: i64,
) -> Result<(), String> {
    use crate::managers::suggestion_engine::{
        apply_accepted_suggestion, mark_decision_applied, record_decision, SuggestionDecision,
    };

    // Idempotent: window-close beforeunload + button click can both fire.
    if !mark_decision_applied() {
        log::info!(
            "[SuggestionEngine] respond_rule_suggestion: decision already applied, skipping"
        );
        return Ok(());
    }

    log::info!(
        "[SuggestionEngine] respond decision={:?} app={} title={}",
        decision,
        app_name,
        title
    );

    if matches!(decision, SuggestionDecision::Accepted) {
        let mut s = settings::get_settings(&app);
        apply_accepted_suggestion(&mut s, &app_name, &title, &prompt_id);
        settings::write_settings(&app, s);

        // Notify any open windows so their settings UI re-fetches and shows
        // the freshly-added TitleRule without a manual reload.
        use tauri::Emitter;
        if let Err(e) = app.emit("app-profiles-updated", ()) {
            log::warn!(
                "[SuggestionEngine] failed to emit app-profiles-updated: {}",
                e
            );
        }
    }

    let hm = app
        .try_state::<std::sync::Arc<crate::managers::history::HistoryManager>>()
        .ok_or_else(|| "HistoryManager not initialized".to_string())?;
    let conn = rusqlite::Connection::open(&hm.db_path).map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))
        .map_err(|e| e.to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    record_decision(&conn, &app_name, &title, threshold, decision, now)
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn set_app_to_profile(
    app: AppHandle,
    app_to_profile: std::collections::HashMap<String, String>,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.app_to_profile = app_to_profile;
    settings::write_settings(&app, settings);
    Ok(())
}

// Group: Other Settings
#[tauri::command]
#[specta::specta]
pub fn change_mute_while_recording_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.mute_while_recording = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_audio_input_auto_enhance_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.audio_input_auto_enhance = enabled;

    // Bind the preference to the currently selected microphone.
    let mic_key = settings
        .selected_microphone
        .clone()
        .unwrap_or_else(|| "default".to_string());
    settings.mic_enhance_preferences.insert(mic_key, enabled);

    settings::write_settings(&app, settings);

    if let Some(audio_manager) =
        app.try_state::<std::sync::Arc<crate::managers::audio::AudioRecordingManager>>()
    {
        audio_manager.set_auto_enhance_enabled(enabled);
    }

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_append_trailing_space_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.append_trailing_space = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_app_language_setting(app: AppHandle, language: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.app_language = language;
    settings::write_settings(&app, settings);
    let current_state = app
        .state::<ManagedTrayIconState>()
        .inner()
        .0
        .lock()
        .map(|s| s.clone())
        .unwrap_or(TrayIconState::Idle);
    tray::update_tray_menu(&app, &current_state);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_show_tray_icon_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.show_tray_icon = enabled;
    settings::write_settings(&app, settings);
    tray::set_tray_visibility(&app, enabled);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_experimental_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.experimental_enabled = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_autostart_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.autostart_enabled = enabled;
    settings::write_settings(&app, settings);
    let autostart_manager = app.autolaunch();
    if enabled {
        let _ = autostart_manager.enable();
    } else {
        let _ = autostart_manager.disable();
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_update_checks_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.update_checks_enabled = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_expert_mode_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.expert_mode = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_onboarding_completed_setting(app: AppHandle, completed: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.onboarding_completed = completed;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_word_correction_threshold_setting(
    app: AppHandle,
    threshold: f64,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.word_correction_threshold = threshold;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_paste_method_setting(app: AppHandle, method: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let parsed = match method.as_str() {
        "ctrl_v" => settings::PasteMethod::CtrlV,
        "direct" => settings::PasteMethod::Direct,
        "none" => settings::PasteMethod::None,
        "shift_insert" => settings::PasteMethod::ShiftInsert,
        "ctrl_shift_v" => settings::PasteMethod::CtrlShiftV,
        "external_script" => settings::PasteMethod::ExternalScript,
        _ => settings::PasteMethod::CtrlV,
    };
    settings.paste_method = parsed;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_paste_delay_ms_setting(app: AppHandle, delay_ms: u64) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.paste_delay_ms = delay_ms;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_extra_recording_buffer_setting(app: AppHandle, buffer_ms: u64) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.extra_recording_buffer_ms = buffer_ms;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_clipboard_handling_setting(app: AppHandle, handling: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let parsed = match handling.as_str() {
        "dont_modify" => settings::ClipboardHandling::DontModify,
        "copy_to_clipboard" => settings::ClipboardHandling::CopyToClipboard,
        _ => settings::ClipboardHandling::DontModify,
    };
    settings.clipboard_handling = parsed;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_auto_submit_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.auto_submit = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_auto_submit_key_setting(app: AppHandle, key: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let parsed = match key.as_str() {
        "enter" => settings::AutoSubmitKey::Enter,
        "ctrl_enter" => settings::AutoSubmitKey::CtrlEnter,
        "cmd_enter" => settings::AutoSubmitKey::CmdEnter,
        _ => settings::AutoSubmitKey::Enter,
    };
    settings.auto_submit_key = parsed;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn add_cached_model(app: AppHandle, mut model: settings::CachedModel) -> Result<(), String> {
    let presets_config =
        app.try_state::<std::sync::Arc<crate::managers::model_preset::ModelPresetsConfig>>();
    if let Some(config) = presets_config {
        if model.model_family.is_none() {
            model.model_family = crate::managers::model_preset::detect_model_family_with_label(
                &model.model_id,
                model.custom_label.as_deref(),
                &config,
            );
            if let Some(ref family) = model.model_family {
                log::info!(
                    "Auto-detected model family '{}' for model '{}'",
                    family,
                    model.model_id
                );
            }
        }
    }
    let mut settings = settings::get_settings(&app);
    if !settings.cached_models.iter().any(|m| m.id == model.id) {
        settings.cached_models.push(model);
        settings::write_settings(&app, settings);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn update_cached_model_capability(
    app: AppHandle,
    id: String,
    model_type: settings::ModelType,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    if let Some(m) = settings.cached_models.iter_mut().find(|m| m.id == id) {
        m.model_type = model_type;
        settings::write_settings(&app, settings);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_cached_model_prompt_message_role(
    app: AppHandle,
    id: String,
    role: String,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let parsed = match role.as_str() {
        "developer" => settings::PromptMessageRole::Developer,
        _ => settings::PromptMessageRole::System,
    };
    if let Some(m) = settings.cached_models.iter_mut().find(|m| m.id == id) {
        m.prompt_message_role = parsed;
        settings::write_settings(&app, settings);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn update_cached_model(
    app: AppHandle,
    id: String,
    custom_label: Option<String>,
    extra_params: Option<String>,
    extra_headers: Option<String>,
    is_thinking_model: Option<bool>,
    prompt_message_role: Option<String>,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    if let Some(m) = settings.cached_models.iter_mut().find(|m| m.id == id) {
        if let Some(label) = custom_label {
            m.custom_label = if label.trim().is_empty() {
                None
            } else {
                Some(label.trim().to_string())
            };
        }
        if let Some(params_str) = extra_params {
            if params_str.trim().is_empty() {
                m.extra_params = None;
            } else {
                match serde_json::from_str::<std::collections::HashMap<String, serde_json::Value>>(
                    &params_str,
                ) {
                    Ok(params) => {
                        m.extra_params = if params.is_empty() {
                            None
                        } else {
                            Some(params)
                        };
                    }
                    Err(e) => return Err(format!("Invalid extra params JSON: {}", e)),
                }
            }
        }
        if let Some(headers_str) = extra_headers {
            if headers_str.trim().is_empty() {
                m.extra_headers = None;
            } else {
                match serde_json::from_str::<std::collections::HashMap<String, String>>(
                    &headers_str,
                ) {
                    Ok(hdrs) => {
                        m.extra_headers = if hdrs.is_empty() { None } else { Some(hdrs) };
                    }
                    Err(e) => return Err(format!("Invalid extra headers JSON: {}", e)),
                }
            }
        }
        if let Some(thinking) = is_thinking_model {
            m.is_thinking_model = thinking;
        }
        if let Some(role) = prompt_message_role {
            m.prompt_message_role = match role.as_str() {
                "developer" => settings::PromptMessageRole::Developer,
                _ => settings::PromptMessageRole::System,
            };
        }
        // Re-detect model_family whenever custom_label changes, unless user has manually set it
        let presets_config =
            app.try_state::<std::sync::Arc<crate::managers::model_preset::ModelPresetsConfig>>();
        if let Some(config) = presets_config {
            let detected = crate::managers::model_preset::detect_model_family_with_label(
                &m.model_id,
                m.custom_label.as_deref(),
                &config,
            );
            if detected != m.model_family {
                log::info!(
                    "[ModelPreset] Re-detected family for '{}' (label={:?}): {:?} → {:?}",
                    m.model_id,
                    m.custom_label,
                    m.model_family,
                    detected
                );
                m.model_family = detected;
            }
        }
        settings::write_settings(&app, settings);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn get_thinking_config(
    model_id: String,
    provider_id: String,
    enabled: bool,
    model_name: Option<String>,
    custom_label: Option<String>,
    model_family: Option<String>,
) -> Option<String> {
    let aliases: Vec<&str> = [
        model_name.as_deref(),
        custom_label.as_deref(),
        model_family.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect();
    settings::thinking_extra_params_with_aliases(&model_id, &provider_id, enabled, &aliases)
        .and_then(|params| serde_json::to_string_pretty(&params).ok())
}

#[tauri::command]
#[specta::specta]
pub fn toggle_cached_model_thinking(
    app: AppHandle,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    if let Some(m) = settings.cached_models.iter_mut().find(|m| m.id == id) {
        m.is_thinking_model = enabled;
        let aliases: Vec<&str> = [m.name.as_str(), m.custom_label.as_deref().unwrap_or("")]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        if let Some(params) = settings::thinking_extra_params_with_aliases(
            &m.model_id,
            &m.provider_id,
            enabled,
            &aliases,
        ) {
            let extra = m
                .extra_params
                .get_or_insert_with(std::collections::HashMap::new);
            for (k, v) in params {
                extra.insert(k, v);
            }
        }
        settings::write_settings(&app, settings);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn remove_cached_model(app: AppHandle, id: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.cached_models.retain(|m| m.id != id);
    settings::cleanup_stale_model_references(&mut settings);
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_show_overlay_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    if !enabled {
        settings.overlay_position = settings::OverlayPosition::None;
    } else if matches!(settings.overlay_position, settings::OverlayPosition::None) {
        settings.overlay_position = settings::OverlayPosition::Bottom;
    }
    settings::write_settings(&app, settings);
    crate::utils::update_overlay_position(&app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_favorite_transcription_models_setting(
    app: AppHandle,
    models: Vec<String>,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.favorite_transcription_models = models;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_punctuation_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.punctuation_enabled = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_punctuation_model_setting(app: AppHandle, model: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.punctuation_model = model;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_realtime_transcription_enabled_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.realtime_transcription_enabled = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_offline_vad_force_interval_ms_setting(
    app: AppHandle,
    interval: u64,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.offline_vad_force_interval_ms = interval;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_offline_vad_force_window_seconds_setting(
    app: AppHandle,
    window: u64,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.offline_vad_force_window_seconds = window;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_post_process_use_local_candidate_when_online_asr_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.post_process_use_local_candidate_when_online_asr = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_post_process_secondary_model_id_setting(
    app: AppHandle,
    model_id: Option<String>,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.post_process_secondary_model_id = model_id;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_post_process_intent_model_id_setting(
    app: AppHandle,
    model_id: Option<String>,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.post_process_intent_model = match model_id {
        Some(id) => {
            let existing = settings.post_process_intent_model.as_ref();
            Some(crate::fallback::ModelChain {
                primary_id: id,
                fallback_id: existing.and_then(|c| c.fallback_id.clone()),
                strategy: existing.map(|c| c.strategy.clone()).unwrap_or_default(),
            })
        }
        None => None,
    };
    settings::write_settings(&app, settings);
    Ok(())
}

// Group: Length Routing Settings
#[tauri::command]
#[specta::specta]
pub fn change_length_routing_enabled_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.length_routing_enabled = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_length_routing_threshold_setting(
    app: AppHandle,
    threshold: u32,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.length_routing_threshold = threshold;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_length_routing_short_model_setting(
    app: AppHandle,
    model_id: Option<String>,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.length_routing_short_model = match model_id {
        Some(id) => {
            let existing = settings.length_routing_short_model.as_ref();
            Some(crate::fallback::ModelChain {
                primary_id: id,
                fallback_id: existing.and_then(|c| c.fallback_id.clone()),
                strategy: existing.map(|c| c.strategy.clone()).unwrap_or_default(),
            })
        }
        None => None,
    };
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_length_routing_long_model_setting(
    app: AppHandle,
    model_id: Option<String>,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.length_routing_long_model = match model_id {
        Some(id) => {
            let existing = settings.length_routing_long_model.as_ref();
            Some(crate::fallback::ModelChain {
                primary_id: id,
                fallback_id: existing.and_then(|c| c.fallback_id.clone()),
                strategy: existing.map(|c| c.strategy.clone()).unwrap_or_default(),
            })
        }
        None => None,
    };
    settings::write_settings(&app, settings);
    Ok(())
}

// Group: Model Chain Settings
#[tauri::command]
#[specta::specta]
pub fn update_model_chain(
    app: AppHandle,
    field: String,
    chain: Option<crate::fallback::ModelChain>,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    match field.as_str() {
        "selected_asr_model" => settings.selected_asr_model = chain,
        "selected_prompt_model" => settings.selected_prompt_model = chain,
        "selected_clustering_model" => settings.selected_clustering_model = chain,
        "post_process_intent_model" => settings.post_process_intent_model = chain,
        "post_process_translation_model" => settings.post_process_translation_model = chain,
        "length_routing_short_model" => settings.length_routing_short_model = chain,
        "length_routing_long_model" => settings.length_routing_long_model = chain,
        _ => return Err(format!("Unknown model chain field: {}", field)),
    }
    settings::write_settings(&app, settings);
    Ok(())
}

// Group: Bluetooth / Stream Settings
#[tauri::command]
#[specta::specta]
pub fn change_lazy_stream_close_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.lazy_stream_close = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

// Group: Activation Mode and Audio Settings
#[tauri::command]
#[specta::specta]
pub fn change_activation_mode_setting(app: AppHandle, mode: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.activation_mode = match mode.as_str() {
        "hold" => settings::ActivationMode::Hold,
        "hold_or_toggle" => settings::ActivationMode::HoldOrToggle,
        _ => settings::ActivationMode::Toggle,
    };
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_audio_feedback_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.audio_feedback = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_audio_feedback_volume_setting(app: AppHandle, volume: f32) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.audio_feedback_volume = volume;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_sound_theme_setting(app: AppHandle, theme: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let parsed = match theme.as_str() {
        "marimba" => settings::SoundTheme::Marimba,
        "pop" => settings::SoundTheme::Pop,
        "custom" => settings::SoundTheme::Custom,
        _ => settings::SoundTheme::Marimba,
    };
    settings.sound_theme = parsed;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_translate_to_english_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.translate_to_english = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_selected_language_setting(app: AppHandle, language: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.selected_language = language;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_overlay_position_setting(app: AppHandle, position: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let parsed = match position.as_str() {
        "bottom" => settings::OverlayPosition::Bottom,
        "top" => settings::OverlayPosition::Top,
        "none" => settings::OverlayPosition::None,
        _ => settings::OverlayPosition::Bottom,
    };
    settings.overlay_position = parsed;
    settings::write_settings(&app, settings);
    crate::utils::update_overlay_position(&app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_debug_mode_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.debug_mode = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_debug_log_channel(
    app: AppHandle,
    channel: String,
    enabled: bool,
) -> Result<(), String> {
    use crate::{
        DEBUG_LOG_POST_PROCESS, DEBUG_LOG_ROUTING, DEBUG_LOG_SKILL_ROUTING, DEBUG_LOG_TRANSCRIPTION,
    };
    use std::sync::atomic::Ordering;

    let mut settings = settings::get_settings(&app);
    match channel.as_str() {
        "post_process" => {
            settings.debug_log_post_process = enabled;
            DEBUG_LOG_POST_PROCESS.store(enabled, Ordering::Relaxed);
        }
        "skill_routing" => {
            settings.debug_log_skill_routing = enabled;
            DEBUG_LOG_SKILL_ROUTING.store(enabled, Ordering::Relaxed);
        }
        "routing" => {
            settings.debug_log_routing = enabled;
            DEBUG_LOG_ROUTING.store(enabled, Ordering::Relaxed);
        }
        "transcription" => {
            settings.debug_log_transcription = enabled;
            DEBUG_LOG_TRANSCRIPTION.store(enabled, Ordering::Relaxed);
        }
        _ => return Err(format!("Unknown debug log channel: {}", channel)),
    }
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_start_hidden_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.start_hidden = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_post_process_use_secondary_output_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.post_process_use_secondary_output = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

// Group: Model Preset Commands
#[tauri::command]
#[specta::specta]
pub fn get_model_families(app: AppHandle) -> Result<Vec<(String, String)>, String> {
    let config = app
        .try_state::<std::sync::Arc<crate::managers::model_preset::ModelPresetsConfig>>()
        .ok_or("Model presets not loaded".to_string())?;
    Ok(config.family_options())
}

#[tauri::command]
#[specta::specta]
pub fn detect_model_family_cmd(
    app: AppHandle,
    model_id: String,
    custom_label: Option<String>,
) -> Option<String> {
    let config =
        app.try_state::<std::sync::Arc<crate::managers::model_preset::ModelPresetsConfig>>()?;
    crate::managers::model_preset::detect_model_family_with_label(
        &model_id,
        custom_label.as_deref(),
        &config,
    )
}

#[tauri::command]
#[specta::specta]
pub fn get_preset_params(
    app: AppHandle,
    family_id: String,
    preset_name: String,
) -> Result<std::collections::HashMap<String, serde_json::Value>, String> {
    let config = app
        .try_state::<std::sync::Arc<crate::managers::model_preset::ModelPresetsConfig>>()
        .ok_or("Model presets not loaded".to_string())?;
    let family = config
        .find_family(&family_id)
        .ok_or(format!("Family '{}' not found", family_id))?;
    Ok(family
        .presets
        .get(&preset_name)
        .cloned()
        .unwrap_or_default())
}

#[tauri::command]
#[specta::specta]
pub fn get_available_presets(app: AppHandle) -> Result<Vec<String>, String> {
    let config = app
        .try_state::<std::sync::Arc<crate::managers::model_preset::ModelPresetsConfig>>()
        .ok_or("Model presets not loaded".to_string())?;
    Ok(config.presets.clone())
}

#[tauri::command]
#[specta::specta]
pub fn update_cached_model_family(
    app: AppHandle,
    id: String,
    model_family: Option<String>,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    if let Some(m) = settings.cached_models.iter_mut().find(|m| m.id == id) {
        m.model_family = model_family.filter(|s| !s.trim().is_empty());
        settings::write_settings(&app, settings);
    }
    Ok(())
}

// Group: Accelerator / GPU Settings

/// Save accelerator settings, re-apply globals, and unload the model so it
/// reloads with the new backend on next transcription.
fn apply_and_reload_accelerator(app: &AppHandle, s: settings::AppSettings) {
    settings::write_settings(app, s);
    crate::managers::transcription::apply_accelerator_settings(app);

    let tm = app.state::<std::sync::Arc<crate::managers::transcription::TranscriptionManager>>();
    if tm.is_model_loaded() {
        if let Err(e) = tm.unload_model() {
            log::warn!("Failed to unload model after accelerator change: {e}");
        }
    }
}

#[tauri::command]
#[specta::specta]
pub fn change_whisper_accelerator_setting(
    app: AppHandle,
    accelerator: settings::WhisperAcceleratorSetting,
) -> Result<(), String> {
    let mut s = settings::get_settings(&app);
    s.whisper_accelerator = accelerator;
    apply_and_reload_accelerator(&app, s);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_ort_accelerator_setting(
    app: AppHandle,
    accelerator: settings::OrtAcceleratorSetting,
) -> Result<(), String> {
    let mut s = settings::get_settings(&app);
    s.ort_accelerator = accelerator;
    apply_and_reload_accelerator(&app, s);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_whisper_gpu_device(app: AppHandle, device: i32) -> Result<(), String> {
    let mut s = settings::get_settings(&app);
    s.whisper_gpu_device = device;
    apply_and_reload_accelerator(&app, s);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn get_available_accelerators() -> crate::managers::transcription::AvailableAccelerators {
    crate::managers::transcription::get_available_accelerators()
}

// Group: Multi-Key Management

#[tauri::command]
#[specta::specta]
pub fn set_post_process_api_keys(
    app: AppHandle,
    provider_id: String,
    keys: Vec<crate::settings::KeyEntry>,
) -> Result<(), String> {
    let mut settings = crate::settings::get_settings(&app);
    super::validate_provider_exists(&settings, &provider_id)?;
    settings
        .post_process_api_keys
        .insert(provider_id.clone(), keys);
    crate::settings::write_settings(&app, settings);
    if let Some(selector) = app.try_state::<crate::key_selector::KeySelector>() {
        selector.reset(&provider_id);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn get_post_process_api_keys(
    app: AppHandle,
    provider_id: String,
) -> Result<Vec<crate::settings::KeyEntry>, String> {
    let settings = crate::settings::get_settings(&app);
    Ok(settings
        .post_process_api_keys
        .get(&provider_id)
        .cloned()
        .unwrap_or_default())
}

// Group: Proxy Settings

#[tauri::command]
#[specta::specta]
pub fn set_proxy_settings(
    app: AppHandle,
    url: Option<String>,
    global_enabled: bool,
) -> Result<(), String> {
    let mut settings = crate::settings::get_settings(&app);
    settings.proxy_url = url;
    settings.proxy_global_enabled = global_enabled;
    crate::settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn set_provider_use_proxy(
    app: AppHandle,
    provider_id: String,
    use_proxy: bool,
) -> Result<(), String> {
    let mut settings = crate::settings::get_settings(&app);
    if let Some(provider) = settings
        .post_process_providers
        .iter_mut()
        .find(|p| p.id == provider_id)
    {
        provider.use_proxy = use_proxy;
        crate::settings::write_settings(&app, settings);
        Ok(())
    } else {
        Err(format!("Provider not found: {}", provider_id))
    }
}

#[tauri::command]
#[specta::specta]
pub async fn test_proxy_connection(proxy_url: String) -> Result<(), String> {
    let client = crate::http_client::build_http_client(
        Some(&proxy_url),
        std::time::Duration::from_secs(10),
        reqwest::header::HeaderMap::new(),
    )?;
    client
        .get("https://www.google.com/generate_204")
        .send()
        .await
        .map_err(|e| format!("{}", e))?;
    Ok(())
}
