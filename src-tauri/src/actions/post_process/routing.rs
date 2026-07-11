use crate::managers::prompt::{self, PromptManager};
use crate::settings::{AppSettings, LLMPrompt, PostProcessProvider};
use log::info;
use std::sync::Arc;
use tauri::{AppHandle, Manager};

/// Action determined by the smart routing pre-processor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmartAction {
    /// Text needs no correction — output as-is.
    PassThrough,
    /// Minor corrections needed — delegate to lightweight model + prompt.
    LitePolish,
    /// Complex content — needs full polish pipeline.
    FullPolish,
}

/// Resolve a cached_model_id to an owned (PostProcessProvider, remote_model_id) pair.
///
/// Used by fallback closures that need owned data for `Send + 'static` futures.
pub(super) fn resolve_cached_model_to_provider_owned(
    settings: &AppSettings,
    cached_model_id: &str,
) -> Option<(PostProcessProvider, String)> {
    let cached = settings
        .cached_models
        .iter()
        .find(|m| m.id == cached_model_id)?;
    let provider = settings.post_process_provider(&cached.provider_id)?;
    let model_id = cached.model_id.trim().to_string();
    if model_id.is_empty() {
        return None;
    }
    Some((provider.clone(), model_id))
}

/// Execute smart action routing using the intent model.
/// Returns an IntentDecision, or None on failure (caller should fallback to full polish).
pub(super) async fn execute_smart_action_routing(
    app_handle: &AppHandle,
    settings: &AppSettings,
    fallback_provider: &PostProcessProvider,
    transcription: &str,
    history_id: Option<i64>,
) -> Option<super::IntentDecision> {
    // Resolve intent model
    let default_prompt = settings.post_process_prompts.first()?;
    let (provider, model) =
        resolve_intent_routing_model(settings, fallback_provider, default_prompt)?;

    // Load the smart routing prompt
    let prompt_manager = app_handle.state::<Arc<PromptManager>>();
    let system_prompt = prompt_manager
        .get_prompt(app_handle, "system_smart_routing")
        .unwrap_or_else(|_| {
            "You are a text router. Output JSON: {\"action\": \"pass_through|lite_polish|full_polish\", \"needs_hotword\": true|false}".to_string()
        });

    let start = std::time::Instant::now();

    // Use execute_with_fallback when the intent model chain has a fallback
    let intent_chain = settings.post_process_intent_model.as_ref();
    let has_fallback = intent_chain.and_then(|c| c.fallback_id.as_ref()).is_some();

    // Capture metrics manager for per-participant logging
    let intent_metrics = app_handle
        .try_state::<std::sync::Arc<crate::managers::llm_metrics::LlmMetricsManager>>()
        .map(|m| (*m).clone());
    let intent_history_id = history_id;

    let (response_text, actual_model_id, actual_provider_id) = if has_fallback {
        let chain = intent_chain.unwrap();
        let app = app_handle.clone();
        let s = settings.clone();
        let sys_prompt = system_prompt.clone();
        let text = transcription.to_string();

        let fb_result = crate::fallback::execute_with_fallback(chain, |model_id| {
            let app = app.clone();
            let s = s.clone();
            let sys_prompt = sys_prompt.clone();
            let text = text.clone();
            let metrics = intent_metrics.clone();
            let hist_id = intent_history_id;
            let id_for_err = model_id.clone();
            let s_for_err = s.clone();
            async move {
                // Per-call 5s timeout (spec docs/specs/2026-05-26-polish-pipeline-timeout.spec.md).
                match tokio::time::timeout(
                    std::time::Duration::from_secs(super::core::PER_CALL_TIMEOUT_SECS),
                    async move {
                        let (prov, remote_model) = resolve_cached_model_to_provider_owned(
                            &s, &model_id,
                        )
                        .ok_or_else(|| {
                            format!(
                                "Model {} not found or invalid",
                                super::core::format_model_ident(&s, &model_id)
                            )
                        })?;
                        // Resolve the API model_id and provider_id for logging
                        let log_model_id = s
                            .cached_models
                            .iter()
                            .find(|m| m.id == model_id)
                            .map(|m| m.model_id.clone())
                            .unwrap_or_else(|| model_id.clone());
                        let log_provider_id = prov.id.clone();

                        let call_start = std::time::Instant::now();
                        let (result, err, error_msg, token_count) =
                            super::core::execute_llm_request(
                                &app,
                                &s,
                                &prov,
                                &remote_model,
                                None,
                                &sys_prompt,
                                Some(&text),
                                None,
                                None,
                                None,
                                None,
                            )
                            .await;
                        let elapsed_ms = call_start.elapsed().as_millis() as i64;

                        // Self-log metrics for this participant
                        if let Some(ref m) = metrics {
                            let token_est = token_count.map(|t| t as f64);
                            let speed = match (token_est, elapsed_ms) {
                                (Some(est), dur) if dur > 0 => Some(est / dur as f64 * 1000.0),
                                _ => None,
                            };
                            let err_msg = if err { error_msg.clone() } else { None };
                            let _ = m.log_call(&crate::managers::llm_metrics::LlmCallRecord {
                                history_id: hist_id,
                                model_id: log_model_id,
                                provider: log_provider_id,
                                call_type: "intent".to_string(),
                                input_tokens: None,
                                output_tokens: None,
                                total_tokens: token_count,
                                token_estimate: token_est,
                                duration_ms: elapsed_ms,
                                tokens_per_sec: speed,
                                error: err_msg,
                                is_fallback: false, // closure cannot distinguish primary vs fallback
                            });
                        }

                        if err {
                            Err(error_msg.unwrap_or_else(|| "LLM error".into()))
                        } else {
                            result.ok_or_else(|| "Empty result".into())
                        }
                    },
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => {
                        log::warn!(
                            "[SmartRouting] intent model '{}' exceeded {}s timeout",
                            super::core::format_model_ident(&s_for_err, &id_for_err),
                            super::core::PER_CALL_TIMEOUT_SECS
                        );
                        Err(format!(
                            "Intent model {} exceeded {}s timeout",
                            super::core::format_model_ident(&s_for_err, &id_for_err),
                            super::core::PER_CALL_TIMEOUT_SECS
                        ))
                    }
                }
            }
        })
        .await;

        if fb_result.is_fallback {
            info!(
                "[SmartRouting] Used fallback model '{}' (primary error: {:?})",
                fb_result.actual_model_id, fb_result.primary_error
            );
        }

        let actual_id = fb_result.actual_model_id.clone();
        let prov_id = resolve_cached_model_to_provider_owned(settings, &actual_id)
            .map(|(p, _)| p.id)
            .unwrap_or_else(|| provider.id.clone());

        match fb_result.result {
            Ok(text) => (text, actual_id, prov_id),
            Err(_) => return None,
        }
    } else {
        // No fallback — use the original direct path
        let captured_provider_id = provider.id.clone();
        let captured_model_id = model.clone();

        let call_start = std::time::Instant::now();
        let (result, err, error_msg, token_count) = super::core::execute_llm_request(
            app_handle,
            settings,
            provider,
            &model,
            None,
            &system_prompt,
            Some(transcription),
            None,
            None,
            None,
            None,
        )
        .await;
        let elapsed_ms = call_start.elapsed().as_millis() as i64;

        // Log metrics for direct (non-fallback) intent call
        if let Some(ref m) = intent_metrics {
            let token_est = token_count.map(|t| t as f64);
            let speed = match (token_est, elapsed_ms) {
                (Some(est), dur) if dur > 0 => Some(est / dur as f64 * 1000.0),
                _ => None,
            };
            let err_msg = if err { error_msg.clone() } else { None };
            let _ = m.log_call(&crate::managers::llm_metrics::LlmCallRecord {
                history_id: intent_history_id,
                model_id: captured_model_id.clone(),
                provider: captured_provider_id.clone(),
                call_type: "intent".to_string(),
                input_tokens: None,
                output_tokens: None,
                total_tokens: token_count,
                token_estimate: token_est,
                duration_ms: elapsed_ms,
                tokens_per_sec: speed,
                error: err_msg,
                is_fallback: false,
            });
        }

        match result {
            Some(text) => (text, captured_model_id, captured_provider_id),
            None => return None,
        }
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    // Parse JSON response — try direct parse, then extract JSON from possible markdown wrapper
    let parsed: serde_json::Value = serde_json::from_str(&response_text)
        .or_else(|_| {
            let trimmed = response_text.trim();
            let json_str = trimmed
                .find('{')
                .and_then(|start| trimmed.rfind('}').map(|end| &trimmed[start..=end]))
                .unwrap_or(trimmed);
            serde_json::from_str(json_str)
        })
        .ok()?;

    let action_str = parsed
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("full_polish");

    let needs_hotword = parsed
        .get("needs_hotword")
        .and_then(|v| v.as_bool())
        .unwrap_or(true); // Default to true (safe fallback)

    let language = parsed
        .get("language")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let action = match action_str {
        "pass_through" => SmartAction::PassThrough,
        "lite_polish" => SmartAction::LitePolish,
        _ => SmartAction::FullPolish,
    };

    // token_count not available from fallback path; use None for now
    let token_count: Option<i64> = None;

    info!(
        "[SmartRouting] Action={} needs_hotword={} language={:?} tokens={:?} input_len={}",
        action_str,
        needs_hotword,
        language,
        token_count,
        transcription.chars().count()
    );

    Some(super::IntentDecision {
        action,
        needs_hotword,
        language,
        token_count,
        model_id: actual_model_id,
        provider_id: actual_provider_id,
        duration_ms,
    })
}

/// Build the system prompt for skill routing
pub(super) fn build_skill_routing_prompt(
    template: &str,
    prompts: &[LLMPrompt],
    has_selected_text: bool,
) -> String {
    let mut skill_list = String::new();
    let mut enabled_skill_names: Vec<String> = Vec::new();

    for prompt in prompts {
        // Skip disabled skills
        if !prompt.enabled {
            info!(
                "[SkillRouter] Skipping disabled skill: {} (id={})",
                prompt.name, prompt.id
            );
            continue;
        }

        let description = if prompt.description.is_empty() {
            format!("{}.", prompt.name)
        } else {
            prompt.description.clone()
        };

        skill_list.push_str(&format!(
            "- id: \"{}\", name: \"{}\", description: \"{}\"\n",
            prompt.id, prompt.name, description
        ));

        enabled_skill_names.push(format!("{} ({})", prompt.name, prompt.id));
    }

    info!(
        "[SkillRouter] Building prompt with {} enabled skills: [{}]",
        enabled_skill_names.len(),
        enabled_skill_names.join(", ")
    );

    let selected_text_note = if has_selected_text {
        r#"

## 注意
用户当前**有选中的文本内容**。如果用户的指令是针对选中内容的操作（如"翻译这个"、"总结一下"、"帮我检查"等），应该返回 input_source: "select"。"#
    } else {
        ""
    };

    let mut vars = std::collections::HashMap::new();
    vars.insert("SKILL_LIST", skill_list);
    vars.insert("SELECTED_TEXT_NOTE", selected_text_note.to_string());

    prompt::substitute_variables(template, &vars)
}

/// Parse the skill routing response from LLM
pub(super) fn parse_skill_route_response(content: &str) -> Option<super::SkillRouteResponse> {
    let cleaned = super::core::clean_response_content(content);
    if let Some(json) = super::core::extract_json_block(&cleaned) {
        if let Ok(parsed) = serde_json::from_str::<super::SkillRouteResponse>(&json) {
            return Some(parsed);
        }
    }
    // Try parsing directly
    if let Ok(parsed) = serde_json::from_str::<super::SkillRouteResponse>(&cleaned) {
        return Some(parsed);
    }
    None
}

/// Perform asynchronous skill routing using LLM
/// Returns the full routing response including skill_id, confidence, input_source and extracted_content
/// Result from skill routing that includes token usage
pub(super) struct SkillRoutingResult {
    pub response: super::SkillRouteResponse,
    pub token_count: Option<i64>,
}

pub(super) async fn perform_skill_routing(
    app_handle: &AppHandle,
    prompts: &[LLMPrompt],
    provider: &PostProcessProvider,
    model: &str,
    transcription: &str,
    selected_text: Option<&str>,
) -> Option<SkillRoutingResult> {
    let settings = crate::settings::get_settings(app_handle);

    let has_selected_text = selected_text.map(|s| !s.trim().is_empty()).unwrap_or(false);

    info!(
        "[SkillRouter] perform_skill_routing called with: model={}, transcription_len={}, has_selected_text={}, prompts_count={}",
        model, transcription.len(), has_selected_text, prompts.len()
    );

    let prompt_manager = app_handle.state::<Arc<PromptManager>>();
    let template = match prompt_manager.get_prompt(app_handle, "system_skill_routing") {
        Ok(t) => t,
        Err(e) => {
            log::warn!("[SkillRouter] Failed to load prompt: {}", e);
            return None;
        }
    };
    let routing_prompt = build_skill_routing_prompt(&template, prompts, has_selected_text);

    if crate::DEBUG_LOG_SKILL_ROUTING.load(std::sync::atomic::Ordering::Relaxed) {
        super::core::preview_multiline("SkillRouter.SystemPrompt", &routing_prompt);
        super::core::preview_multiline("SkillRouter.UserMessage", transcription);
    }

    let system_prompts = vec![routing_prompt];
    let (response_text, _is_error, _error_msg, token_count) =
        super::core::execute_llm_request_with_messages(
            app_handle,
            &settings,
            provider,
            model,
            None,
            &system_prompts,
            Some(transcription),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;

    let content = match response_text {
        Some(c) if !c.trim().is_empty() => c,
        _ => {
            log::warn!("[SkillRouter] LLM response has no content");
            return None;
        }
    };

    if crate::DEBUG_LOG_SKILL_ROUTING.load(std::sync::atomic::Ordering::Relaxed) {
        info!("[SkillRouter] Raw LLM response: {}", content);
    }

    let route = match parse_skill_route_response(&content) {
        Some(r) => r,
        None => {
            log::warn!("[SkillRouter] Failed to parse skill route response");
            return None;
        }
    };

    // Apply confidence coefficient adjustment based on context
    // With selected text: boost confidence by 15% (user intent is clearer with context)
    let raw_confidence = route.confidence.unwrap_or(0);
    let adjusted_confidence = if has_selected_text {
        ((raw_confidence as f32) * 1.15).min(100.0) as i32
    } else {
        raw_confidence
    };

    info!(
        "[SkillRouter] Confidence: raw={}, adjusted={} (selected_text={})",
        raw_confidence, adjusted_confidence, has_selected_text
    );

    // Confidence threshold: Route if adjusted confidence is fairly high (≥ 70%)
    // User can reject via confirmation dialog if incorrect
    if route.skill_id == "default" || adjusted_confidence < 70 {
        if route.skill_id != "default" {
            info!(
                "[SkillRouter] Low confidence routing ignored: {} (raw: {}, adjusted: {})",
                route.skill_id, raw_confidence, adjusted_confidence
            );
        } else {
            info!("[SkillRouter] LLM returned 'default' - no skill match");
        }
        None
    } else {
        info!(
            "[SkillRouter] Routed to skill: {} (Confidence: {} -> {}, InputSource: {:?})",
            route.skill_id, raw_confidence, adjusted_confidence, route.input_source
        );
        Some(SkillRoutingResult {
            response: route,
            token_count,
        })
    }
}

/// Result from default polish that includes token usage
#[allow(dead_code)]
pub(super) struct DefaultPolishResult {
    pub text: String,
    pub token_count: Option<i64>,
    pub model_id: String,
    pub provider_id: String,
    pub duration_ms: u64,
    /// When true, metrics were self-logged per participant (fallback chain).
    pub metrics_self_logged: bool,
}

/// Execute default polish request for parallel processing.
/// This is a simplified version that only runs the default prompt.
/// Returns the polished text with token count, or None if failed.
pub(super) async fn execute_default_polish<'a>(
    app_handle: &AppHandle,
    settings: &'a AppSettings,
    fallback_provider: &'a PostProcessProvider,
    default_prompt: &LLMPrompt,
    transcription: &str,
    app_name: Option<String>,
    window_title: Option<String>,
    history_id: Option<i64>,
) -> Option<DefaultPolishResult> {
    // Check for fallback chain (only when no prompt-specific model)
    let polish_chain = if default_prompt.model_id.is_none() {
        settings.selected_prompt_model.as_ref()
    } else {
        None
    };
    let has_fallback = polish_chain.and_then(|c| c.fallback_id.as_ref()).is_some();

    // For non-fallback: resolve model early (fail fast)
    let single_resolved = if !has_fallback {
        match resolve_effective_model(settings, fallback_provider, default_prompt) {
            Some(r) => Some(r),
            None => return None,
        }
    } else {
        None
    };

    let hotword_injection = if settings.post_process_hotword_injection_enabled {
        if let Some(hm) =
            app_handle.try_state::<std::sync::Arc<crate::managers::history::HistoryManager>>()
        {
            let hotword_manager = crate::managers::hotword::HotwordManager::new(hm.db_path.clone());
            let scenario = super::pipeline::detect_scenario(&app_name);
            let effective_scenario = scenario.unwrap_or(crate::settings::HotwordScenario::Work);
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
                        "[ParallelPolish] Hotwords injected: scenario={:?}, terms={}",
                        effective_scenario, total_terms
                    );
                    log::debug!(
                        "[ParallelPolish] Hotword summary:\n{}",
                        crate::managers::hotword::HotwordManager::summarize_injection(&injection)
                    );
                    Some(injection)
                }
                Ok(_) => {
                    info!(
                        "[ParallelPolish] Hotword injection skipped: scenario={:?}, no active matches or entries",
                        effective_scenario
                    );
                    None
                }
                Err(e) => {
                    log::error!("[ParallelPolish] Failed to build hotword injection: {}", e);
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    let history_entries = if settings.post_process_context_enabled {
        if let Some(app) = &app_name {
            if let Some(hm) =
                app_handle.try_state::<std::sync::Arc<crate::managers::history::HistoryManager>>()
            {
                hm.get_recent_history_texts_for_app(
                    app,
                    window_title.as_deref(),
                    None,
                    None,
                    settings.post_process_context_limit as usize,
                    history_id,
                )
                .unwrap_or_default()
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
        default_prompt.file_path.as_deref(),
        app_name.as_deref(),
        app_category,
    );
    let refs_content = if resolved_refs.count > 0 {
        log::info!(
            "[ParallelPolish] Injecting {} reference(s): {:?}",
            resolved_refs.count,
            resolved_refs.matched_files,
        );
        Some(resolved_refs.content)
    } else {
        None
    };

    // Use PromptBuilder for consistent variable processing
    let detected_language =
        super::pipeline::detect_language_heuristic(transcription, &settings.app_language);
    let built = super::prompt_builder::PromptBuilder::new(default_prompt, transcription)
        .app_name(app_name.as_deref())
        .window_title(window_title.as_deref())
        .history_entries(history_entries)
        .hotword_injection(hotword_injection)
        .resolved_references(refs_content)
        .app_language(&settings.app_language)
        .detected_language(Some(&detected_language))
        .injection_policy(super::prompt_builder::InjectionPolicy::for_post_process(
            settings,
        ))
        .build();

    // --- Phase 3: Execute ---
    if has_fallback {
        // Fallback chain: each participant resolves model + presets, executes, and self-logs
        let chain = polish_chain.unwrap();
        let app = app_handle.clone();
        let s = settings.clone();
        let sys_msgs = built.system_messages.clone();
        let user_msg = built.user_message.clone();
        let an = app_name;
        let wt = window_title;
        let metrics = app_handle
            .try_state::<std::sync::Arc<crate::managers::llm_metrics::LlmMetricsManager>>()
            .map(|m| (*m).clone());
        let presets_config = app_handle
            .try_state::<std::sync::Arc<crate::managers::model_preset::ModelPresetsConfig>>()
            .map(|c| (*c).clone());
        let prompt_preset = default_prompt.param_preset.clone();
        let hist_id = history_id;

        let fb_result = crate::fallback::execute_with_fallback(chain, |cached_model_id| {
            let app = app.clone();
            let s = s.clone();
            let sys_msgs = sys_msgs.clone();
            let user_msg = user_msg.clone();
            let an = an.clone();
            let wt = wt.clone();
            let metrics = metrics.clone();
            let presets_config = presets_config.clone();
            let prompt_preset = prompt_preset.clone();
            async move {
                let cached = s
                    .cached_models
                    .iter()
                    .find(|m| m.id == cached_model_id)
                    .ok_or_else(|| {
                        format!(
                            "Model {} not found",
                            super::core::format_model_ident(&s, &cached_model_id)
                        )
                    })?;
                let provider = s
                    .post_process_provider(&cached.provider_id)
                    .ok_or_else(|| {
                        format!(
                            "Provider {} not found",
                            super::core::format_provider_ident(&s, &cached.provider_id)
                        )
                    })?;
                let model = cached.model_id.clone();
                let log_model_id = cached.model_id.clone();
                let log_provider_id = provider.id.clone();

                // Resolve presets for this model
                let merged_extra_params = if let Some(ref config) = presets_config {
                    let preset_params = crate::managers::model_preset::resolve_preset_params(
                        prompt_preset.as_deref(),
                        cached.model_family.as_deref(),
                        &model,
                        config,
                    );
                    if preset_params.is_empty() {
                        None
                    } else {
                        Some(crate::managers::model_preset::merge_params(
                            preset_params,
                            cached.extra_params.as_ref(),
                        ))
                    }
                } else {
                    None
                };

                let call_start = std::time::Instant::now();
                let (result, err, error_msg, token_count) =
                    super::core::execute_llm_request_with_messages(
                        &app,
                        &s,
                        provider,
                        &model,
                        Some(&cached_model_id),
                        &sys_msgs,
                        user_msg.as_deref(),
                        None,
                        an,
                        wt,
                        None,
                        None,
                        merged_extra_params.as_ref(),
                    )
                    .await;
                let elapsed_ms = call_start.elapsed().as_millis() as u64;

                // Self-log metrics for this participant
                if let Some(ref m) = metrics {
                    let tokens_per_sec = match (&result, elapsed_ms) {
                        (Some(ref t), d) if d > 0 => {
                            let est = super::extensions::estimate_tokens(t);
                            Some(est / d as f64 * 1000.0)
                        }
                        _ => None,
                    };
                    let _ = m.log_call(&crate::managers::llm_metrics::LlmCallRecord {
                        history_id: hist_id,
                        model_id: log_model_id.clone(),
                        provider: log_provider_id.clone(),
                        call_type: "single_polish".to_string(),
                        input_tokens: None,
                        output_tokens: None,
                        total_tokens: token_count,
                        token_estimate: None,
                        duration_ms: elapsed_ms as i64,
                        tokens_per_sec,
                        error: if err { error_msg.clone() } else { None },
                        is_fallback: false,
                    });
                }

                if err {
                    Err(error_msg.unwrap_or_else(|| "LLM error".into()))
                } else {
                    result
                        .map(|text| DefaultPolishResult {
                            text,
                            token_count,
                            model_id: log_model_id,
                            provider_id: log_provider_id,
                            duration_ms: elapsed_ms,
                            metrics_self_logged: true,
                        })
                        .ok_or_else(|| "Empty result".into())
                }
            }
        })
        .await;

        if fb_result.is_fallback {
            info!(
                "[DefaultPolish] Used fallback model '{}' (primary error: {:?})",
                fb_result.actual_model_id, fb_result.primary_error
            );
        }

        match fb_result.result {
            Ok(result) => Some(result),
            Err(e) => {
                log::warn!("[DefaultPolish] Fallback chain failed: {}", e);
                None
            }
        }
    } else {
        // --- Single model path (existing behavior) ---
        let (actual_provider, model) = single_resolved.unwrap();

        let cached_model_id = default_prompt.model_id.as_deref().or(settings
            .selected_prompt_model
            .as_ref()
            .map(|c| c.primary_id.as_str()));

        // Resolve preset parameters
        let presets_config = app_handle
            .try_state::<std::sync::Arc<crate::managers::model_preset::ModelPresetsConfig>>();
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
                default_prompt.param_preset.as_deref(),
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

        let polish_start = std::time::Instant::now();
        let (result, _err, _error_message, api_token_count) =
            super::core::execute_llm_request_with_messages(
                app_handle,
                settings,
                actual_provider,
                &model,
                cached_model_id,
                &built.system_messages,
                built.user_message.as_deref(),
                None,
                app_name,
                window_title,
                None,
                None,
                merged_extra_params.as_ref(),
            )
            .await;

        if let Some(ref text) = result {
            info!(
                "[ParallelPolish] Default polish completed, result length: {}",
                text.len()
            );
        }
        let polish_duration = polish_start.elapsed().as_millis() as u64;
        result.map(|text| DefaultPolishResult {
            text,
            token_count: api_token_count,
            model_id: model.clone(),
            provider_id: actual_provider.id.clone(),
            duration_ms: polish_duration,
            metrics_self_logged: false,
        })
    }
}

pub(super) fn resolve_effective_model<'a>(
    settings: &'a AppSettings,
    fallback_provider: &'a PostProcessProvider,
    prompt: &LLMPrompt,
) -> Option<(&'a PostProcessProvider, String)> {
    // Check if a model ID exists in cached_models and its provider is available.
    // `warn_on_miss`: true for primary config entries (real config issues),
    // false for fallback iteration (expected misses during search).
    let check_cached_model = |model_id_opt: Option<&String>,
                              source: &str,
                              warn_on_miss: bool|
     -> Option<(&'a PostProcessProvider, String)> {
        let id_str = model_id_opt.filter(|id| !id.trim().is_empty())?;
        let cached = settings.cached_models.iter().find(|m| m.id == *id_str);
        if cached.is_none() {
            if warn_on_miss {
                log::warn!(
                    "[ResolveModel] {} id={} not found in cached_models",
                    source,
                    id_str
                );
            } else {
                log::debug!(
                    "[ResolveModel] {} id={} not found in cached_models",
                    source,
                    id_str
                );
            }
            return None;
        }
        let cached = cached.unwrap();
        let provider = settings.post_process_provider(&cached.provider_id);
        if provider.is_none() {
            log::warn!(
                "[ResolveModel] {} id={} found model={} but provider={} not found",
                source,
                id_str,
                cached.model_id,
                cached.provider_id
            );
            return None;
        }
        Some((provider.unwrap(), cached.model_id.clone()))
    };

    log::debug!(
        "[ResolveModel] prompt.model_id={:?} selected_prompt_model={:?} fallback_provider={}",
        prompt.model_id,
        settings.selected_prompt_model,
        fallback_provider.id
    );

    // 1. Prompt-specific model
    if let Some(res) = check_cached_model(prompt.model_id.as_ref(), "prompt.model_id", true) {
        if !res.1.trim().is_empty() {
            log::debug!(
                "[ResolveModel] Resolved via prompt.model_id → provider={} model={}",
                res.0.id,
                res.1
            );
            return Some(res);
        }
    }

    // 2. Global selected model
    if let Some(res) = check_cached_model(
        settings
            .selected_prompt_model
            .as_ref()
            .map(|c| &c.primary_id),
        "selected_prompt_model",
        true,
    ) {
        if !res.1.trim().is_empty() {
            log::debug!(
                "[ResolveModel] Resolved via selected_prompt_model → provider={} model={}",
                res.0.id,
                res.1
            );
            return Some(res);
        }
    }

    // 3. Fallback provider's configured model
    if let Some(m) = settings.post_process_models.get(&fallback_provider.id) {
        if !m.trim().is_empty() {
            log::debug!(
                "[ResolveModel] Resolved via fallback_provider → provider={} model={}",
                fallback_provider.id,
                m
            );
            return Some((fallback_provider, m.clone()));
        }
    }

    // 4. Last resort: pick the first valid model from multi-model selection.
    // Uses debug-level logging for misses since iterating through the list is expected.
    let total = settings.multi_model_selected_ids.len();
    let mut skipped = 0usize;
    for id in &settings.multi_model_selected_ids {
        if let Some(res) = check_cached_model(Some(id), "multi_model_fallback", false) {
            if !res.1.trim().is_empty() {
                if skipped > 0 {
                    log::warn!(
                        "[ResolveModel] Primary model config is stale — resolved via multi_model_fallback (skipped {}/{} stale IDs) → provider={} model={}",
                        skipped, total, res.0.id, res.1
                    );
                } else {
                    log::info!(
                        "[ResolveModel] Resolved via multi_model_fallback → provider={} model={}",
                        res.0.id,
                        res.1
                    );
                }
                return Some(res);
            }
        }
        skipped += 1;
    }

    log::warn!(
        "[ResolveModel] No model found! prompt.model_id={:?} selected_prompt_model={:?} fallback_provider={} cached_models_count={} multi_model_ids={}",
        prompt.model_id,
        settings.selected_prompt_model,
        fallback_provider.id,
        settings.cached_models.len(),
        total
    );
    None
}

pub(crate) fn resolve_intent_routing_model<'a>(
    settings: &'a AppSettings,
    fallback_provider: &'a PostProcessProvider,
    fallback_prompt: &LLMPrompt,
) -> Option<(&'a PostProcessProvider, String)> {
    if let Some(intent_model_id) = settings
        .post_process_intent_model
        .as_ref()
        .map(|c| &c.primary_id)
    {
        if let Some(cached_model) = settings
            .cached_models
            .iter()
            .find(|cached| cached.id == *intent_model_id)
        {
            if cached_model.model_type == crate::settings::ModelType::Text {
                if let Some(provider) = settings.post_process_provider(&cached_model.provider_id) {
                    let model_id = cached_model.model_id.trim().to_string();
                    if !model_id.is_empty() {
                        return Some((provider, model_id));
                    }
                } else {
                    log::warn!(
                        "[SkillRouter] Intent model provider not found: {}",
                        cached_model.provider_id
                    );
                }
            } else {
                log::warn!(
                    "[SkillRouter] Intent model is not text-capable: {}",
                    cached_model.id
                );
            }
        } else {
            log::warn!(
                "[SkillRouter] Intent model not found in cache: {}",
                intent_model_id
            );
        }
    }

    let (actual_provider, model) =
        resolve_effective_model(settings, fallback_provider, fallback_prompt)?;
    Some((actual_provider, model))
}

pub(super) fn resolve_prompt_from_text(
    text: &str,
    prompts: &[LLMPrompt],
    default_prompt: Option<&LLMPrompt>,
    override_prompt_id: Option<&str>,
) -> (Option<LLMPrompt>, String, bool) {
    // 1. If override_prompt_id specified, use that prompt
    if let Some(override_id) = override_prompt_id {
        if let Some(p) = prompts.iter().find(|p| p.id == override_id) {
            return (Some(p.clone()), text.to_string(), true);
        }
    }

    // 2. Return default prompt with full original text
    (default_prompt.cloned(), text.to_string(), false)
}

pub(super) fn get_default_prompt<'a>(
    prompts: &'a [LLMPrompt],
    selected_id: Option<&str>,
    override_prompt_id: Option<&str>,
) -> Option<&'a LLMPrompt> {
    if let Some(pid) = override_prompt_id {
        if let Some(p) = prompts.iter().find(|p| p.id == pid) {
            return Some(p);
        }
    }

    if let Some(id) = selected_id {
        if let Some(p) = prompts.iter().find(|p| p.id == id) {
            return Some(p);
        }
    }

    prompts.first()
}

