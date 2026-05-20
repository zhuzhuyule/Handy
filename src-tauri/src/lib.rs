mod actions;
mod active_window;
mod app_category;
mod app_list;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod apple_intelligence;
mod audio_feedback;
pub mod audio_toolkit;
mod cli;
mod clipboard;
mod commands;
pub mod error;
pub mod fallback;
mod foreground_tracker;
mod helpers;
pub mod http_client;
mod input;
mod key_selector;
mod llm_client;
mod managers;
mod online_asr;
mod openai_api_server;
mod overlay;
pub mod phonetic_similarity;
mod provider_gateway;
mod review_window;
mod settings;
mod shortcut;
mod signal_handle;
#[cfg(test)]
mod test_handy;
pub mod transcription_coordinator;
pub use transcription_coordinator::TranscriptionCoordinator;
mod tray;
mod utils;
mod window_context;

use env_filter::Builder as EnvFilterBuilder;
use managers::audio::AudioRecordingManager;
use managers::history::HistoryManager;
use managers::model::ModelManager;
use managers::summary::SummaryManager;
use managers::transcription::TranscriptionManager;
#[cfg(unix)]
use signal_hook::consts::SIGUSR2;
#[cfg(unix)]
use signal_hook::iterator::Signals;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use tauri::image::Image;

use tauri::tray::TrayIconBuilder;
use tauri::Emitter;
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_log::fern::colors::{Color, ColoredLevelConfig};
use tauri_plugin_log::{Builder as LogBuilder, RotationStrategy, Target, TargetKind};

// Global atomic to store the file log level filter
// We use u8 to store the log::LevelFilter as a number
pub static FILE_LOG_LEVEL: AtomicU8 = AtomicU8::new(log::LevelFilter::Debug as u8);
pub static CONSOLE_LOG_LEVEL: AtomicU8 = AtomicU8::new(log::LevelFilter::Info as u8);

// Debug log channel toggles
pub static DEBUG_LOG_POST_PROCESS: AtomicBool = AtomicBool::new(false);
pub static DEBUG_LOG_SKILL_ROUTING: AtomicBool = AtomicBool::new(false);
pub static DEBUG_LOG_ROUTING: AtomicBool = AtomicBool::new(false);
pub static DEBUG_LOG_TRANSCRIPTION: AtomicBool = AtomicBool::new(false);

fn level_filter_from_u8(value: u8) -> log::LevelFilter {
    match value {
        0 => log::LevelFilter::Off,
        1 => log::LevelFilter::Error,
        2 => log::LevelFilter::Warn,
        3 => log::LevelFilter::Info,
        4 => log::LevelFilter::Debug,
        5 => log::LevelFilter::Trace,
        _ => log::LevelFilter::Trace,
    }
}

fn build_console_filter() -> env_filter::Filter {
    let mut builder = EnvFilterBuilder::new();

    match std::env::var("RUST_LOG") {
        Ok(spec) if !spec.trim().is_empty() => {
            if let Err(err) = builder.try_parse(&spec) {
                log::warn!(
                    "Ignoring invalid RUST_LOG value '{}': {}. Falling back to info-level console logging",
                    spec,
                    err
                );
                builder.filter_level(log::LevelFilter::Info);
            }
        }
        _ => {
            builder.filter_level(log::LevelFilter::Info);
        }
    }

    // Suppress verbose DEBUG logs from HTTP client libraries
    // These libraries log every connection attempt which floods the console
    builder.filter_module("reqwest", log::LevelFilter::Info);
    builder.filter_module("hyper", log::LevelFilter::Info);
    builder.filter_module("hyper_util", log::LevelFilter::Info);
    // Suppress ERROR logs from updater plugin in dev mode (endpoint not available)
    builder.filter_module("tauri_plugin_updater", log::LevelFilter::Warn);

    builder.build()
}

#[derive(Default)]
#[allow(dead_code)]
struct ShortcutToggleStates {
    // Map: shortcut_binding_id -> is_active
    active_toggles: HashMap<String, bool>,
}

#[allow(dead_code)]
type ManagedToggleState = Mutex<ShortcutToggleStates>;

/// State for pending skill confirmation when selected text is present
#[derive(Default, Clone)]
pub struct PendingSkillConfirmation {
    pub skill_id: Option<String>,
    pub skill_name: Option<String>,
    pub transcription: Option<String>,
    pub selected_text: Option<String>,
    pub override_prompt_id: Option<String>,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub history_id: Option<i64>,
    /// Process ID of the original window for focus restoration
    pub process_id: Option<u64>,
    /// Original active window snapshot for review fallback / focus restore
    pub active_window: Option<active_window::ActiveWindowInfo>,
    /// App review policy resolved during the original transcription flow
    pub app_review_policy: Option<settings::AppReviewPolicy>,
    /// Cached polish result from parallel request
    pub polish_result: Option<String>,
    /// Input source from skill routing: "select", "output", or "extract"
    pub input_source: Option<String>,
    /// Extracted content from speech (when input_source is "extract")
    pub extracted_content: Option<String>,
    /// Whether the confirmation UI is visible in the frontend
    pub is_ui_visible: bool,
}

pub type ManagedPendingSkillConfirmation = Mutex<PendingSkillConfirmation>;

/// State for pending ASR timeout response (online-only mode)
pub type AsrTimeoutResponseSender = Mutex<Option<tokio::sync::oneshot::Sender<String>>>;

