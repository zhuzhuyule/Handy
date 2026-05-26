use crate::managers::history::HistoryManager;
use crate::managers::HotwordManager;
use crate::overlay::show_llm_processing_overlay;
use crate::settings::{AppSettings, HotwordScenario, LLMPrompt};
use log::{error, info};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

pub async fn post_process_text_with_prompt(
    app_handle: &AppHandle,
    settings: &AppSettings,
    transcription: &str,
    streaming_transcription: Option<&str>,
    prompt: &LLMPrompt,
    show_overlay: bool,
    app_name: Option<String>,
    window_title: Option<String>,
    history_id: Option<i64>,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
    Option<String>,
) {
    let fallback_provider = match settings.active_post_process_provider() {
        Some(p) => p,
        None => return (None, None, None, false, None),
    };

    let (actual_provider, model) =
        match super::routing::resolve_effective_model(settings, fallback_provider, prompt) {
            Some((p, m)) => (p, m),
            None => return (None, None, Some(prompt.id.clone()), false, None),
        };

    if show_overlay {
        show_llm_processing_overlay(app_handle);
    }

    // Build hotword injection
    let hotword_injection = if settings.post_process_hotword_injection_enabled {
        if let Some(hm) = app_handle.try_state::<Arc<HistoryManager>>() {
            let hotword_manager = HotwordManager::new(hm.db_path.clone());
            let scenario = super::pipeline::detect_scenario(&app_name);
            let effective_scenario = scenario.unwrap_or(HotwordScenario::Work);
            match hotword_manager.build_contextual_injection(
                effective_scenario,
                transcription,
                transcription,
                app_name.as_deref(),
            ) {
                Ok(injection)
                    if !(injection.person_names.is_empty()
                        && injection.product_names.is_empty()
                        && injection.domain_terms.is_empty()
                        && injection.hotwords.is_empty()) =>
                {
                    let total_terms = injection.person_names.len()
                        + injection.product_names.len()
                        + injection.domain_terms.len()
                        + injection.hotwords.len();
                    info!(
                        "[ManualPostProcess] Hotwords injected: scenario={:?}, terms={}",
                        effective_scenario, total_terms
                    );
                    log::debug!(
                        "[ManualPostProcess] Hotword summary:\n{}",
                        HotwordManager::summarize_injection(&injection)
                    );
                    Some(injection)
                }
                Ok(_) => {
                    info!(
                        "[ManualPostProcess] Hotword injection skipped: scenario={:?}, no active matches or entries",
                        effective_scenario
                    );
                    None
                }
                Err(e) => {
                    error!(
                        "[ManualPostProcess] Failed to build hotword injection: {}",
                        e
                    );
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    // Fetch history context
    let history_entries = if settings.post_process_context_enabled {
        if let Some(app) = &app_name {
            if let Some(hm) = app_handle.try_state::<Arc<HistoryManager>>() {
                match hm.get_recent_history_texts_for_app(
                    app,
                    window_title.as_deref(),
                    None,
                    None,
                    settings.post_process_context_limit as usize,
                    history_id,
                ) {
                    Ok(history) => history,
                    Err(e) => {
                        error!("Failed to fetch history for manual context: {}", e);
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    // Resolve convention-based references
    let app_category = app_name
        .as_deref()
        .map(crate::app_category::from_app_name)
        .unwrap_or("Other");
    let resolved_refs = super::reference_resolver::resolve_references(
        prompt.file_path.as_deref(),
        app_name.as_deref(),
        app_category,
    );
    let refs_content = if resolved_refs.count > 0 {
        log::info!(
            "[ManualPostProcess] Injecting {} reference(s): {:?}",
            resolved_refs.count,
            resolved_refs.matched_files,
        );
        Some(resolved_refs.content)
    } else {
        None
    };

    // Use PromptBuilder for unified variable processing
    let built = super::prompt_builder::PromptBuilder::new(prompt, transcription)
        .streaming_transcription(streaming_transcription)
        .app_name(app_name.as_deref())
        .window_title(window_title.as_deref())
        .history_entries(history_entries)
        .hotword_injection(hotword_injection)
        .resolved_references(refs_content)
        .app_language(&settings.app_language)
        .injection_policy(super::prompt_builder::InjectionPolicy::for_post_process(
            settings,
        ))
        .build();

    let cached_model_id = prompt.model_id.as_deref().or(settings
        .selected_prompt_model
        .as_ref()
        .map(|c| c.primary_id.as_str()));

    // Resolve preset parameters
    let presets_config =
        app_handle.try_state::<std::sync::Arc<crate::managers::model_preset::ModelPresetsConfig>>();
    let merged_extra_params = if let Some(config) = presets_config {
        let cached_model = cached_model_id
            .and_then(|id| settings.cached_models.iter().find(|m| m.id == id))
            .or_else(|| {
                settings
                    .cached_models
                    .iter()
                    .find(|m| m.model_id == model && m.provider_id == actual_provider.id)
            });

        let preset_params = crate::managers::model_preset::resolve_preset_params(
            prompt.param_preset.as_deref(),
            cached_model.and_then(|m| m.model_family.as_deref()),
            &model,
            &config,
        );

        if preset_params.is_empty() {
            None
        } else {
            let merged = crate::managers::model_preset::merge_params(
                preset_params,
                cached_model.and_then(|m| m.extra_params.as_ref()),
            );
            Some(merged)
        }
    } else {
        None
    };

    // Honor the configured model chain strategy (serial / staggered / race)
    // when a fallback is wired up on settings.selected_prompt_model.
    let chain: Option<crate::fallback::ModelChain> = if prompt.model_id.is_some() {
        None
    } else {
        settings.selected_prompt_model.clone()
    };

    let mut actual_model_label = model.clone();

    let (result, err, error_message) = if let Some(ref ch) = chain {
        let app = app_handle.clone();
        let s_clone = settings.clone();
        let sys_msgs = built.system_messages.clone();
        let user_msg = built.user_message.clone();
        let extra = merged_extra_params.clone();
        let app_name_c = app_name.clone();
        let window_title_c = window_title.clone();

        let fb = crate::fallback::execute_with_fallback(ch, |cached_id| {
            let app = app.clone();
            let s = s_clone.clone();
            let sys_msgs = sys_msgs.clone();
            let user_msg = user_msg.clone();
            let extra = extra.clone();
            let app_name_c = app_name_c.clone();
            let window_title_c = window_title_c.clone();
            let id_for_err = cached_id.clone();
            async move {
                // Per-call 5s timeout (spec docs/specs/2026-05-26-polish-pipeline-timeout.spec.md).
                match tokio::time::timeout(
                    std::time::Duration::from_secs(super::core::PER_CALL_TIMEOUT_SECS),
                    async move {
                        let (prov, remote_model) =
                            super::routing::resolve_cached_model_to_provider_owned(&s, &cached_id)
                                .ok_or_else(|| format!("Model {} not resolvable", cached_id))?;
                        let (result, err, error_msg, _token_count) =
                            super::core::execute_llm_request_with_messages_silent(
                                &app,
                                &s,
                                &prov,
                                &remote_model,
                                Some(&cached_id),
                                &sys_msgs,
                                user_msg.as_deref(),
                                None,
                                app_name_c,
                                window_title_c,
                                None,
                                None,
                                extra.as_ref(),
                            )
                            .await;
                        if err {
                            Err(error_msg.unwrap_or_else(|| "LLM error".into()))
                        } else {
                            result
                                .map(|text| (text, remote_model))
                                .ok_or_else(|| "Empty LLM response".into())
                        }
                    },
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => {
                        log::warn!(
                            "[ManualPostProcess] model '{}' exceeded {}s timeout",
                            id_for_err,
                            super::core::PER_CALL_TIMEOUT_SECS
                        );
                        Err(format!(
                            "Model {} exceeded {}s timeout",
                            id_for_err,
                            super::core::PER_CALL_TIMEOUT_SECS
                        ))
                    }
                }
            }
        })
        .await;

        if fb.is_fallback {
            log::warn!(
                "[ManualPostProcess] Chain: primary '{}' lost (error: {}), used '{}' (strategy={:?})",
                ch.primary_id,
                fb.primary_error.as_deref().unwrap_or("n/a"),
                fb.actual_model_id,
                ch.strategy,
            );
        }

        match fb.result {
            Ok((text, remote_model)) => {
                actual_model_label = remote_model;
                (Some(text), false, None)
            }
            Err(e) => {
                let _ = app_handle.emit(
                    "overlay-error",
                    serde_json::json!({
                        "code": "llm_error",
                        "message": e.clone(),
                    }),
                );
                (None, true, Some(e))
            }
        }
    } else {
        let (r, e, msg, _tok) = super::core::execute_llm_request_with_messages(
            app_handle,
            settings,
            actual_provider,
            &model,
            cached_model_id,
            &built.system_messages,
            built.user_message.as_deref(),
            None,
            app_name.clone(),
            window_title.clone(),
            None,
            None,
            merged_extra_params.as_ref(),
        )
        .await;
        (r, e, msg)
    };

    if let Some(res) = &result {
        info!(
            "[ManualPostProcess] FinalResult model={} prompt_id={} len={}",
            actual_model_label,
            prompt.id,
            res.chars().count()
        );
    }

    (
        result,
        Some(actual_model_label),
        Some(prompt.id.clone()),
        err,
        error_message,
    )
}