/// Execute the full Smart Routing pipeline (classify → execute) as a single reusable call.
///
/// When Smart Routing is enabled and text is short enough, this runs intent classification
/// first, then dispatches to PassThrough, LitePolish, or FullPolish accordingly.
/// When Smart Routing is disabled or text exceeds the threshold, falls back to FullPolish.
///
/// Has the same parameter signature as `execute_default_polish` for drop-in use.
pub(super) async fn execute_smart_polish<'a>(
    app_handle: &AppHandle,
    settings: &'a AppSettings,
    fallback_provider: &'a PostProcessProvider,
    default_prompt: &LLMPrompt,
    transcription: &str,
    app_name: Option<String>,
    window_title: Option<String>,
    history_id: Option<i64>,
) -> Option<super::SmartPolishResult> {
    let start = std::time::Instant::now();
    let char_count = transcription.chars().count() as u32;
    let smart_routing_enabled =
        settings.length_routing_enabled && settings.post_process_intent_model.is_some();
    let is_short_text = char_count <= settings.length_routing_threshold;

    // If smart routing is enabled and text is short, run classification first
    if smart_routing_enabled && is_short_text {
        let decision = execute_smart_action_routing(
            app_handle,
            settings,
            fallback_provider,
            transcription,
            history_id,
        )
        .await;

        match decision {
            Some(d) => {
                let routing_tokens = d.token_count;
                let _routing_duration = d.duration_ms;

                match d.action {
                    SmartAction::PassThrough => {
                        // Override PassThrough to LitePolish if repetition detected
                        if super::pipeline::has_repetition_pattern(transcription) {
                            info!(
                                "[SmartPolish] PassThrough overridden to LitePolish (repetition detected, {} chars)",
                                char_count
                            );
                            return execute_smart_polish_lite(
                                app_handle,
                                settings,
                                fallback_provider,
                                transcription,
                                d.needs_hotword,
                                &app_name,
                                routing_tokens,
                                start,
                                history_id,
                            )
                            .await;
                        }

                        info!("[SmartPolish] PassThrough ({} chars)", char_count);
                        Some(super::SmartPolishResult {
                            text: transcription.to_string(),
                            action: SmartAction::PassThrough,
                            token_count: routing_tokens,
                            model_id: d.model_id,
                            provider_id: d.provider_id,
                            duration_ms: start.elapsed().as_millis() as u64,
                        })
                    }
                    SmartAction::LitePolish => {
                        info!(
                            "[SmartPolish] LitePolish (needs_hotword={}, {} chars)",
                            d.needs_hotword, char_count
                        );
                        execute_smart_polish_lite(
                            app_handle,
                            settings,
                            fallback_provider,
                            transcription,
                            d.needs_hotword,
                            &app_name,
                            routing_tokens,
                            start,
                            history_id,
                        )
                        .await
                    }
                    SmartAction::FullPolish => {
                        info!(
                            "[SmartPolish] FullPolish via routing ({} chars)",
                            char_count
                        );
                        // Delegate to existing execute_default_polish and wrap
                        let result = execute_default_polish(
                            app_handle,
                            settings,
                            fallback_provider,
                            default_prompt,
                            transcription,
                            app_name,
                            window_title,
                            history_id,
                        )
                        .await?;

                        let combined_tokens = match (routing_tokens, result.token_count) {
                            (Some(a), Some(b)) => Some(a + b),
                            (Some(a), None) => Some(a),
                            (None, Some(b)) => Some(b),
                            (None, None) => None,
                        };

                        Some(super::SmartPolishResult {
                            text: result.text,
                            action: SmartAction::FullPolish,
                            token_count: combined_tokens,
                            model_id: result.model_id,
                            provider_id: result.provider_id,
                            duration_ms: start.elapsed().as_millis() as u64,
                        })
                    }
                }
            }
            None => {
                // Intent analysis failed — fall back to FullPolish
                info!("[SmartPolish] Intent analysis unavailable, falling back to FullPolish");
                let result = execute_default_polish(
                    app_handle,
                    settings,
                    fallback_provider,
                    default_prompt,
                    transcription,
                    app_name,
                    window_title,
                    history_id,
                )
                .await?;

                Some(super::SmartPolishResult {
                    text: result.text,
                    action: SmartAction::FullPolish,
                    token_count: result.token_count,
                    model_id: result.model_id,
                    provider_id: result.provider_id,
                    duration_ms: start.elapsed().as_millis() as u64,
                })
            }
        }
    } else {
        // Smart routing disabled or text too long — go straight to FullPolish
        if !smart_routing_enabled {
            info!("[SmartPolish] Smart routing disabled, using FullPolish");
        } else {
            info!(
                "[SmartPolish] Long text ({} > {}), using FullPolish",
                char_count, settings.length_routing_threshold
            );
        }

        let result = execute_default_polish(
            app_handle,
            settings,
            fallback_provider,
            default_prompt,
            transcription,
            app_name,
            window_title,
            history_id,
        )
        .await?;

        Some(super::SmartPolishResult {
            text: result.text,
            action: SmartAction::FullPolish,
            token_count: result.token_count,
            model_id: result.model_id,
            provider_id: result.provider_id,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }
}

/// Execute the LitePolish path: lightweight model + lite prompt.
async fn execute_smart_polish_lite<'a>(
    app_handle: &AppHandle,
    settings: &'a AppSettings,
    fallback_provider: &'a PostProcessProvider,
    transcription: &str,
    needs_hotword: bool,
    app_name: &Option<String>,
    routing_tokens: Option<i64>,
    start: std::time::Instant,
    history_id: Option<i64>,
) -> Option<super::SmartPolishResult> {
    // Build temporary settings with lightweight model override
    let mut lite_settings = settings.clone();
    if let Some(ref short_model) = settings.length_routing_short_model {
        lite_settings.selected_prompt_model = Some(short_model.clone());
    }
    if !needs_hotword {
        lite_settings.post_process_hotword_injection_enabled = false;
    }

    // Load the lite polish prompt
    let prompt_manager = app_handle.state::<Arc<PromptManager>>();
    let lite_instructions = prompt_manager
        .get_prompt(app_handle, "system_lite_polish")
        .unwrap_or_else(|_| "Fix minor ASR errors. Output corrected text only.".to_string());

    // Build a synthetic LLMPrompt from the first prompt in settings
    let lite_prompt = if let Some(base) = lite_settings.post_process_prompts.first() {
        let mut p = base.clone();
        p.id = "__LITE_POLISH__".to_string();
        p.name = "轻量润色".to_string();
        p.instructions = lite_instructions;
        p
    } else {
        return None;
    };

    // Check for fallback chain on the lite model
    let lite_chain = lite_settings.selected_prompt_model.as_ref();
    let has_fallback = lite_chain.and_then(|c| c.fallback_id.as_ref()).is_some();

    // For non-fallback: resolve model early
    let lite_fallback = match lite_settings.active_post_process_provider() {
        Some(p) => p,
        None => fallback_provider,
    };
    let single_resolved = if !has_fallback {
        match resolve_effective_model(&lite_settings, lite_fallback, &lite_prompt) {
            Some(r) => Some(r),
            None => return None,
        }
    } else {
        None
    };

    // Build hotword injection only if needed
    let hotword_injection = if needs_hotword && lite_settings.post_process_hotword_injection_enabled
    {
        super::pipeline::build_hotword_injection(app_handle, app_name, transcription)
    } else {
        None
    };

    // Build prompt (model-independent)
    let detected_language =
        super::pipeline::detect_language_heuristic(transcription, &lite_settings.app_language);
    let built = super::prompt_builder::PromptBuilder::new(&lite_prompt, transcription)
        .app_name(app_name.as_deref())
        .hotword_injection(hotword_injection)
        .app_language(&lite_settings.app_language)
        .detected_language(Some(&detected_language))
        .injection_policy(super::prompt_builder::InjectionPolicy::for_post_process(
            &lite_settings,
        ))
        .build();

    let (text, api_token_count, result_model, result_provider) = if has_fallback {
        let chain = lite_chain.unwrap();
        let app = app_handle.clone();
        let s = lite_settings.clone();
        let sys_msgs = built.system_messages.clone();
        let user_msg = built.user_message.clone();
        let an = app_name.clone();
        let metrics = app_handle
            .try_state::<std::sync::Arc<crate::managers::llm_metrics::LlmMetricsManager>>()
            .map(|m| (*m).clone());
        let hist_id = history_id;

        let fb_result = crate::fallback::execute_with_fallback(chain, |cached_model_id| {
            let app = app.clone();
            let s = s.clone();
            let sys_msgs = sys_msgs.clone();
            let user_msg = user_msg.clone();
            let an = an.clone();
            let metrics = metrics.clone();
            async move {
                let cached = s
                    .cached_models
                    .iter()
                    .find(|m| m.id == cached_model_id)
                    .ok_or_else(|| {
                        format!(
                            "Model {} not found",
                            super::core::format_model_ident(&s, &cached_model_id)
                        )
                    })?;
                let provider = s
                    .post_process_provider(&cached.provider_id)
                    .ok_or_else(|| {
                        format!(
                            "Provider {} not found",
                            super::core::format_provider_ident(&s, &cached.provider_id)
                        )
                    })?;
                let model = cached.model_id.clone();
                let log_model_id = cached.model_id.clone();
                let log_provider_id = provider.id.clone();

                let call_start = std::time::Instant::now();
                let (result, err, error_msg, token_count) =
                    super::core::execute_llm_request_with_messages(
                        &app,
                        &s,
                        provider,
                        &model,
                        Some(&cached_model_id),
                        &sys_msgs,
                        user_msg.as_deref(),
                        None,
                        an,
                        None,
                        None,
                        None,
                        None,
                    )
                    .await;
                let elapsed_ms = call_start.elapsed().as_millis() as u64;

                // Self-log metrics
                if let Some(ref m) = metrics {
                    let tokens_per_sec = match (&result, elapsed_ms) {
                        (Some(ref t), d) if d > 0 => {
                            let est = super::extensions::estimate_tokens(t);
                            Some(est / d as f64 * 1000.0)
                        }
                        _ => None,
                    };
                    let _ = m.log_call(&crate::managers::llm_metrics::LlmCallRecord {
                        history_id: hist_id,
                        model_id: log_model_id.clone(),
                        provider: log_provider_id.clone(),
                        call_type: "single_polish".to_string(),
                        input_tokens: None,
                        output_tokens: None,
                        total_tokens: token_count,
                        token_estimate: None,
                        duration_ms: elapsed_ms as i64,
                        tokens_per_sec,
                        error: if err { error_msg.clone() } else { None },
                        is_fallback: false,
                    });
                }

                if err {
                    Err(error_msg.unwrap_or_else(|| "LLM error".into()))
                } else {
                    result
                        .map(|text| (text, token_count, log_model_id, log_provider_id))
                        .ok_or_else(|| "Empty result".into())
                }
            }
        })
        .await;

        if fb_result.is_fallback {
            info!(
                "[LitePolish] Used fallback model '{}' (primary error: {:?})",
                fb_result.actual_model_id, fb_result.primary_error
            );
        }

        match fb_result.result {
            Ok(r) => r,
            Err(e) => {
                log::warn!("[LitePolish] Fallback chain failed: {}", e);
                return None;
            }
        }
    } else {
        let (actual_provider, model) = single_resolved.unwrap();
        let cached_model_id = lite_prompt.model_id.as_deref().or(lite_settings
            .selected_prompt_model
            .as_ref()
            .map(|c| c.primary_id.as_str()));

        let (result, _err, _error_message, api_token_count) =
            super::core::execute_llm_request_with_messages(
                app_handle,
                &lite_settings,
                actual_provider,
                &model,
                cached_model_id,
                &built.system_messages,
                built.user_message.as_deref(),
                None,
                app_name.clone(),
                None,
                None,
                None,
                None,
            )
            .await;

        let text = result?;
        (
            text,
            api_token_count,
            model.clone(),
            actual_provider.id.clone(),
        )
    };

    let lite_duration_ms = start.elapsed().as_millis() as u64;

    let combined_tokens = match (routing_tokens, api_token_count) {
        (Some(a), Some(b)) => Some(a + b),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

    info!(
        "[SmartPolish] LitePolish completed: result_len={}, duration={}ms",
        text.len(),
        lite_duration_ms
    );

    Some(super::SmartPolishResult {
        text,
        action: SmartAction::LitePolish,
        token_count: combined_tokens,
        model_id: result_model,
        provider_id: result_provider,
        duration_ms: lite_duration_ms,
    })
}