fn initialize_core_logic(app_handle: &AppHandle) {
    // Initialize the input state (Enigo will be lazily initialized on first use)
    let enigo_state = input::EnigoState::new();
    app_handle.manage(enigo_state);

    // Initialize the managers
    let recording_manager = Arc::new(
        AudioRecordingManager::new(app_handle).expect("Failed to initialize recording manager"),
    );
    let model_manager =
        Arc::new(ModelManager::new(app_handle).expect("Failed to initialize model manager"));
    let transcription_manager = Arc::new(
        TranscriptionManager::new(app_handle, model_manager.clone())
            .expect("Failed to initialize transcription manager"),
    );
    let history_manager =
        Arc::new(HistoryManager::new(app_handle).expect("Failed to initialize history manager"));
    let post_processing_manager = Arc::new(managers::post_processing::PostProcessingManager::new());

    // Initialize summary manager with the same db_path as history
    let app_data_dir = app_handle
        .path()
        .app_data_dir()
        .expect("Failed to get app data dir");
    let db_path = app_data_dir.join("history.db");
    let summary_manager = Arc::new(SummaryManager::new(db_path.clone()));

    // Add managers to Tauri's managed state
    app_handle.manage(recording_manager.clone());
    app_handle.manage(model_manager.clone());
    app_handle.manage(transcription_manager.clone());
    app_handle.manage(history_manager.clone());
    app_handle.manage(post_processing_manager.clone());
    app_handle.manage(summary_manager.clone());
    let llm_metrics_manager = Arc::new(crate::managers::llm_metrics::LlmMetricsManager::new(
        history_manager.db_path.clone(),
    ));
    app_handle.manage(llm_metrics_manager);
    app_handle.manage(std::sync::Arc::new(
        crate::managers::pipeline_log::PipelineLogManager::new(db_path.clone()),
    ));
    let hotword_manager = Arc::new(managers::HotwordManager::new(db_path.clone()));
    app_handle.manage(hotword_manager.clone());
    let prompt_manager = Arc::new(managers::prompt::PromptManager::new(app_handle));
    app_handle.manage(prompt_manager.clone());
    let task_clusters_manager = Arc::new(crate::managers::task_clusters::TaskClustersManager::new(
        db_path.clone(),
    ));
    app_handle.manage(task_clusters_manager.clone());

    // T23: Migrate legacy `summary.stats.task_clusters` JSON into the new
    // `task_clusters` table at startup. Runs in the background so it never
    // blocks app startup, and is idempotent (no-op if the table already has
    // rows for the summary).
    {
        let tc = task_clusters_manager.clone();
        let sm = summary_manager.clone();
        tauri::async_runtime::spawn(async move {
            match tauri::async_runtime::spawn_blocking(move || {
                sm.list_all_summaries_with_legacy_clusters()
            })
            .await
            {
                Ok(Ok(rows)) => {
                    for (summary_id, date, legacy_json) in rows {
                        if let Err(e) =
                            tc.migrate_legacy_stats_clusters(summary_id, &date, &legacy_json)
                        {
                            eprintln!(
                                "legacy cluster migration failed for summary {}: {}",
                                summary_id, e
                            );
                        }
                    }
                }
                Ok(Err(e)) => eprintln!("failed to list legacy summaries: {}", e),
                Err(e) => eprintln!("legacy summary list join error: {}", e),
            }
        });
    }

    let cluster_feedback_manager =
        Arc::new(crate::managers::cluster_feedback::ClusterFeedbackManager::new(db_path.clone()));
    app_handle.manage(cluster_feedback_manager.clone());
    let free_models_cache = Arc::new(crate::managers::free_models::FreeModelsCache::new(
        app_data_dir.clone(),
    ));
    app_handle.manage(free_models_cache.clone());
    let unsupported_params_manager =
        crate::managers::unsupported_params::UnsupportedParamsManager::new(app_data_dir.clone());
    app_handle.manage(unsupported_params_manager);
    app_handle.manage(key_selector::KeySelector::new());
    let presets_config = crate::managers::model_preset::load_model_presets(app_handle)
        .unwrap_or_else(|e| {
            log::warn!("Failed to load model presets: {}. Using empty config.", e);
            crate::managers::model_preset::ModelPresetsConfig {
                version: 0,
                presets: vec![],
                families: vec![],
            }
        });
    log::info!(
        "[ModelPreset] Loaded {} families, {} presets",
        presets_config.families.len(),
        presets_config.presets.len(),
    );
    app_handle.manage(Arc::new(presets_config));

    // Migration: detect model_family for existing cached models that don't have one set
    {
        let presets_cfg =
            app_handle.state::<Arc<crate::managers::model_preset::ModelPresetsConfig>>();
        let mut settings = settings::get_settings(app_handle);
        let mut changed = false;
        for model in settings.cached_models.iter_mut() {
            if model.model_family.is_none() {
                let detected = crate::managers::model_preset::detect_model_family_with_label(
                    &model.model_id,
                    model.custom_label.as_deref(),
                    &presets_cfg,
                );
                if detected.is_some() {
                    log::info!(
                        "Migration: detected model family '{:?}' for existing model '{}'",
                        detected,
                        model.model_id
                    );
                    model.model_family = detected;
                    changed = true;
                }
            }
        }
        if changed {
            settings::write_settings(app_handle, settings);
        }
    }

    // Apply accelerator preferences before any model is loaded
    crate::managers::transcription::apply_accelerator_settings(app_handle);

    app_handle.manage(tray::ManagedTrayIconState(std::sync::Mutex::new(
        tray::TrayIconState::Idle,
    )));
    app_handle.manage(Mutex::new(PendingSkillConfirmation::default()));
    app_handle.manage(AsrTimeoutResponseSender::default());

    // Refresh free models cache in background
    {
        let cache = free_models_cache.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = cache.ensure_loaded().await {
                log::warn!("[FreeModels] Background refresh failed: {}", e);
            }
        });
    }

    // Initialize the TranscriptionCoordinator before shortcuts so shortcut
    // handlers can find it in state when processing transcribe actions.
    let coordinator = transcription_coordinator::TranscriptionCoordinator::new(app_handle.clone());
    app_handle.manage(coordinator);

    // Initialize global shortcuts eagerly so they work even when the main
    // window has not been shown yet (e.g. start_hidden mode).  Global
    // shortcuts do NOT require macOS Accessibility permissions — only Enigo
    // (key simulation for paste) does, and that is initialized separately.
    // The frontend `initialize_shortcuts` call is idempotent and acts as a
    // no-op if shortcuts are already registered.
    shortcut::init_shortcuts(app_handle);
    app_handle.manage(commands::ShortcutsInitialized);

    #[cfg(unix)]
    let signals = Signals::new([SIGUSR2]).unwrap();
    // Set up SIGUSR2 signal handler for toggling transcription
    #[cfg(unix)]
    signal_handle::setup_signal_handler(app_handle.clone(), signals);

    // Apply macOS Accessory policy if starting hidden
    #[cfg(target_os = "macos")]
    {
        let settings = settings::get_settings(app_handle);
        if settings.start_hidden {
            let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Accessory);
        }
    }
    // Get the current theme to set the appropriate initial icon
    let initial_theme = tray::get_current_theme(app_handle);

    // Choose the appropriate initial icon based on theme
    let initial_icon_path = tray::get_icon_path(initial_theme, tray::TrayIconState::Idle);

    let tooltip = tray::version_label();

    let tray = TrayIconBuilder::new()
        .icon(
            Image::from_path(
                app_handle
                    .path()
                    .resolve(initial_icon_path, tauri::path::BaseDirectory::Resource)
                    .unwrap(),
            )
            .unwrap(),
        )
        .tooltip(tooltip)
        .show_menu_on_left_click(true)
        .icon_as_template(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "settings" => {
                let _ = utils::show_or_create_main_window(app, Some("dashboard"));
            }
            "check_updates" => {
                let settings = settings::get_settings(app);
                if settings.update_checks_enabled {
                    let _ = utils::show_or_create_main_window(app, Some("dashboard"));
                    let _ = app.emit("check-for-updates", ());
                }
            }
            "cancel" => {
                use crate::utils::cancel_current_operation;

                // Use centralized cancellation that handles all operations
                cancel_current_operation(app);
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .build(app_handle)
        .unwrap();
    app_handle.manage(tray);

    // Initialize tray menu with idle state
    utils::update_tray_menu(app_handle, &utils::TrayIconState::Idle);

    // Get the autostart manager and configure based on user setting
    let autostart_manager = app_handle.autolaunch();
    let settings = settings::get_settings(app_handle);

    if settings.autostart_enabled {
        // Enable autostart if user has opted in
        let _ = autostart_manager.enable();
    } else {
        // Disable autostart if user has opted out
        let _ = autostart_manager.disable();
    }

    // Create the recording overlay window (hidden by default)
    utils::create_recording_overlay(app_handle);

    // 后台 500ms 轮询，保存最近一次"非 Votype 自身"的 frontmost app。
    // 详情卡片的 Insert 按钮以此为目标。
    // 见 docs/specs/2026-05-20-quick-insert-from-history-detail.spec.md
    foreground_tracker::start();

    openai_api_server::start_openai_api_server(app_handle);

    // Review window is created lazily on first use (saves ~80MB idle memory)
}

fn ensure_runtime_dirs(app_handle: &AppHandle) {
    match app_handle.path().app_data_dir() {
        Ok(app_data_dir) => {
            if let Err(err) = std::fs::create_dir_all(&app_data_dir) {
                eprintln!(
                    "Failed to create app data dir {}: {}",
                    app_data_dir.display(),
                    err
                );
            }
        }
        Err(err) => {
            eprintln!("Failed to resolve app data dir: {}", err);
        }
    }

    match app_handle.path().app_log_dir() {
        Ok(log_dir) => {
            if let Err(err) = std::fs::create_dir_all(&log_dir) {
                eprintln!("Failed to create log dir {}: {}", log_dir.display(), err);
            }
        }
        Err(err) => {
            eprintln!("Failed to resolve log dir: {}", err);
        }
    }
}

#[tauri::command]
fn trigger_update_check(app: AppHandle) -> Result<(), String> {
    let settings = settings::get_settings(&app);
    if !settings.update_checks_enabled {
        return Ok(());
    }
    app.emit("check-for-updates", ())
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Generate TypeScript bindings in debug mode (before app starts)
    #[cfg(debug_assertions)]
    {
        use tauri_specta::{collect_commands, Builder};

        let specta_builder = Builder::<tauri::Wry>::new().commands(collect_commands![
            shortcut::change_binding,
            shortcut::reset_binding,
            shortcut::suspend_binding,
            shortcut::resume_binding,
            shortcut::change_keyboard_implementation_setting,
            shortcut::get_keyboard_implementation,
            shortcut::settings_cmds::change_post_process_enabled_setting,
            shortcut::settings_cmds::change_post_process_context_enabled_setting,
            shortcut::settings_cmds::change_post_process_context_limit_setting,
            shortcut::settings_cmds::change_post_process_streaming_output_enabled_setting,
            shortcut::settings_cmds::change_post_process_hotword_injection_enabled_setting,
            shortcut::settings_cmds::change_openai_compatible_api_enabled_setting,
            shortcut::settings_cmds::change_openai_compatible_api_access_key_setting,
            shortcut::settings_cmds::change_openai_compatible_api_allow_lan_setting,
            shortcut::settings_cmds::change_openai_compatible_api_port_setting,
            shortcut::settings_cmds::change_openai_compatible_api_base_path_setting,
            shortcut::settings_cmds::change_post_process_base_url_setting,
            shortcut::settings_cmds::change_post_process_api_key_setting,
            shortcut::settings_cmds::set_post_process_api_keys,
            shortcut::settings_cmds::get_post_process_api_keys,
            shortcut::settings_cmds::set_proxy_settings,
            shortcut::settings_cmds::set_provider_use_proxy,
            shortcut::settings_cmds::test_proxy_connection,
            shortcut::settings_cmds::change_post_process_model_setting,
            shortcut::settings_cmds::set_post_process_provider,
            shortcut::settings_cmds::toggle_online_asr,
            shortcut::settings_cmds::select_asr_model,
            shortcut::settings_cmds::select_post_process_model,
            shortcut::settings_cmds::set_post_process_selected_prompt,
            shortcut::settings_cmds::upsert_app_profile,
            shortcut::settings_cmds::remove_app_profile,
            shortcut::settings_cmds::assign_app_to_profile,
            shortcut::settings_cmds::set_app_profiles,
            shortcut::settings_cmds::set_app_to_profile,
            shortcut::settings_cmds::change_lazy_stream_close_setting,
            shortcut::settings_cmds::change_mute_while_recording_setting,
            shortcut::settings_cmds::change_audio_input_auto_enhance_setting,
            shortcut::settings_cmds::change_append_trailing_space_setting,
            shortcut::settings_cmds::change_app_language_setting,
            shortcut::settings_cmds::change_show_tray_icon_setting,
            shortcut::settings_cmds::change_experimental_enabled_setting,
            shortcut::settings_cmds::change_autostart_setting,
            shortcut::settings_cmds::change_update_checks_setting,
            shortcut::settings_cmds::change_expert_mode_setting,
            shortcut::settings_cmds::change_onboarding_completed_setting,
            shortcut::settings_cmds::change_word_correction_threshold_setting,
            shortcut::settings_cmds::change_paste_method_setting,
            shortcut::settings_cmds::change_paste_delay_ms_setting,
            shortcut::settings_cmds::change_extra_recording_buffer_setting,
            shortcut::settings_cmds::change_clipboard_handling_setting,
            shortcut::settings_cmds::change_auto_submit_setting,
            shortcut::settings_cmds::change_auto_submit_key_setting,
            shortcut::settings_cmds::change_activation_mode_setting,
            shortcut::settings_cmds::change_audio_feedback_setting,
            shortcut::settings_cmds::change_audio_feedback_volume_setting,
            shortcut::settings_cmds::change_sound_theme_setting,
            shortcut::settings_cmds::change_translate_to_english_setting,
            shortcut::settings_cmds::change_selected_language_setting,
            shortcut::settings_cmds::change_overlay_position_setting,
            shortcut::settings_cmds::change_debug_mode_setting,
            shortcut::settings_cmds::change_debug_log_channel,
            shortcut::settings_cmds::change_start_hidden_setting,
            shortcut::settings_cmds::change_show_overlay_setting,
            shortcut::settings_cmds::add_cached_model,
            shortcut::settings_cmds::update_cached_model_capability,
            shortcut::settings_cmds::change_cached_model_prompt_message_role,
            shortcut::settings_cmds::update_cached_model,
            shortcut::settings_cmds::get_thinking_config,
            shortcut::settings_cmds::toggle_cached_model_thinking,
            shortcut::settings_cmds::remove_cached_model,
            shortcut::settings_cmds::change_favorite_transcription_models_setting,
            shortcut::settings_cmds::change_punctuation_enabled_setting,
            shortcut::settings_cmds::change_punctuation_model_setting,
            shortcut::settings_cmds::change_realtime_transcription_enabled_setting,
            shortcut::settings_cmds::change_offline_vad_force_interval_ms_setting,
            shortcut::settings_cmds::change_offline_vad_force_window_seconds_setting,
            shortcut::settings_cmds::change_post_process_use_local_candidate_when_online_asr_setting,
            shortcut::settings_cmds::change_post_process_secondary_model_id_setting,
            shortcut::settings_cmds::change_post_process_intent_model_id_setting,
            shortcut::settings_cmds::update_model_chain,
            shortcut::settings_cmds::change_length_routing_enabled_setting,
            shortcut::settings_cmds::change_length_routing_threshold_setting,
            shortcut::settings_cmds::change_length_routing_short_model_setting,
            shortcut::settings_cmds::change_length_routing_long_model_setting,
            shortcut::settings_cmds::change_post_process_use_secondary_output_setting,
            shortcut::settings_cmds::get_model_families,
            shortcut::settings_cmds::detect_model_family_cmd,
            shortcut::settings_cmds::get_preset_params,
            shortcut::settings_cmds::get_available_presets,
            shortcut::settings_cmds::update_cached_model_family,
            shortcut::settings_cmds::change_whisper_accelerator_setting,
            shortcut::settings_cmds::change_ort_accelerator_setting,
            shortcut::settings_cmds::change_whisper_gpu_device,
            shortcut::settings_cmds::get_available_accelerators,
            shortcut::multi_model_cmds::toggle_multi_model_selection,
            shortcut::multi_model_cmds::change_multi_model_post_process_enabled_setting,
            shortcut::multi_model_cmds::change_multi_model_strategy_setting,
            shortcut::multi_model_cmds::add_multi_model_post_process_item,
            shortcut::multi_model_cmds::update_multi_model_post_process_item,
            shortcut::multi_model_cmds::remove_multi_model_post_process_item,
            shortcut::multi_model_cmds::set_multi_model_preferred_id,
            shortcut::review_cmds::confirm_reviewed_transcription,
            shortcut::review_cmds::show_review_translation_overlay,
            shortcut::review_cmds::log_from_frontend,
            shortcut::review_cmds::get_review_translation_settings,
            shortcut::review_cmds::cancel_transcription_review,
            shortcut::review_cmds::set_review_editor_active_state,
            shortcut::review_cmds::set_review_editor_content_state,
            shortcut::review_cmds::rerun_single_with_prompt,
            shortcut::review_cmds::get_post_process_prompts,
            shortcut::review_cmds::get_review_model_options,
            shortcut::review_cmds::rerun_multi_model_with_prompt,
            shortcut::provider_cmds::fetch_post_process_models,
            shortcut::provider_cmds::get_provider_avatar_path,
            shortcut::provider_cmds::set_provider_avatar_icon_key,
            shortcut::provider_cmds::set_provider_avatar_from_path,
            shortcut::provider_cmds::set_provider_avatar_from_url,
            shortcut::provider_cmds::reset_provider_avatar,
            shortcut::provider_cmds::refresh_provider_avatar,
            shortcut::provider_cmds::add_custom_provider,
            shortcut::provider_cmds::update_custom_provider,
            shortcut::provider_cmds::remove_custom_provider,
            shortcut::provider_cmds::reorder_post_process_providers,
            shortcut::test_cmds::test_post_process_model_inference,
            shortcut::test_cmds::test_asr_model_inference,
            shortcut::handy_keys::start_handy_keys_recording,
            shortcut::handy_keys::stop_handy_keys_recording,
            shortcut::skills_cmds::get_all_skills,
            shortcut::skills_cmds::create_skill,
            shortcut::skills_cmds::delete_skill,
            shortcut::skills_cmds::get_skill_templates,
            shortcut::skills_cmds::save_external_skill,
            shortcut::skills_cmds::create_skill_from_template,
            shortcut::skills_cmds::reorder_skills,
            shortcut::skills_cmds::get_skills_order,
            shortcut::skills_cmds::get_builtin_skills,
            shortcut::skills_cmds::get_default_skill_content,
            shortcut::skills_cmds::get_external_skills,
            shortcut::skills_cmds::open_skills_folder,
            shortcut::skills_cmds::refresh_external_skills,
            shortcut::skills_cmds::reset_skill_to_file_version,
            shortcut::skills_cmds::open_skill_source_file,
            shortcut::skills_cmds::ai_generate_skill,
            shortcut::skills_cmds::check_skill_id_conflict,
            shortcut::skills_cmds::is_directory_skill,
            shortcut::skills_cmds::get_skill_references,
            shortcut::skills_cmds::save_skill_reference,
            shortcut::skills_cmds::delete_skill_reference,
        ]);

        let bindings_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/bindings.ts");
        if let Err(e) = specta_builder.export(
            specta_typescript::Typescript::default()
                .bigint(specta_typescript::BigIntExportBehavior::Number)
                .header("// Auto-generated by tauri-specta. Do not edit.\n"),
            &bindings_path,
        ) {
            eprintln!("Warning: Failed to export specta bindings: {}", e);
        }
    }

    // Parse console logging directives from RUST_LOG, falling back to info-level logging
    // when the variable is unset
    let console_filter = build_console_filter();

    // Colored log levels for terminal output
    let colors = ColoredLevelConfig::new()
        .error(Color::Red)
        .warn(Color::Yellow)
        .info(Color::Green)
        .debug(Color::Blue)
        .trace(Color::Magenta);

    let mut builder = tauri::Builder::default()
        .device_event_filter(tauri::DeviceEventFilter::Always)
        .plugin(
            LogBuilder::new()
                .level(log::LevelFilter::Trace) // Set to most verbose level globally
                .max_file_size(500_000)
                .rotation_strategy(RotationStrategy::KeepOne)
                .clear_format() // Remove default global format to avoid duplicate metadata
                .clear_targets()
                .targets([
                    // Console output with colored log levels
                    Target::new(TargetKind::Stdout)
                        .filter({
                            let console_filter = console_filter.clone();
                            move |metadata| {
                                // Check RUST_LOG filter first
                                if console_filter.enabled(metadata) {
                                    return true;
                                }
                                // Fallback to dynamic console level
                                let console_level = CONSOLE_LOG_LEVEL.load(Ordering::Relaxed);
                                metadata.level() <= level_filter_from_u8(console_level)
                            }
                        })
                        .format(move |out, message, record| {
                            let now = chrono::Local::now();
                            out.finish(format_args!(
                                "[{}][{}][{}] {}",
                                now.format("%H:%M:%S"),
                                colors.color(record.level()),
                                record.target(),
                                message
                            ))
                        }),
                    // File logs with timestamp (no color)
                    Target::new(TargetKind::LogDir {
                        file_name: Some("votype".into()),
                    })
                    .filter(|metadata| {
                        let file_level = FILE_LOG_LEVEL.load(Ordering::Relaxed);
                        metadata.level() <= level_filter_from_u8(file_level)
                    })
                    .format(|out, message, record| {
                        let now = chrono::Local::now();
                        out.finish(format_args!(
                            "[{}][{}][{}] {}",
                            now.format("%Y-%m-%d %H:%M:%S"),
                            record.level(),
                            record.target(),
                            message
                        ))
                    }),
                ])
                .build(),
        )
        .plugin(tauri_plugin_dialog::init());

    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(tauri_nspanel::init());
    }

    builder
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            cli::handle_cli_args(app, &args);
        }))
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_macos_permissions::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![]),
        ))
        .manage(Mutex::new(ShortcutToggleStates::default()))
        .setup(move |app| {
            ensure_runtime_dirs(app.handle());

            // Parse CLI args for first-launch flags
            let cli_args = cli::CliArgs::try_parse_from_args(&std::env::args().collect::<Vec<_>>());

            let settings = settings::get_settings(app.handle());
            let file_log_level: log::Level = settings.log_level.clone().into();
            // Store the file log level in the atomic for the filter to use
            FILE_LOG_LEVEL.store(file_log_level.to_level_filter() as u8, Ordering::Relaxed);

            // Apply debug mode from CLI or settings
            let debug_mode = cli_args.as_ref().map_or(settings.debug_mode, |a| a.debug || settings.debug_mode);
            let console_level = if debug_mode {
                log::LevelFilter::Debug
            } else {
                log::LevelFilter::Info
            };
            CONSOLE_LOG_LEVEL.store(console_level as u8, Ordering::Relaxed);

            // Sync debug log channel toggles from settings
            DEBUG_LOG_POST_PROCESS.store(settings.debug_log_post_process, Ordering::Relaxed);
            DEBUG_LOG_SKILL_ROUTING.store(settings.debug_log_skill_routing, Ordering::Relaxed);
            DEBUG_LOG_ROUTING.store(settings.debug_log_routing, Ordering::Relaxed);
            DEBUG_LOG_TRANSCRIPTION.store(settings.debug_log_transcription, Ordering::Relaxed);

            let app_handle = app.handle().clone();

            initialize_core_logic(&app_handle);

            // Show main window unless hidden by CLI flag or settings
            let start_hidden = cli_args.as_ref().map_or(settings.start_hidden, |a| a.start_hidden || settings.start_hidden);
            if !start_hidden {
                let _ = utils::show_or_create_main_window(&app_handle, Some("dashboard"));
            }

            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { .. } => {
                // Window close behavior is controlled by settings elsewhere; do not force-hide here.
                #[cfg(target_os = "macos")]
                {
                    let res = window
                        .app_handle()
                        .set_activation_policy(tauri::ActivationPolicy::Accessory);
                    if let Err(e) = res {
                        log::error!("Failed to set activation policy: {}", e);
                    }
                }
            }
            tauri::WindowEvent::ThemeChanged(theme) => {
                log::info!("Theme changed to: {:?}", theme);
                // Update tray icon to match new theme, maintaining idle state
                utils::change_tray_icon(window.app_handle(), utils::TrayIconState::Idle);
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            shortcut::change_binding,
            shortcut::reset_binding,
            shortcut::suspend_binding,
            shortcut::resume_binding,
            shortcut::settings_cmds::change_activation_mode_setting,
            shortcut::settings_cmds::change_audio_feedback_setting,
            shortcut::settings_cmds::change_audio_feedback_volume_setting,
            shortcut::settings_cmds::change_sound_theme_setting,
            shortcut::settings_cmds::change_start_hidden_setting,
            shortcut::settings_cmds::change_autostart_setting,
            shortcut::settings_cmds::change_update_checks_setting,
            shortcut::settings_cmds::change_expert_mode_setting,
            shortcut::settings_cmds::change_onboarding_completed_setting,
            shortcut::settings_cmds::change_translate_to_english_setting,
            shortcut::settings_cmds::change_selected_language_setting,
            shortcut::settings_cmds::change_app_language_setting,
            shortcut::settings_cmds::change_overlay_position_setting,
            shortcut::settings_cmds::change_debug_mode_setting,
            shortcut::settings_cmds::change_debug_log_channel,
            shortcut::settings_cmds::change_word_correction_threshold_setting,
            shortcut::settings_cmds::change_paste_method_setting,
            shortcut::settings_cmds::change_paste_delay_ms_setting,
            shortcut::settings_cmds::change_extra_recording_buffer_setting,
            shortcut::settings_cmds::change_clipboard_handling_setting,
            shortcut::settings_cmds::change_auto_submit_setting,
            shortcut::settings_cmds::change_auto_submit_key_setting,
            shortcut::settings_cmds::change_lazy_stream_close_setting,
            shortcut::settings_cmds::change_mute_while_recording_setting,
            shortcut::settings_cmds::change_audio_input_auto_enhance_setting,
            shortcut::settings_cmds::change_append_trailing_space_setting,
            shortcut::settings_cmds::change_show_tray_icon_setting,
            shortcut::settings_cmds::change_show_overlay_setting,
            shortcut::settings_cmds::change_experimental_enabled_setting,
            shortcut::change_keyboard_implementation_setting,
            shortcut::get_keyboard_implementation,
            shortcut::settings_cmds::change_post_process_enabled_setting,
            shortcut::settings_cmds::change_post_process_use_secondary_output_setting,
            shortcut::settings_cmds::change_post_process_use_local_candidate_when_online_asr_setting,
            shortcut::settings_cmds::change_post_process_secondary_model_id_setting,
            shortcut::multi_model_cmds::toggle_multi_model_selection,
            shortcut::review_cmds::get_post_process_prompts,
            shortcut::review_cmds::get_review_model_options,
            shortcut::review_cmds::rerun_single_with_prompt,
            shortcut::review_cmds::rerun_multi_model_with_prompt,
            shortcut::multi_model_cmds::change_multi_model_post_process_enabled_setting,
            shortcut::multi_model_cmds::change_multi_model_strategy_setting,
            shortcut::multi_model_cmds::add_multi_model_post_process_item,
            shortcut::multi_model_cmds::update_multi_model_post_process_item,
            shortcut::multi_model_cmds::remove_multi_model_post_process_item,
            shortcut::multi_model_cmds::set_multi_model_preferred_id,
            shortcut::settings_cmds::change_post_process_intent_model_id_setting,
            shortcut::settings_cmds::update_model_chain,
            shortcut::settings_cmds::change_length_routing_enabled_setting,
            shortcut::settings_cmds::change_length_routing_threshold_setting,
            shortcut::settings_cmds::change_length_routing_short_model_setting,
            shortcut::settings_cmds::change_length_routing_long_model_setting,
            shortcut::settings_cmds::change_post_process_base_url_setting,
            shortcut::settings_cmds::change_post_process_api_key_setting,
            shortcut::settings_cmds::set_post_process_api_keys,
            shortcut::settings_cmds::get_post_process_api_keys,
            shortcut::settings_cmds::set_proxy_settings,
            shortcut::settings_cmds::set_provider_use_proxy,
            shortcut::settings_cmds::test_proxy_connection,
            shortcut::settings_cmds::change_post_process_model_setting,
            shortcut::settings_cmds::change_post_process_context_enabled_setting,
            shortcut::settings_cmds::change_post_process_context_limit_setting,
            shortcut::settings_cmds::change_post_process_streaming_output_enabled_setting,
            shortcut::settings_cmds::change_post_process_hotword_injection_enabled_setting,
            shortcut::settings_cmds::change_openai_compatible_api_enabled_setting,
            shortcut::settings_cmds::change_openai_compatible_api_access_key_setting,
            shortcut::settings_cmds::change_openai_compatible_api_allow_lan_setting,
            shortcut::settings_cmds::change_openai_compatible_api_port_setting,
            shortcut::settings_cmds::change_openai_compatible_api_base_path_setting,
            shortcut::settings_cmds::set_post_process_provider,
            shortcut::provider_cmds::fetch_post_process_models,
            shortcut::provider_cmds::get_provider_avatar_path,
            shortcut::provider_cmds::set_provider_avatar_icon_key,
            shortcut::provider_cmds::set_provider_avatar_from_path,
            shortcut::provider_cmds::set_provider_avatar_from_url,
            shortcut::provider_cmds::reset_provider_avatar,
            shortcut::provider_cmds::refresh_provider_avatar,
            shortcut::provider_cmds::add_custom_provider,
            shortcut::provider_cmds::update_custom_provider,
            shortcut::provider_cmds::remove_custom_provider,
            shortcut::provider_cmds::reorder_post_process_providers,
            shortcut::settings_cmds::add_cached_model,
            shortcut::settings_cmds::update_cached_model_capability,
            shortcut::settings_cmds::change_cached_model_prompt_message_role,
            shortcut::settings_cmds::update_cached_model,
            shortcut::settings_cmds::update_cached_model_family,
            shortcut::settings_cmds::change_whisper_accelerator_setting,
            shortcut::settings_cmds::change_ort_accelerator_setting,
            shortcut::settings_cmds::change_whisper_gpu_device,
            shortcut::settings_cmds::get_available_accelerators,
            shortcut::settings_cmds::get_model_families,
            shortcut::settings_cmds::detect_model_family_cmd,
            shortcut::settings_cmds::get_preset_params,
            shortcut::settings_cmds::get_available_presets,
            shortcut::settings_cmds::get_thinking_config,
            shortcut::settings_cmds::toggle_cached_model_thinking,
            shortcut::settings_cmds::remove_cached_model,
            shortcut::settings_cmds::toggle_online_asr,
            shortcut::settings_cmds::select_asr_model,
            shortcut::settings_cmds::select_post_process_model,
            shortcut::settings_cmds::set_post_process_selected_prompt,
            shortcut::skills_cmds::get_all_skills,
            shortcut::skills_cmds::get_builtin_skills,
            shortcut::skills_cmds::get_external_skills,
            shortcut::skills_cmds::get_skill_templates,
            shortcut::skills_cmds::get_default_skill_content,
            shortcut::skills_cmds::create_skill,
            shortcut::skills_cmds::create_skill_from_template,
            shortcut::skills_cmds::delete_skill,
            shortcut::skills_cmds::save_external_skill,
            shortcut::skills_cmds::reorder_skills,
            shortcut::skills_cmds::get_skills_order,
            shortcut::skills_cmds::refresh_external_skills,
            shortcut::skills_cmds::open_skills_folder,
            shortcut::skills_cmds::reset_skill_to_file_version,
            shortcut::skills_cmds::open_skill_source_file,
            shortcut::skills_cmds::ai_generate_skill,
            shortcut::skills_cmds::check_skill_id_conflict,
            shortcut::skills_cmds::is_directory_skill,
            shortcut::skills_cmds::get_skill_references,
            shortcut::skills_cmds::save_skill_reference,
            shortcut::skills_cmds::delete_skill_reference,
            shortcut::settings_cmds::change_favorite_transcription_models_setting,
            shortcut::settings_cmds::change_punctuation_enabled_setting,
            shortcut::settings_cmds::change_punctuation_model_setting,
            shortcut::settings_cmds::change_realtime_transcription_enabled_setting,
            shortcut::settings_cmds::change_offline_vad_force_interval_ms_setting,
            shortcut::settings_cmds::change_offline_vad_force_window_seconds_setting,
            shortcut::settings_cmds::upsert_app_profile,
            shortcut::settings_cmds::remove_app_profile,
            shortcut::settings_cmds::assign_app_to_profile,
            shortcut::settings_cmds::set_app_profiles,
            shortcut::settings_cmds::set_app_to_profile,
            shortcut::review_cmds::confirm_reviewed_transcription,
            shortcut::review_cmds::show_review_translation_overlay,
            shortcut::review_cmds::log_from_frontend,
            shortcut::review_cmds::get_review_translation_settings,
            shortcut::review_cmds::cancel_transcription_review,
            shortcut::review_cmds::set_review_editor_active_state,
            shortcut::review_cmds::set_review_editor_content_state,
            shortcut::test_cmds::test_post_process_model_inference,
            shortcut::test_cmds::test_asr_model_inference,
            shortcut::handy_keys::start_handy_keys_recording,
            shortcut::handy_keys::stop_handy_keys_recording,
            review_window::review_window_ready,
            review_window::review_window_content_ready,
            review_window::resize_review_window,
            trigger_update_check,
            commands::get_app_settings,
            commands::cancel_operation,
            commands::get_app_dir_path,
            commands::get_log_dir_path,
            commands::set_log_level,
            commands::open_recordings_folder,
            commands::get_recordings_folder_path,
            commands::open_log_dir,
            commands::open_app_data_dir,
            commands::get_active_window_info,
            commands::list_installed_apps,
            commands::get_cursor_position,
            commands::show_main_window,
            commands::get_first_history_entry,
            commands::paste_text_to_active_window,
            commands::paste_to_previous_window,
            commands::log_to_console,
            commands::focus_overlay,
            commands::confirm_skill,
            commands::respond_asr_timeout,
            commands::initialize_shortcuts,
            commands::models::get_available_models,
            commands::models::get_model_info,
            commands::models::download_model,
            commands::models::add_model_from_url,
            commands::models::remove_custom_model,
            commands::models::delete_model,
            commands::models::cancel_download,
            commands::models::set_active_model,
            commands::models::get_current_model,
            commands::models::get_transcription_model_status,
            commands::models::is_model_loading,
            commands::models::has_any_models_available,
            commands::models::has_any_models_or_downloads,
            commands::models::get_recommended_first_model,
            commands::audio::update_microphone_mode,
            commands::audio::get_microphone_mode,
            commands::audio::get_available_microphones,
            commands::audio::set_selected_microphone,
            commands::audio::get_selected_microphone,
            commands::audio::get_available_output_devices,
            commands::audio::set_selected_output_device,
            commands::audio::get_selected_output_device,
            commands::audio::play_test_sound,
            commands::audio::check_custom_sounds,
            commands::audio::set_clamshell_microphone,
            commands::audio::get_clamshell_microphone,
            commands::audio::is_recording,
            commands::transcription::set_model_unload_timeout,
            commands::transcription::get_model_load_status,
            commands::transcription::unload_model_manually,
            commands::history::get_history_entries,
            commands::history::get_history_entries_paginated,
            commands::history::get_history_entries_by_ids,
            commands::history::get_history_dashboard_stats,
            commands::history::toggle_history_entry_saved,
            commands::history::get_audio_file_path,
            commands::history::get_audio_path_by_history_id,
            commands::history::delete_history_entry,
            commands::history::update_history_limit,
            commands::history::update_recording_retention_period,
            commands::history::retranscribe_history_entry,
            commands::history::reprocess_history_entry,
            commands::history::update_history_entry_text,
            commands::history::reject_post_process_result,
            commands::history::cascade_reject_post_process,
            commands::history::get_model_speed_stats,
            commands::history::get_llm_usage_stats,
            commands::hotword::get_hotwords,
            commands::hotword::add_hotword,
            commands::hotword::update_hotword,
            commands::hotword::delete_hotword,
            commands::hotword::infer_hotword_category,
            commands::hotword::adjust_hotword_use_count,
            commands::hotword::increment_hotword_false_positive,
            commands::hotword::get_hotword_suggestions,
            commands::hotword::accept_hotword_suggestion,
            commands::hotword::dismiss_hotword_suggestion,
            commands::hotword::accept_all_hotword_suggestions,
            commands::hotword::dismiss_all_hotword_suggestions,
            commands::hotword::get_hotword_categories,
            commands::hotword::add_hotword_category,
            commands::hotword::update_hotword_category,
            commands::hotword::delete_hotword_category,
            commands::summary::get_summary_stats,
            commands::summary::get_or_create_summary,
            commands::summary::get_summary_list,
            commands::summary::get_user_profile,
            commands::summary::update_feedback_style,
            commands::summary::update_style_prompt,
            commands::summary::delete_summary_ai_history_entry,
            commands::summary::generate_summary_ai_analysis,
            commands::summary::export_summary,
            commands::task_clusters::get_task_clusters_by_date,
            commands::task_clusters::generate_task_clusters,
            commands::task_clusters::update_task_cluster_field,
            commands::task_clusters::split_task_cluster,
            commands::task_clusters::merge_task_clusters,
            commands::task_clusters::delete_task_cluster,
            commands::cluster_feedback::add_cluster_feedback,
            commands::cluster_feedback::list_cluster_feedback,
            commands::cluster_feedback::list_recent_negative_cluster_feedback,
            commands::cluster_feedback::delete_cluster_feedback,
            commands::free_models::get_free_models,
            commands::free_models::refresh_free_models_cache,
            commands::text::optimize_text_with_llm,
            commands::text::generate_skill_description,
            commands::text::generate_skill_metadata,
            commands::text::translate_review_text,
            commands::text::translate_text_to_english_command,
            helpers::clamshell::is_clamshell,
            helpers::clamshell::is_laptop,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
