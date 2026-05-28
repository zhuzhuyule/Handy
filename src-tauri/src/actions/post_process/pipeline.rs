use crate::managers::history::HistoryManager;
use crate::managers::HotwordManager;
use crate::overlay::show_llm_processing_overlay;
use crate::settings::{AppSettings, HotwordScenario, LLMPrompt, PostProcessProvider};
use log::{error, info};
use std::borrow::Cow;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

/// Unified post-processing entry point.
///
/// Implements the 4-step routing pipeline:
///   1. History exact match (short-circuit if hit)
///   2. Intent analysis (pass_through / lite_polish / full_polish + needs_hotword)
///   3. Model selection (single vs multi-model, prompt selection)
///   4. Execute polish
///
/// Returns `PipelineResult` — caller handles all UI (review window, paste, history).
/// Pipeline timeout (per spec docs/specs/2026-05-26-polish-pipeline-timeout.spec.md).
/// On exceed, the whole pipeline returns ASR original text as PassThrough.
const PIPELINE_TIMEOUT_SECS: u64 = 10;

pub async fn unified_post_process(
    app_handle: &AppHandle,
    settings: &AppSettings,
    transcription: &str,
    streaming_transcription: Option<&str>,
    show_overlay: bool,
    override_prompt_id: Option<String>,
    app_name: Option<String>,
    window_title: Option<String>,
    match_pattern: Option<String>,
    match_type: Option<crate::settings::TitleMatchType>,
    history_id: Option<i64>,
    skill_mode: bool,
    review_editor_active: bool,
    selected_text: Option<String>,
    review_document_text: Option<String>,
    cursor_context: Option<crate::clipboard::CursorContext>,
) -> super::PipelineResult {
    let original_text = transcription.to_string();
    // Retain a copy for the ceiling-timeout log row below; the inner future
    // consumes the original.
    let app_name_for_timeout_log = app_name.clone();
    match tokio::time::timeout(
        std::time::Duration::from_secs(PIPELINE_TIMEOUT_SECS),
        unified_post_process_inner(
            app_handle,
            settings,
            transcription,
            streaming_transcription,
            show_overlay,
            override_prompt_id,
            app_name,
            window_title,
            match_pattern,
            match_type,
            history_id,
            skill_mode,
            review_editor_active,
            selected_text,
            review_document_text,
            cursor_context,
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            log::warn!(
                "[UnifiedPipeline] exceeded {}s ceiling — falling back to PassThrough",
                PIPELINE_TIMEOUT_SECS
            );
            // The inner future is dropped here so it never logs its own
            // pipeline_decisions row. Write a minimal one so the History UI
            // can surface "pipeline timed out" instead of going silent.
            if let Some(manager) =
                app_handle.try_state::<Arc<crate::managers::pipeline_log::PipelineLogManager>>()
            {
                let record = crate::managers::pipeline_log::PipelineDecisionRecord {
                    history_id,
                    input_length: original_text.chars().count() as u32,
                    app_name: app_name_for_timeout_log,
                    smart_routing_enabled: settings.smart_routing_enabled,
                    result_type: "PipelineTimeout".to_string(),
                    total_elapsed_ms: PIPELINE_TIMEOUT_SECS * 1000,
                    error_type: Some("pipeline_timeout".to_string()),
                    error_detail: Some(format!(
                        "Pipeline exceeded {}s ceiling",
                        PIPELINE_TIMEOUT_SECS
                    )),
                    ..Default::default()
                };
                manager.log_decision(&record);
            }
            super::PipelineResult::PassThrough {
                text: original_text,
                intent_token_count: None,
            }
        }
    }
}

async fn unified_post_process_inner(
    app_handle: &AppHandle,
    settings: &AppSettings,
    transcription: &str,
    streaming_transcription: Option<&str>,
    show_overlay: bool,
    override_prompt_id: Option<String>,
    app_name: Option<String>,
    window_title: Option<String>,
    match_pattern: Option<String>,
    match_type: Option<crate::settings::TitleMatchType>,
    history_id: Option<i64>,
    skill_mode: bool,
    review_editor_active: bool,
    selected_text: Option<String>,
    review_document_text: Option<String>,
    cursor_context: Option<crate::clipboard::CursorContext>,
) -> super::PipelineResult {
    let log_routing = crate::DEBUG_LOG_ROUTING.load(std::sync::atomic::Ordering::Relaxed);

    // --- Gate: post-processing disabled ---
    if !settings.post_process_enabled {
        return super::PipelineResult::Skipped;
    }
    if override_prompt_id.as_deref() == Some("__SKIP_POST_PROCESS__") {
        info!("[UnifiedPipeline] Skipping post-processing due to app rule override");
        return super::PipelineResult::Skipped;
    }

    let char_count = transcription.chars().count() as u32;
    let smart_routing_enabled =
        settings.length_routing_enabled && settings.post_process_intent_model.is_some();
    let is_short_text = char_count <= settings.length_routing_threshold;

    // Show LLM processing overlay as soon as post-processing begins,
    // so the user sees "在润色中" during intent analysis and model selection,
    // not just during the final LLM call.
    if show_overlay {
        show_llm_processing_overlay(app_handle);
    }

    let pipeline_start = std::time::Instant::now();
    let mut decision = crate::managers::pipeline_log::PipelineDecisionRecord {
        input_length: char_count,
        app_name: app_name.clone(),
        smart_routing_enabled,
        history_id,
        ..Default::default()
    };
    decision.has_cursor_context = cursor_context.is_some();

    // ═══════════════════════════════════════════════════════════════
    // Step 1 + 2: Smart Routing (only for short text with smart mode on)
    // Skip smart routing for ReviewRewrite mode (voice instruction on selected text)
    // ═══════════════════════════════════════════════════════════════
    let is_rewrite_mode = review_document_text.is_some();
    let has_selected_text_raw = selected_text
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let mut intent_decision: Option<super::IntentDecision> = None;

    // Skip smart routing when:
    // - skill_mode: dedicated skill hotkey pressed, need skill routing not smart routing
    // - has_selected_text_raw: speech may be an instruction targeting selected text,
    //   need intent detection (Mode C) which smart routing would preempt
    if smart_routing_enabled
        && is_short_text
        && !is_rewrite_mode
        && !skill_mode
        && !has_selected_text_raw
    {
        // Step 1: History exact match
        if let Some(hm) = app_handle.try_state::<Arc<HistoryManager>>() {
            match hm.find_cached_post_process_result(transcription) {
                Ok(Some((cached_text, cached_model, cached_prompt_id))) => {
                    if log_routing {
                        info!(
                            "[UnifiedPipeline] Step 1: HistoryHit (len={})",
                            cached_text.chars().count()
                        );
                    }
                    if let Some(ref an) = app_name {
                        super::recent_context::push(&cached_text, an);
                    }
                    decision.history_hit = true;
                    decision.result_type = "Cached".to_string();
                    decision.total_elapsed_ms = pipeline_start.elapsed().as_millis() as u64;
                    log_pipeline_decision(app_handle, &decision);
                    return super::PipelineResult::Cached {
                        text: cached_text,
                        model: cached_model,
                        prompt_id: cached_prompt_id,
                    };
                }
                Ok(None) => {
                    if log_routing {
                        info!("[UnifiedPipeline] Step 1: HistoryMiss (len={})", char_count);
                    }
                }
                Err(e) => {
                    error!("[UnifiedPipeline] Step 1: History lookup failed: {}", e);
                }
            }
        }

        // Step 2: Intent analysis
        let fallback_provider = match settings.active_post_process_provider() {
            Some(p) => p,
            None => {
                decision.result_type = "Skipped".to_string();
                decision.bypass_reason = Some("no_provider".to_string());
                decision.total_elapsed_ms = pipeline_start.elapsed().as_millis() as u64;
                log_pipeline_decision(app_handle, &decision);
                return super::PipelineResult::Skipped;
            }
        };

        let routing_decision = super::routing::execute_smart_action_routing(
            app_handle,
            settings,
            fallback_provider,
            transcription,
            history_id,
        )
        .await;

        // Intent metrics are now self-logged inside routing.rs (each race participant logs independently)

        match &routing_decision {
            Some(d) if d.action == super::routing::SmartAction::PassThrough => {
                // Override PassThrough to LitePolish if text contains repetition patterns
                // (e.g. stuttering, filler sounds like "啊啊", "嗯嗯", or ABAB patterns)
                if has_repetition_pattern(transcription) {
                    if log_routing {
                        info!(
                            "[UnifiedPipeline] Step 2: PassThrough overridden to LitePolish (repetition detected, {} chars)",
                            char_count
                        );
                    }
                    // Fall through to LitePolish handling below
                    let mut upgraded = d.clone();
                    upgraded.action = super::routing::SmartAction::LitePolish;
                    decision.intent_overridden = true;
                    decision.intent_override_reason = Some("repetition_pattern".to_string());
                    intent_decision = Some(upgraded);
                } else {
                    if log_routing {
                        info!(
                            "[UnifiedPipeline] Step 2: PassThrough ({} chars)",
                            char_count
                        );
                    }
                    if let Some(ref an) = app_name {
                        super::recent_context::push(transcription, an);
                    }
                    decision.result_type = "PassThrough".to_string();
                    decision.intent_action = Some("PassThrough".to_string());
                    decision.intent_needs_hotword = Some(d.needs_hotword);
                    decision.intent_language = d.language.clone();
                    decision.intent_model_id = Some(d.model_id.clone());
                    decision.intent_provider_id = Some(d.provider_id.clone());
                    decision.intent_elapsed_ms = Some(d.duration_ms);
                    decision.total_elapsed_ms = pipeline_start.elapsed().as_millis() as u64;
                    log_pipeline_decision(app_handle, &decision);
                    return super::PipelineResult::PassThrough {
                        text: transcription.to_string(),
                        intent_token_count: d.token_count,
                    };
                }
            }
            Some(d) => {
                if log_routing {
                    info!(
                        "[UnifiedPipeline] Step 2: {:?} (needs_hotword={}, {} chars)",
                        d.action, d.needs_hotword, char_count
                    );
                }
            }
            None => {
                if log_routing {
                    info!(
                        "[UnifiedPipeline] Step 2: Intent analysis unavailable, defaulting to FullPolish"
                    );
                }
            }
        }

        // Only set from original decision if we didn't already upgrade it
        if intent_decision.is_none() {
            intent_decision = routing_decision;
        }
    } else if log_routing {
        if is_rewrite_mode {
            info!("[UnifiedPipeline] Rewrite mode, skipping smart routing");
        } else if skill_mode {
            info!("[UnifiedPipeline] Skill mode, skipping smart routing");
        } else if has_selected_text_raw {
            info!(
                "[UnifiedPipeline] Has selected text, skipping smart routing for intent detection"
            );
        } else if !smart_routing_enabled {
            info!("[UnifiedPipeline] Smart routing disabled, going to full pipeline");
        } else {
            info!(
                "[UnifiedPipeline] Long text ({} > {}), skipping smart routing",
                char_count, settings.length_routing_threshold
            );
        }
    }

    // Record intent decision into the pipeline decision accumulator
    if let Some(ref d) = intent_decision {
        decision.intent_action = Some(format!("{:?}", d.action));
        decision.intent_needs_hotword = Some(d.needs_hotword);
        decision.intent_language = d.language.clone();
        decision.intent_model_id = Some(d.model_id.clone());
        decision.intent_provider_id = Some(d.provider_id.clone());
        decision.intent_elapsed_ms = Some(d.duration_ms);
    }

    // ═══════════════════════════════════════════════════════════════
    // Step 3 + 4: Model Selection and Execution
    // ═══════════════════════════════════════════════════════════════
    let is_lite = intent_decision
        .as_ref()
        .map(|d| d.action == super::routing::SmartAction::LitePolish)
        .unwrap_or(false);
    let needs_hotword = intent_decision
        .as_ref()
        .map(|d| d.needs_hotword)
        .unwrap_or(true); // Default: inject hotwords
    let intent_tokens = intent_decision.as_ref().and_then(|d| d.token_count);

    // Resolve detected language: intent model > heuristic > user setting
    let detected_language = intent_decision
        .as_ref()
        .and_then(|d| d.language.clone())
        .unwrap_or_else(|| detect_language_heuristic(transcription, &settings.app_language));

    if log_routing {
        info!("[UnifiedPipeline] Detected language: {}", detected_language);
    }

    // For LitePolish: use lightweight model + lite prompt, always single-model
    if is_lite {
        if log_routing {
            info!("[UnifiedPipeline] Step 3: LitePolish → lightweight single-model path");
        }

        // Build a temporary settings override for lightweight model
        let mut lite_settings = settings.clone();
        if let Some(ref short_model) = settings.length_routing_short_model {
            lite_settings.selected_prompt_model = Some(short_model.clone());
        }
        // Disable hotwords if intent says not needed
        if !needs_hotword {
            lite_settings.post_process_hotword_injection_enabled = false;
        }

        // Load the lite polish prompt and create a temporary LLMPrompt
        let prompt_manager = app_handle.state::<Arc<crate::managers::prompt::PromptManager>>();
        let lite_instructions = prompt_manager
            .get_prompt(app_handle, "system_lite_polish")
            .unwrap_or_else(|_| "Fix minor ASR errors. Output corrected text only.".to_string());

        // Use default prompt as base, override id and instructions
        let lite_prompt = if let Some(base) = lite_settings.post_process_prompts.first() {
            let mut p = base.clone();
            p.id = "__LITE_POLISH__".to_string();
            p.name = "轻量润色".to_string();
            p.instructions = lite_instructions;
            p
        } else {
            decision.result_type = "Skipped".to_string();
            decision.bypass_reason = Some("prompt_not_found".to_string());
            decision.total_elapsed_ms = pipeline_start.elapsed().as_millis() as u64;
            log_pipeline_decision(app_handle, &decision);
            return super::PipelineResult::Skipped;
        };

        // Check for fallback chain on the lite model
        let lite_chain = lite_settings.selected_prompt_model.as_ref();
        let has_lite_fallback = lite_chain.and_then(|c| c.fallback_id.as_ref()).is_some();

        let fallback_provider = match lite_settings.active_post_process_provider() {
            Some(p) => p,
            None => {
                decision.result_type = "Skipped".to_string();
                decision.bypass_reason = Some("no_provider".to_string());
                decision.total_elapsed_ms = pipeline_start.elapsed().as_millis() as u64;
                log_pipeline_decision(app_handle, &decision);
                return super::PipelineResult::Skipped;
            }
        };

        // For non-fallback: resolve model early
        let single_resolved = if !has_lite_fallback {
            match super::routing::resolve_effective_model(
                &lite_settings,
                fallback_provider,
                &lite_prompt,
            ) {
                Some(r) => Some(r),
                None => {
                    decision.result_type = "Skipped".to_string();
                    decision.bypass_reason = Some("model_not_found".to_string());
                    decision.total_elapsed_ms = pipeline_start.elapsed().as_millis() as u64;
                    log_pipeline_decision(app_handle, &decision);
                    return super::PipelineResult::Skipped;
                }
            }
        } else {
            None
        };

        let lite_start = std::time::Instant::now();

        // Build prompt (minimal — no history context for lite, hotword only if needed)
        let hotword_injection =
            if needs_hotword && lite_settings.post_process_hotword_injection_enabled {
                build_hotword_injection(app_handle, &app_name, transcription)
            } else {
                None
            };

        let session_ctx = app_name
            .as_deref()
            .map(super::recent_context::get_for_app)
            .unwrap_or_default();

        let built = super::prompt_builder::PromptBuilder::new(&lite_prompt, transcription)
            .app_name(app_name.as_deref())
            .window_title(window_title.as_deref())
            .hotword_injection(hotword_injection)
            .cursor_context(cursor_context.as_ref())
            .session_context(session_ctx)
            .app_language(&lite_settings.app_language)
            .injection_policy(super::prompt_builder::InjectionPolicy::for_post_process(
                &lite_settings,
            ))
            .build();

        // Save app_name for session context push (before ownership moves into closures)
        let lite_app_name_for_context = app_name.clone();

        // Execute with or without fallback chain
        let (
            result,
            err,
            error_message,
            api_token_count,
            result_model,
            result_provider,
            metrics_logged,
        ) = if has_lite_fallback {
            let chain = lite_chain.unwrap();
            let app = app_handle.clone();
            let s = lite_settings.clone();
            let sys_msgs = built.system_messages.clone();
            let user_msg = built.user_message.clone();
            let an = app_name;
            let wt = window_title;
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
                let wt = wt.clone();
                let metrics = metrics.clone();
                let id_for_err = cached_model_id.clone();
                async move {
                    // Per-call 5s timeout (spec docs/specs/2026-05-26-polish-pipeline-timeout.spec.md).
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(super::core::PER_CALL_TIMEOUT_SECS),
                        async move {
                            let cached = s
                                .cached_models
                                .iter()
                                .find(|m| m.id == cached_model_id)
                                .ok_or_else(|| {
                                format!("Model {} not found", cached_model_id)
                            })?;
                            let provider = s
                                .post_process_provider(&cached.provider_id)
                                .ok_or_else(|| {
                                    format!("Provider {} not found", cached.provider_id)
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
                                    wt,
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
                        },
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(_) => {
                            log::warn!(
                                "[UnifiedPipeline] LitePolish model '{}' exceeded {}s timeout",
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

            if fb_result.is_fallback {
                info!(
                    "[UnifiedPipeline] LitePolish used fallback model '{}'",
                    fb_result.actual_model_id
                );
            }

            match fb_result.result {
                Ok((text, tc, mid, pid)) => (Some(text), false, None, tc, mid, pid, true),
                Err(e) => {
                    log::warn!("[UnifiedPipeline] LitePolish fallback chain failed: {}", e);
                    (
                        None,
                        true,
                        Some(e),
                        None,
                        String::new(),
                        String::new(),
                        true,
                    )
                }
            }
        } else {
            let (actual_provider, model) = single_resolved.unwrap();
            let cached_model_id = lite_prompt.model_id.as_deref().or(lite_settings
                .selected_prompt_model
                .as_ref()
                .map(|c| c.primary_id.as_str()));

            let (result, err, error_message, api_token_count) =
                super::core::execute_llm_request_with_messages(
                    app_handle,
                    &lite_settings,
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
                    None,
                )
                .await;
            (
                result,
                err,
                error_message,
                api_token_count,
                model.clone(),
                actual_provider.id.clone(),
                false,
            )
        };

        let lite_duration_ms = lite_start.elapsed().as_millis() as u64;
        let total_tokens = sum_tokens(intent_tokens, api_token_count);
        let total_calls = Some(if intent_tokens.is_some() { 2 } else { 1 });

        let lite_tokens_per_sec = match (&result, lite_duration_ms) {
            (Some(ref t), d) if d > 0 => {
                let estimate = super::extensions::estimate_tokens(t);
                Some(estimate / d as f64 * 1000.0)
            }
            _ => None,
        };

        if let (Some(ref text), Some(ref an)) = (&result, &lite_app_name_for_context) {
            super::recent_context::push(text, an);
        }
        decision.model_selection = Some("lite".to_string());
        decision.selected_model_id = Some(result_model.clone());
        decision.result_type = "SingleModel".to_string();
        decision.total_elapsed_ms = pipeline_start.elapsed().as_millis() as u64;
        if err {
            if let Some(ref msg) = error_message {
                decision.error_type = Some(classify_error_message(msg).to_string());
            }
            decision.error_detail = error_message.clone();
        }
        log_pipeline_decision(app_handle, &decision);
        return super::PipelineResult::SingleModel {
            text: result,
            model: Some(result_model.clone()),
            prompt_id: Some(lite_prompt.id.clone()),
            token_count: total_tokens,
            llm_call_count: total_calls,
            error: err,
            error_message,
            metrics_model_id: if metrics_logged {
                None
            } else {
                Some(result_model)
            },
            metrics_provider_id: if metrics_logged {
                None
            } else {
                Some(result_provider)
            },
            metrics_duration_ms: Some(lite_duration_ms),
            metrics_tokens_per_sec: lite_tokens_per_sec,
        };
    }

    // FullPolish path: check if multi-model should be used
    // Skip multi-model for rewrite mode, skill mode, and when selected text is present
    // (selected text needs intent detection in Mode C which lives in maybe_post_process_transcription)
    if log_routing {
        info!(
            "[UnifiedPipeline] multi-model gate: enabled={}, selected_ids={}, strategy={}, skill_mode={}, rewrite_mode={}, has_selected_text={}",
            settings.multi_model_post_process_enabled,
            settings.multi_model_selected_ids.len(),
            settings.multi_model_strategy,
            skill_mode,
            is_rewrite_mode,
            has_selected_text_raw,
        );
    }
    let use_multi_model = settings.multi_model_post_process_enabled
        && !skill_mode
        && !is_rewrite_mode
        && !has_selected_text_raw
        && !matches!(
            crate::window_context::resolve_votype_input_mode(
                app_name.as_deref(),
                window_title.as_deref(),
                review_editor_active,
                selected_text
                    .as_ref()
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false),
            ),
            crate::window_context::VotypeInputMode::MainPolishInput
                | crate::window_context::VotypeInputMode::MainSelectedEdit
                | crate::window_context::VotypeInputMode::ReviewRewrite
        );

    if use_multi_model {
        let multi_items = settings.build_multi_model_items_from_selection();
        if !multi_items.is_empty() {
            let strategy = settings.multi_model_strategy.clone();
            let effective_prompt_id = override_prompt_id
                .clone()
                .or(settings.post_process_selected_prompt_id.clone());

            if log_routing {
                info!(
                    "[UnifiedPipeline] Step 3: FullPolish → multi-model ({} models, strategy={})",
                    multi_items.len(),
                    strategy
                );
            }

            decision.model_selection = Some("multi".to_string());
            decision.is_multi_model = true;
            decision.result_type = "MultiModel".to_string();
            decision.total_elapsed_ms = pipeline_start.elapsed().as_millis() as u64;
            log_pipeline_decision(app_handle, &decision);
            return super::PipelineResult::MultiModel {
                multi_items,
                intent_token_count: intent_tokens,
                prompt_id: effective_prompt_id,
                strategy,
            };
        }
    }

    // Single-model full polish: delegate to existing maybe_post_process_transcription
    if log_routing {
        info!("[UnifiedPipeline] Step 3: FullPolish → single-model path");
    }

    // Build effective settings for single-model path
    // - Rewrite mode: select model based on target text length
    // - Non-rewrite: disable hotwords if intent says not needed
    let mut overridden_settings: Option<AppSettings> = None;

    if is_rewrite_mode {
        // Rewrite mode: disable hotword injection (instruction should execute precisely)
        // and select model based on target text length
        let target_len = review_document_text
            .as_ref()
            .map(|t| t.chars().count() as u32)
            .unwrap_or(0);
        let mut s = settings.clone();
        s.post_process_hotword_injection_enabled = false;
        if target_len <= settings.length_routing_threshold {
            if let Some(ref short_model) = settings.length_routing_short_model {
                if log_routing {
                    info!(
                        "[UnifiedPipeline] Rewrite: target_len={}, using lite model, hotwords disabled",
                        target_len
                    );
                }
                s.selected_prompt_model = Some(short_model.clone());
            }
        } else if log_routing {
            info!(
                "[UnifiedPipeline] Rewrite: target_len={}, using default model, hotwords disabled",
                target_len
            );
        }
        overridden_settings = Some(s);
    } else if !needs_hotword && settings.post_process_hotword_injection_enabled {
        let mut s = settings.clone();
        s.post_process_hotword_injection_enabled = false;
        overridden_settings = Some(s);
    }

    let settings_ref = overridden_settings.as_ref().unwrap_or(settings);
    let app_name_for_context = app_name.clone();

    let (
        text,
        model,
        prompt_id,
        err,
        error_message,
        api_token_count,
        api_call_count,
        metrics_model_id,
        metrics_provider_id,
        metrics_duration_ms,
    ) = maybe_post_process_transcription(
        app_handle,
        settings_ref,
        transcription,
        streaming_transcription,
        show_overlay,
        override_prompt_id,
        app_name,
        window_title,
        match_pattern,
        match_type,
        history_id,
        skill_mode,
        review_editor_active,
        selected_text,
        review_document_text,
        cursor_context,
        true, // skip_smart_routing: already done by unified_post_process
    )
    .await;

    // Check for pending skill confirmation
    if model.as_deref() == Some("__PENDING_SKILL_CONFIRMATION__") {
        decision.result_type = "PendingSkillConfirmation".to_string();
        decision.total_elapsed_ms = pipeline_start.elapsed().as_millis() as u64;
        log_pipeline_decision(app_handle, &decision);
        return super::PipelineResult::PendingSkillConfirmation;
    }

    let metrics_tokens_per_sec = match (&text, metrics_duration_ms) {
        (Some(ref t), Some(d)) if d > 0 => {
            let estimate = super::extensions::estimate_tokens(t);
            Some(estimate / d as f64 * 1000.0)
        }
        _ => None,
    };

    if let (Some(ref t), Some(ref an)) = (&text, &app_name_for_context) {
        super::recent_context::push(t, an);
    }

    decision.model_selection = Some("full".to_string());
    decision.selected_model_id = model.clone();
    decision.result_type = "SingleModel".to_string();
    decision.total_elapsed_ms = pipeline_start.elapsed().as_millis() as u64;
    if err {
        if let Some(ref msg) = error_message {
            decision.error_type = Some(classify_error_message(msg).to_string());
        }
        decision.error_detail = error_message.clone();
    }
    log_pipeline_decision(app_handle, &decision);
    super::PipelineResult::SingleModel {
        text,
        model,
        prompt_id,
        token_count: sum_tokens(intent_tokens, api_token_count),
        llm_call_count: sum_counts(
            if intent_tokens.is_some() {
                Some(1)
            } else {
                None
            },
            api_call_count,
        ),
        error: err,
        error_message,
        metrics_model_id,
        metrics_provider_id,
        metrics_duration_ms,
        metrics_tokens_per_sec,
    }
}

/// Helper: sum two optional token counts
fn sum_tokens(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

/// Helper: sum two optional call counts
fn sum_counts(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    sum_tokens(a, b) // Same logic
}

/// Helper: build hotword injection from history manager
pub(super) fn build_hotword_injection(
    app_handle: &AppHandle,
    app_name: &Option<String>,
    transcription: &str,
) -> Option<crate::managers::hotword::HotwordInjection> {
    let hm = app_handle.try_state::<Arc<HistoryManager>>()?;
    let hotword_manager = HotwordManager::new(hm.db_path.clone());
    let scenario = detect_scenario(app_name);
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
            Some(injection)
        }
        _ => None,
    }
}

/// Detect language from text heuristic: if all printable chars are ASCII, assume English;
/// otherwise use the user's app_language setting.
fn detect_language_heuristic(text: &str, app_language: &str) -> String {
    let has_non_ascii = text
        .chars()
        .any(|c| !c.is_ascii() && !c.is_ascii_whitespace());
    if has_non_ascii {
        // Contains CJK or other non-ASCII → use app language (likely "zh" or "ja")
        if app_language.starts_with("zh") {
            "zh".to_string()
        } else if app_language.starts_with("en") {
            "en".to_string()
        } else {
            app_language.to_string()
        }
    } else {
        // All ASCII → English
        "en".to_string()
    }
}

/// Detect usage scenario from app name
pub(super) fn detect_scenario(app_name: &Option<String>) -> Option<HotwordScenario> {
    let work_apps = [
        "Code", "VSCode", "Cursor", "Terminal", "iTerm", "Slack", "Notion", "Figma", "Xcode",
        "IntelliJ",
    ];
    let casual_apps = ["WeChat", "Messages", "Telegram", "WhatsApp", "Discord"];

    if let Some(name) = app_name {
        for app in work_apps {
            if name.contains(app) {
                return Some(HotwordScenario::Work);
            }
        }
        for app in casual_apps {
            if name.contains(app) {
                return Some(HotwordScenario::Casual);
            }
        }
    }
    None // Both scenarios apply
}

async fn execute_votype_rewrite_prompt(
    app_handle: &AppHandle,
    settings: &AppSettings,
    fallback_provider: &PostProcessProvider,
    prompt: &LLMPrompt,
    input_text: &str,
    target_text: &str,
    app_name: Option<String>,
    window_title: Option<String>,
    language: Option<&str>,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
    Option<String>,
    Option<i64>,
    Option<i64>,    // llm_call_count
    Option<String>, // metrics_model_id
    Option<String>, // metrics_provider_id
    Option<u64>,    // metrics_duration_ms
) {
    info!(
        "[VotypeRewrite] app={:?} title={:?} prompt_id={} target_len={} instruction_len={}",
        app_name,
        window_title,
        prompt.id,
        target_text.chars().count(),
        input_text.chars().count()
    );
    info!("[VotypeRewrite] TargetText:\n{}", target_text);
    info!("[VotypeRewrite] SpokenInstruction:\n{}", input_text);

    let session_id = crate::review_window::current_rewrite_session_id();
    let history = crate::review_window::get_rewrite_conversation(session_id);
    let history_turns = history.as_ref().map(|h| h.len() / 2).unwrap_or(0);
    info!(
        "[VotypeRewrite] session_id={} history_turns={}",
        session_id, history_turns
    );

    let Some((actual_provider, model)) =
        super::routing::resolve_effective_model(settings, fallback_provider, prompt)
    else {
        return (
            None,
            None,
            None,
            true,
            Some("未找到可用的后处理模型".to_string()),
            None,
            None,
            None,
            None,
            None,
        );
    };

    let prompt_manager = app_handle.state::<Arc<crate::managers::prompt::PromptManager>>();
    let rewrite_system_prompt = prompt_manager
        .get_prompt(app_handle, "system_votype_rewrite")
        .unwrap_or_else(|_| {
            "You are a high-fidelity document editor. Interpret spoken_instruction, edit current_document. Output JSON with normalized_instruction, operation, rewritten_text, changes.".to_string()
        });
    let system_prompts = vec![rewrite_system_prompt];
    let term_reference = build_rewrite_term_reference(
        app_handle,
        settings,
        target_text,
        input_text,
        app_name.as_deref(),
    );
    let user_message =
        build_rewrite_user_message(target_text, input_text, &term_reference, language);
    let cached_model_id = prompt.model_id.as_deref().or(settings
        .selected_prompt_model
        .as_ref()
        .map(|c| c.primary_id.as_str()));

    // Switch overlay to "处理中…" state for rewrite LLM processing
    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        let _ = overlay_window.emit("show-overlay", "rewrite");
    }

    let rewrite_start = std::time::Instant::now();
    let (result, err, error_message, api_token_count) =
        super::core::execute_llm_request_with_messages(
            app_handle,
            settings,
            actual_provider,
            &model,
            cached_model_id,
            &system_prompts,
            Some(&user_message),
            history.as_deref(),
            app_name,
            window_title,
            None,
            None,
            None,
        )
        .await;
    let rewrite_duration_ms = rewrite_start.elapsed().as_millis() as u64;

    let prompt_text = format!("{}\n{}", system_prompts.join("\n"), user_message);
    let token_count: i64 = api_token_count.unwrap_or_else(|| match tiktoken_rs::cl100k_base() {
        Ok(bpe) => {
            let prompt_tokens = bpe.encode_with_special_tokens(&prompt_text).len() as i64;
            let response_tokens = result
                .as_ref()
                .map(|r| bpe.encode_with_special_tokens(r).len() as i64)
                .unwrap_or(0);
            prompt_tokens + response_tokens
        }
        Err(_) => 0,
    });

    if let Some(raw) = result.as_deref() {
        if let Some(parsed) = super::core::extract_rewrite_response(raw) {
            info!(
                "[VotypeRewrite] ParsedResponse operation={} normalized_instruction={}",
                parsed.operation, parsed.normalized_instruction
            );
            for change in &parsed.changes {
                info!(
                    "[VotypeRewrite] Change from='{}' to='{}' reason='{}'",
                    change.from, change.to, change.reason
                );
            }
            // Append this exchange to the multi-turn conversation history
            crate::review_window::append_rewrite_message(
                session_id,
                crate::review_window::RewriteRole::User,
                user_message.clone(),
            );
            crate::review_window::append_rewrite_message(
                session_id,
                crate::review_window::RewriteRole::Assistant,
                parsed.rewritten_text.clone(),
            );
            // Emit operation completion for overlay flash
            if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
                #[derive(Clone, serde::Serialize)]
                struct OperationComplete {
                    operation: String,
                }
                let _ = overlay_window.emit(
                    "rewrite-operation-complete",
                    OperationComplete {
                        operation: parsed.operation.clone(),
                    },
                );
            }
            return (
                Some(parsed.rewritten_text),
                Some(model.clone()),
                Some(prompt.id.clone()),
                false,
                None,
                Some(token_count),
                Some(1),
                Some(model),
                Some(actual_provider.id.clone()),
                Some(rewrite_duration_ms),
            );
        }
    }

    // Emit fallback operation completion
    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        #[derive(Clone, serde::Serialize)]
        struct OperationComplete {
            operation: String,
        }
        let _ = overlay_window.emit(
            "rewrite-operation-complete",
            OperationComplete {
                operation: "unknown".to_string(),
            },
        );
    }

    (
        result,
        Some(model.clone()),
        Some(prompt.id.clone()),
        err,
        error_message,
        Some(token_count),
        Some(1),
        Some(model),
        Some(actual_provider.id.clone()),
        Some(rewrite_duration_ms),
    )
}

fn build_rewrite_user_message(
    target_text: &str,
    input_text: &str,
    term_reference: &str,
    language: Option<&str>,
) -> String {
    let lang_section = language
        .map(|l| format!("\n\n[output_language]\n{}", l))
        .unwrap_or_default();
    format!(
        "[current_document]\n{}\n\n[spoken_instruction]\n{}\n\n[term_reference]\n{}{}",
        target_text.trim(),
        input_text.trim(),
        term_reference,
        lang_section
    )
}

fn build_rewrite_term_reference(
    app_handle: &AppHandle,
    settings: &AppSettings,
    target_text: &str,
    input_text: &str,
    app_name: Option<&str>,
) -> String {
    if !settings.post_process_hotword_injection_enabled {
        return "(none)".to_string();
    }

    if let Some(hm) = app_handle.try_state::<Arc<HistoryManager>>() {
        let hotword_manager = HotwordManager::new(hm.db_path.clone());
        let scenario = detect_scenario(&app_name.map(|s| s.to_string()));
        let effective_scenario = scenario.unwrap_or(HotwordScenario::Work);
        match hotword_manager.build_ranked_term_reference(
            effective_scenario,
            target_text,
            input_text,
            app_name,
        ) {
            Ok(reference) if !reference.trim().is_empty() => return reference,
            Ok(_) => {}
            Err(e) => {
                error!(
                    "[VotypeRewrite] Failed to build term_reference hotwords: {}",
                    e
                );
            }
        }
    }

    "(none)".to_string()
}

pub async fn maybe_post_process_transcription(
    app_handle: &AppHandle,
    settings: &AppSettings,
    transcription: &str,
    streaming_transcription: Option<&str>,
    _show_overlay: bool,
    override_prompt_id: Option<String>,
    app_name: Option<String>,
    window_title: Option<String>,
    match_pattern: Option<String>,
    match_type: Option<crate::settings::TitleMatchType>,
    history_id: Option<i64>,
    skill_mode: bool,
    review_editor_active: bool,
    selected_text: Option<String>,
    review_document_text: Option<String>,
    cursor_context: Option<crate::clipboard::CursorContext>,
    skip_smart_routing: bool,
) -> (
    Option<String>, // processed text
    Option<String>, // model name
    Option<String>, // prompt id
    bool,           // error
    Option<String>, // error message
    Option<i64>,    // token count (total across all LLM calls)
    Option<i64>,    // llm call count
    Option<String>, // metrics_model_id
    Option<String>, // metrics_provider_id
    Option<u64>,    // metrics_duration_ms
) {
    let log_routing = crate::DEBUG_LOG_ROUTING.load(std::sync::atomic::Ordering::Relaxed);

    if !settings.post_process_enabled {
        return (None, None, None, false, None, None, None, None, None, None);
    }

    // Check for skip post-process marker
    if override_prompt_id.as_deref() == Some("__SKIP_POST_PROCESS__") {
        info!("[PostProcess] Skipping post-processing due to app rule override");
        return (None, None, None, false, None, None, None, None, None, None);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Level 1: Smart Routing (activated by 智能模型 mode / length_routing_enabled)
    // Handles history reuse and action routing BEFORE model selection.
    // ═══════════════════════════════════════════════════════════════════════
    let mut smart_routing_tokens: Option<i64> = None;

    if settings.length_routing_enabled && !skip_smart_routing {
        let char_count = transcription.chars().count() as u32;

        // Layer 0: History exact-match reuse
        if let Some(hm) = app_handle.try_state::<Arc<HistoryManager>>() {
            match hm.find_cached_post_process_result(transcription) {
                Ok(Some((cached_text, cached_model, cached_prompt_id))) => {
                    if log_routing {
                        info!(
                            "[SmartRouting] HistoryHit: reusing cached result (len={}, model={:?})",
                            cached_text.chars().count(),
                            cached_model
                        );
                    }
                    return (
                        Some(cached_text),
                        cached_model,
                        cached_prompt_id,
                        false,
                        None,
                        Some(0),
                        Some(0),
                        None,
                        None,
                        None,
                    );
                }
                Ok(None) => {
                    if log_routing {
                        info!(
                            "[SmartRouting] HistoryMiss: no cached result for input (len={})",
                            char_count
                        );
                    }
                }
                Err(e) => {
                    error!("[SmartRouting] History lookup failed: {}", e);
                }
            }
        }

        // Layer 1: Action routing for short text (only when intent model is configured)
        if char_count <= settings.length_routing_threshold
            && settings.post_process_intent_model.is_some()
        {
            let fallback_provider = match settings.active_post_process_provider() {
                Some(p) => p,
                None => return (None, None, None, false, None, None, None, None, None, None),
            };

            let action_result = super::routing::execute_smart_action_routing(
                app_handle,
                settings,
                fallback_provider,
                transcription,
                history_id,
            )
            .await;

            match &action_result {
                Some(super::IntentDecision {
                    action: super::routing::SmartAction::PassThrough,
                    token_count,
                    ..
                }) => {
                    if log_routing {
                        info!(
                            "[SmartRouting] Action: pass_through ({} chars), returning original text",
                            char_count
                        );
                    }
                    return (
                        Some(transcription.to_string()),
                        None,
                        override_prompt_id.clone(),
                        false,
                        None,
                        *token_count,
                        Some(1),
                        None,
                        None,
                        None,
                    );
                }
                Some(super::IntentDecision {
                    action: super::routing::SmartAction::LitePolish,
                    token_count,
                    ..
                }) => {
                    if log_routing {
                        info!(
                            "[SmartRouting] Action: lite_polish ({} chars), delegating to lightweight model",
                            char_count
                        );
                    }
                    // LitePolish no longer short-circuits — fall through to execution with lightweight model.
                    smart_routing_tokens = *token_count;
                }
                Some(super::IntentDecision {
                    action: super::routing::SmartAction::FullPolish,
                    token_count,
                    ..
                }) => {
                    if log_routing {
                        info!(
                            "[SmartRouting] Action: full_polish ({} chars), delegating to model selection",
                            char_count
                        );
                    }
                    smart_routing_tokens = *token_count;
                }
                None => {
                    if log_routing {
                        info!(
                            "[SmartRouting] Action routing unavailable ({} chars), delegating to model selection",
                            char_count
                        );
                    }
                }
            }
        } else if char_count > settings.length_routing_threshold && log_routing {
            info!(
                "[SmartRouting] Long text ({} chars > {}), skipping action routing",
                char_count, settings.length_routing_threshold
            );
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Level 2: Model Selection (existing logic, unchanged)
    // Single model / Length routing / Multi-model
    // ═══════════════════════════════════════════════════════════════════════

    // Length routing: override selected_prompt_model based on text length
    let settings = if settings.length_routing_enabled && !settings.multi_model_post_process_enabled
    {
        let char_count = transcription.chars().count() as u32;
        let routed_model = if char_count <= settings.length_routing_threshold {
            settings.length_routing_short_model.clone()
        } else {
            settings.length_routing_long_model.clone()
        };
        if routed_model.is_some() {
            let mut s = settings.clone();
            s.selected_prompt_model = routed_model;
            if log_routing {
                info!(
                    "[PostProcess] Length routing: {} chars → model {:?}",
                    char_count, s.selected_prompt_model
                );
            }
            Cow::Owned(s)
        } else {
            Cow::Borrowed(settings)
        }
    } else {
        Cow::Borrowed(settings)
    };
    let settings = settings.as_ref();

    // Track token usage from auxiliary LLM calls
    let mut routing_token_count: i64 = smart_routing_tokens.unwrap_or(0);
    let mut routing_call_count: i64 = if smart_routing_tokens.is_some() { 1 } else { 0 };

    let fallback_provider = match settings.active_post_process_provider() {
        Some(p) => p,
        None => {
            log::warn!(
                "[PostProcess] active_post_process_provider() returned None! provider_id='{}', available_providers=[{}]",
                settings.post_process_provider_id,
                settings.post_process_providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(", ")
            );
            return (None, None, None, false, None, None, None, None, None, None);
        }
    };

    // Load external skills (Phase 9)
    let skill_manager = crate::managers::skill::SkillManager::new(app_handle);
    let external_skills = skill_manager.load_all_external_skills();

    // Merge skills with same logic as merge_external_skills:
    // - Builtin skills take priority over external skills with same ID
    // - Customized skills (customized=true) are not overwritten by file versions
    let mut all_prompts = settings.post_process_prompts.clone();

    for mut file_skill in external_skills {
        // Skip if ID conflicts with builtin skill
        if all_prompts
            .iter()
            .any(|p| p.id == file_skill.id && p.source == crate::settings::SkillSource::Builtin)
        {
            continue;
        }

        if let Some(existing) = all_prompts.iter().find(|p| p.id == file_skill.id) {
            // Keep user's customized version
            if existing.customized {
                continue;
            }

            // Update with file version while preserving customized state
            let pos = all_prompts
                .iter()
                .position(|p| p.id == file_skill.id)
                .unwrap();
            let was_customized = all_prompts[pos].customized;
            let old_file_path = all_prompts[pos].file_path.clone();

            all_prompts[pos] = file_skill;
            all_prompts[pos].customized = was_customized;
            all_prompts[pos].file_path = old_file_path;
        } else {
            // New skill, add it
            file_skill.customized = false;
            all_prompts.push(file_skill);
        }
    }

    // Resolve default prompt from the combined list
    let default_prompt = super::routing::get_default_prompt(
        &all_prompts,
        settings.post_process_selected_prompt_id.as_deref(),
        override_prompt_id.as_deref(),
    );

    let (mut initial_prompt_opt, mut initial_content, mut is_explicit) =
        super::routing::resolve_prompt_from_text(
            transcription,
            &all_prompts,
            default_prompt,
            override_prompt_id.as_deref(),
        );
    // When content is replaced by selected text (input_source="select"),
    // the original transcription becomes the instruction telling the skill what to do.
    // In confirm_skill path, the instruction arrives via streaming_transcription.
    let mut instruction_text: Option<String> = if skill_mode {
        streaming_transcription
            .filter(|s| !s.trim().is_empty() && s.trim() != transcription.trim())
            .map(|s| s.to_string())
    } else {
        None
    };

    // Notify UI immediately about the specific prompt being used if it's an override (app/rule specific)
    if let (Some(p), true) = (&initial_prompt_opt, override_prompt_id.is_some()) {
        app_handle.emit("post-process-status", p.name.clone()).ok();
    }

    // --- Smart Routing Phase ---
    // Only perform LLM-based routing if skill_mode is enabled (dedicated shortcut pressed - Mode B)
    // For selected text (Mode C), we need user confirmation before executing skills
    let votype_mode = crate::window_context::resolve_votype_input_mode(
        app_name.as_deref(),
        window_title.as_deref(),
        review_editor_active,
        selected_text
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false),
    );
    let effective_skill_mode = match votype_mode {
        crate::window_context::VotypeInputMode::MainPolishInput
        | crate::window_context::VotypeInputMode::MainSelectedEdit
        | crate::window_context::VotypeInputMode::ReviewRewrite => false,
        crate::window_context::VotypeInputMode::ExternalDefault => skill_mode,
    };
    let effective_selected_text = match votype_mode {
        crate::window_context::VotypeInputMode::MainPolishInput
        | crate::window_context::VotypeInputMode::ReviewRewrite => None,
        crate::window_context::VotypeInputMode::MainSelectedEdit
        | crate::window_context::VotypeInputMode::ExternalDefault => selected_text.clone(),
    };

    if log_routing {
        info!(
            "[ModeRouting] app={:?}, title={:?}, review_editor_active={}, original_skill_mode={}, resolved_mode={:?}",
            app_name,
            window_title,
            review_editor_active,
            skill_mode,
            votype_mode
        );
        if let Some(text) = selected_text.as_deref() {
            info!("[ModeRouting] RawSelectedText len={}", text.chars().count());
            log::debug!("[ModeRouting] RawSelectedText:\n{}", text);
        }
        if let Some(text) = review_document_text.as_deref() {
            info!(
                "[ModeRouting] ReviewDocumentText len={}",
                text.chars().count()
            );
            log::debug!("[ModeRouting] ReviewDocumentText:\n{}", text);
        }
    }

    if matches!(
        votype_mode,
        crate::window_context::VotypeInputMode::MainSelectedEdit
    ) {
        if let (Some(rewrite_prompt), Some(target_text)) = (
            initial_prompt_opt.as_ref().or(default_prompt),
            selected_text.as_deref(),
        ) {
            return execute_votype_rewrite_prompt(
                app_handle,
                settings,
                fallback_provider,
                rewrite_prompt,
                transcription,
                target_text,
                app_name,
                window_title,
                None, // language: detect from target_text heuristic
            )
            .await;
        }
    }

    if matches!(
        votype_mode,
        crate::window_context::VotypeInputMode::ReviewRewrite
    ) {
        if let (Some(rewrite_prompt), Some(target_text)) = (
            initial_prompt_opt.as_ref().or(default_prompt),
            review_document_text.as_deref(),
        ) {
            // Only clear prompt's model_id when unified_post_process has overridden
            // selected_prompt_model (lite model for short text). For full model path,
            // keep the prompt's own model_id so it can resolve normally.
            let mut rewrite_prompt_adjusted = rewrite_prompt.clone();
            let target_len = target_text.chars().count() as u32;
            if target_len <= settings.length_routing_threshold
                && settings.length_routing_short_model.is_some()
            {
                rewrite_prompt_adjusted.model_id = None;
            }
            // Detect language from target text for rewrite consistency
            let lang = detect_language_heuristic(target_text, &settings.app_language);
            return execute_votype_rewrite_prompt(
                app_handle,
                settings,
                fallback_provider,
                &rewrite_prompt_adjusted,
                transcription,
                target_text,
                app_name,
                window_title,
                Some(&lang),
            )
            .await;
        }
    }

    let has_selected_text = effective_selected_text
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);

    // Mode B: invoke_skill hotkey always enters skill routing, regardless of app rules
    if effective_skill_mode && !transcription.trim().is_empty() {
        if let Some(p) = &initial_prompt_opt {
            if let Some((route_provider, route_model)) =
                super::routing::resolve_intent_routing_model(settings, fallback_provider, p)
            {
                if let Some(routing_result) = super::routing::perform_skill_routing(
                    app_handle,
                    &all_prompts,
                    route_provider,
                    &route_model,
                    transcription,
                    effective_selected_text.as_deref(),
                )
                .await
                {
                    // Accumulate routing token cost
                    if let Some(tc) = routing_result.token_count {
                        routing_token_count += tc;
                        routing_call_count += 1;
                    }
                    let route_response = routing_result.response;
                    let skill_id = &route_response.skill_id;
                    if let Some(routed_prompt) = all_prompts.iter().find(|p| &p.id == skill_id) {
                        // If routed to "default", fall through to normal polish
                        if skill_id != "default" {
                            initial_prompt_opt = Some(routed_prompt.clone());
                            is_explicit = true;
                        }
                        info!(
                            "[PostProcess] Skill mode routed to \"{}\" (input_source: {:?})",
                            routed_prompt.name, route_response.input_source
                        );

                        // Use input_source to determine which content to use.
                        // Heuristic: in skill mode with selected text, default to "select"
                        // unless routing explicitly says "extract". The user pressed the
                        // skill hotkey with a selection — the instruction targets it.
                        let effective_input_source = match route_response.input_source.as_deref() {
                            Some("extract") => "extract",
                            Some("select") => "select",
                            _ if has_selected_text => "select",
                            _ => "output",
                        };
                        initial_content = match effective_input_source {
                            "select" => {
                                instruction_text = Some(transcription.to_string());
                                effective_selected_text.clone().unwrap_or_default()
                            }
                            "extract" => route_response
                                .extracted_content
                                .clone()
                                .unwrap_or_else(|| transcription.to_string()),
                            _ => transcription.to_string(),
                        };

                        // Notify UI about the routed skill
                        app_handle
                            .emit("post-process-status", routed_prompt.name.clone())
                            .ok();
                    }
                }
            }
        }
    }

    // Intent Detection Mode: Only triggered when user has selected text
    // This prevents frequent popup triggers during normal transcription
    // When text is selected, the operation ALWAYS targets the selected content
    // User's speech is only used as intent/instruction hint
    // Note: Some LLM providers may not support concurrent requests, so we handle failures gracefully

    if log_routing {
        info!(
            "[PostProcess] Intent detection conditions: skill_mode={}, is_explicit={}, override_prompt_id={:?}, transcription_empty={}, has_selected_text={}",
            effective_skill_mode, is_explicit, override_prompt_id, transcription.trim().is_empty(), has_selected_text
        );
    }

    // Intent detection: when user has selected text, speech may be an instruction
    // targeting that text. Override prompt (from app rules) is used as fallback for
    // polish if no skill is matched, but should NOT block intent detection.
    if !effective_skill_mode && !transcription.trim().is_empty() && has_selected_text {
        info!("[PostProcess] Entering intent detection mode (has selected text)");

        app_handle.emit("post-process-status", "正在润色中...").ok();

        if let Some(default_prompt) = &initial_prompt_opt {
            if let Some((route_provider, route_model)) =
                super::routing::resolve_intent_routing_model(
                    settings,
                    fallback_provider,
                    default_prompt,
                )
            {
                info!(
                    "[PostProcess] Starting parallel intent detection and polish - provider={}, model={}, selected_text={}",
                    route_provider.label, route_model, has_selected_text
                );

                // Execute both requests in parallel
                let (intent_result, smart_polish_result) = tokio::join!(
                    // Intent detection
                    super::routing::perform_skill_routing(
                        app_handle,
                        &all_prompts,
                        route_provider,
                        &route_model,
                        transcription,
                        effective_selected_text.as_deref(),
                    ),
                    // Smart polish (routes internally via Smart Routing classification)
                    super::routing::execute_smart_polish(
                        app_handle,
                        settings,
                        fallback_provider,
                        default_prompt,
                        transcription,
                        app_name.clone(),
                        window_title.clone(),
                        history_id,
                    )
                );

                // Accumulate token costs from both parallel requests
                if let Some(ref ir) = intent_result {
                    if let Some(tc) = ir.token_count {
                        routing_token_count += tc;
                    }
                    routing_call_count += 1;
                }
                if let Some(ref pr) = smart_polish_result {
                    if let Some(tc) = pr.token_count {
                        routing_token_count += tc;
                    }
                    routing_call_count += match pr.action {
                        super::routing::SmartAction::PassThrough => 1,
                        _ => 2,
                    };
                }

                // Extract inner values for downstream use
                let intent_response = intent_result.map(|r| r.response);
                let polish_text = smart_polish_result.map(|r| r.text);

                // Log results for debugging concurrency issues
                let intent_ok = intent_response.is_some();
                let polish_ok = polish_text.is_some();
                info!(
                    "[PostProcess] Parallel requests completed - Intent: {} (skill: {:?}), Polish: {} (len: {})",
                    if intent_ok { "OK" } else { "FAILED" },
                    intent_response.as_ref().map(|r| &r.skill_id),
                    if polish_ok { "OK" } else { "FAILED" },
                    polish_text.as_ref().map(|s| s.len()).unwrap_or(0)
                );

                // Handle different result combinations:
                // 1. Intent matched + Polish OK/Failed -> Show confirmation (polish_text may be None)
                // 2. Intent failed + Polish OK -> Use polish result directly
                // 3. Both failed -> Fall through to standard processing

                if let Some(route_response) = intent_response {
                    let skill_id = &route_response.skill_id;

                    info!(
                        "[PostProcess] Intent matched! skill_id={}, input_source={:?}, extracted_content={:?}",
                        skill_id, route_response.input_source, route_response.extracted_content
                    );

                    if let Some(routed_prompt) = all_prompts.iter().find(|p| &p.id == skill_id) {
                        info!(
                            "[PostProcess] Found matching prompt: name={}, output_mode={:?}, enabled={}",
                            routed_prompt.name, routed_prompt.output_mode, routed_prompt.enabled
                        );

                        // Check if this is the default skill - if so, skip confirmation and use polish result
                        // Compare with: 1) user-selected default prompt, 2) current default_prompt used for polish
                        let is_default_skill = settings
                            .post_process_selected_prompt_id
                            .as_ref()
                            .map(|default_id| default_id == skill_id)
                            .unwrap_or(false)
                            || default_prompt.id == *skill_id;

                        info!(
                            "[PostProcess] Default skill check: is_default={}, selected_prompt_id={:?}, skill_id={}, default_prompt_id={}",
                            is_default_skill, settings.post_process_selected_prompt_id, skill_id, default_prompt.id
                        );

                        if is_default_skill {
                            info!(
                                "[PostProcess] Matched skill is the default skill, skipping confirmation and using polish result"
                            );
                            // Use polish result directly without confirmation
                            if let Some(ref polished) = polish_text {
                                let total_tokens = if routing_token_count > 0 {
                                    Some(routing_token_count)
                                } else {
                                    None
                                };
                                let total_calls = if routing_call_count > 0 {
                                    Some(routing_call_count)
                                } else {
                                    None
                                };
                                return (
                                    Some(polished.clone()),
                                    None,
                                    Some(routed_prompt.id.clone()),
                                    false,
                                    None,
                                    total_tokens,
                                    total_calls,
                                    None,
                                    None,
                                    None,
                                );
                            }
                            // If polish failed, fall through to standard processing
                        }

                        // [DEBUG] Log selected text content before showing confirmation
                        match &effective_selected_text {
                            Some(text) if !text.trim().is_empty() => {
                                let preview: String = text.chars().take(100).collect();
                                let suffix = if text.chars().count() > 100 {
                                    "..."
                                } else {
                                    ""
                                };
                                info!("[PostProcess] Before confirmation - selected text ({} chars): \"{}{}\"",
                                    text.len(), preview, suffix
                                );
                            }
                            Some(_) => {
                                info!("[PostProcess] Before confirmation - selected text is empty/whitespace");
                            }
                            None => {
                                info!(
                                    "[PostProcess] Before confirmation - no selected text (None)"
                                );
                            }
                        }

                        info!(
                            "[PostProcess] Skill matched: \"{}\" (id={}), polish_available={}, will emit skill-confirmation event",
                            routed_prompt.name, skill_id, polish_ok
                        );

                        // Save pending confirmation state with cached polish result (may be None if failed)
                        // Also capture current active window PID for focus restoration
                        use tauri::Manager;
                        let active_pid = crate::active_window::fetch_active_window()
                            .ok()
                            .map(|info| info.process_id);

                        if let Some(pending_state) =
                            app_handle.try_state::<crate::ManagedPendingSkillConfirmation>()
                        {
                            if let Ok(mut guard) = pending_state.lock() {
                                *guard = crate::PendingSkillConfirmation {
                                    skill_id: Some(skill_id.clone()),
                                    skill_name: Some(routed_prompt.name.clone()),
                                    transcription: Some(transcription.to_string()),
                                    selected_text: effective_selected_text.clone(),
                                    override_prompt_id: override_prompt_id.clone(),
                                    app_name: app_name.clone(),
                                    window_title: window_title.clone(),
                                    history_id,
                                    process_id: active_pid,
                                    active_window: None,
                                    app_review_policy: None,
                                    polish_result: polish_text.clone(), // May be None if parallel polish failed!
                                    input_source: route_response.input_source.clone(),
                                    extracted_content: route_response.extracted_content.clone(),
                                    is_ui_visible: false,
                                };
                            }
                        }

                        // Emit confirmation event to frontend
                        #[derive(serde::Serialize, Clone)]
                        struct SkillConfirmationPayload {
                            skill_id: String,
                            skill_name: String,
                            transcription: String,
                            polish_result: Option<String>,
                            app_name: Option<String>,
                            selected_text_len: Option<usize>,
                            selected_text_preview: Option<String>,
                        }

                        // Build a short preview of the selected text (first 80 chars)
                        let (sel_len, sel_preview) = if let Some(ref text) = effective_selected_text
                        {
                            let chars: Vec<char> = text.chars().collect();
                            let preview = if chars.len() > 200 {
                                format!("{}…", chars[..200].iter().collect::<String>())
                            } else {
                                text.clone()
                            };
                            (Some(chars.len()), Some(preview))
                        } else {
                            (None, None)
                        };

                        app_handle
                            .emit(
                                "skill-confirmation",
                                SkillConfirmationPayload {
                                    skill_id: skill_id.clone(),
                                    skill_name: routed_prompt.name.clone(),
                                    transcription: transcription.to_string(),
                                    polish_result: polish_text,
                                    app_name: app_name.clone(),
                                    selected_text_len: sel_len,
                                    selected_text_preview: sel_preview,
                                },
                            )
                            .ok();

                        // Return early with special model marker to signal pending confirmation
                        // Caller should check for this and skip paste/hide operations
                        // Include routing token costs even though confirmation is pending
                        let total_tokens = if routing_token_count > 0 {
                            Some(routing_token_count)
                        } else {
                            None
                        };
                        let total_calls = if routing_call_count > 0 {
                            Some(routing_call_count)
                        } else {
                            None
                        };
                        return (
                            None,
                            Some("__PENDING_SKILL_CONFIRMATION__".to_string()),
                            None,
                            false,
                            None,
                            total_tokens,
                            total_calls,
                            None,
                            None,
                            None,
                        );
                    }
                }

                // No skill matched - use the polish result directly if available
                // IMPORTANT: Return the default_prompt.id so caller can get correct output_mode
                if let Some(polished) = polish_text {
                    info!(
                        "[PostProcess] No skill matched, using parallel polish result with default prompt: {} (id={})",
                        default_prompt.name, default_prompt.id
                    );
                    let total_tokens = if routing_token_count > 0 {
                        Some(routing_token_count)
                    } else {
                        None
                    };
                    let total_calls = if routing_call_count > 0 {
                        Some(routing_call_count)
                    } else {
                        None
                    };
                    return (
                        Some(polished),
                        None,
                        Some(default_prompt.id.clone()),
                        false,
                        None,
                        total_tokens,
                        total_calls,
                        None,
                        None,
                        None,
                    );
                }

                // Both failed or no match + polish failed - fall through to standard processing
                if !intent_ok && !polish_ok {
                    log::warn!("[PostProcess] Both parallel requests failed, falling back to standard processing");
                }
            }
        }

        // Fallback: continue with default polish
        info!("[PostProcess] Selected text mode - continuing with default polish");
    }

    let initial_prompt = match initial_prompt_opt {
        Some(p) => p,
        None => {
            log::info!(
                "[PostProcess] No user-owned prompt is configured. Skipping post-processing and returning original transcription."
            );
            return (
                Some(transcription.to_string()),
                None,
                None,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            );
        }
    };

    let current_prompt = initial_prompt;
    let current_input_content: String = initial_content;
    let current_transcription = transcription.to_string();

    let final_result;
    let last_model;
    let last_prompt_id;
    let last_err;
    let last_error_message;
    let last_token_count;
    let last_metrics_model_id;
    let last_metrics_provider_id;
    let last_metrics_duration_ms;

    loop {
        let prompt = current_prompt.clone();
        let transcription_content = &current_input_content;
        let transcription_original = &current_transcription;

        let (actual_provider, model) = match super::routing::resolve_effective_model(
            settings,
            fallback_provider,
            &prompt,
        ) {
            Some((p, m)) => {
                if log_routing {
                    log::info!(
                        "[PostProcess] Resolved effective provider: {}, model: {}",
                        p.id,
                        m
                    );
                }
                (p, m)
            }
            None => {
                log::warn!("[PostProcess] resolve_effective_model returned None for fallback_provider={}, prompt={}. Aborting.", fallback_provider.id, prompt.id);
                return (
                    None,
                    None,
                    Some(prompt.id.clone()),
                    false,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                );
            }
        };

        // Keep prompt template as-is, only replace metadata variables
        // (PromptBuilder handles variable detection, stripping, and data injection)

        // Build hotword injection
        let hotword_injection = if settings.post_process_hotword_injection_enabled {
            if let Some(hm) = app_handle.try_state::<Arc<HistoryManager>>() {
                let hotword_manager = HotwordManager::new(hm.db_path.clone());
                let scenario = detect_scenario(&app_name);
                let effective_scenario = scenario.unwrap_or(HotwordScenario::Work);
                match hotword_manager.build_injection(effective_scenario) {
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
                        if log_routing {
                            info!(
                                "[PostProcess] Hotwords injected into prompt: scenario={:?}, terms={}",
                                effective_scenario, total_terms
                            );
                            log::debug!(
                                "[PostProcess] Hotword summary:\n{}",
                                HotwordManager::summarize_injection(&injection)
                            );
                        }
                        Some(injection)
                    }
                    Ok(_) => {
                        info!(
                            "[PostProcess] Hotword injection skipped: scenario={:?}, no active matches or entries",
                            effective_scenario
                        );
                        None
                    }
                    Err(e) => {
                        error!("[PostProcess] Failed to build hotword injection: {}", e);
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
                        match_pattern.as_deref(),
                        match_type,
                        settings.post_process_context_limit as usize,
                        history_id,
                    ) {
                        Ok(history) => history,
                        Err(e) => {
                            error!("Failed to fetch history for context: {}", e);
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

        // Resolve convention-based references from Skill's references/ directory
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
                "[Reference] Injecting {} reference(s) for skill '{}': {:?}",
                resolved_refs.count,
                prompt.name,
                resolved_refs.matched_files,
            );
            Some(resolved_refs.content)
        } else {
            None
        };

        // Use PromptBuilder for unified variable processing
        let session_ctx = app_name
            .as_deref()
            .map(super::recent_context::get_for_app)
            .unwrap_or_default();

        let mut builder = super::prompt_builder::PromptBuilder::new(&prompt, transcription_content)
            .streaming_transcription(streaming_transcription)
            .selected_text(selected_text.as_deref())
            .cursor_context(cursor_context.as_ref())
            .app_name(app_name.as_deref())
            .window_title(window_title.as_deref())
            .history_entries(history_entries)
            .hotword_injection(hotword_injection)
            .resolved_references(refs_content)
            .session_context(session_ctx)
            .app_language(&settings.app_language)
            .injection_policy(super::prompt_builder::InjectionPolicy::for_post_process(
                settings,
            ));
        builder = builder.raw_transcription(transcription_original);
        // When content was replaced (e.g. selected text as input via input_source="select"),
        // pass the original speech as instruction so the skill knows what to do.
        if let Some(ref instr) = instruction_text {
            builder = builder.instruction(Some(instr));
        }
        let built = builder.build();

        let cached_model_id = prompt.model_id.as_deref().or(settings
            .selected_prompt_model
            .as_ref()
            .map(|c| c.primary_id.as_str()));

        // Resolve preset parameters
        let presets_config = app_handle
            .try_state::<std::sync::Arc<crate::managers::model_preset::ModelPresetsConfig>>();
        let merged_extra_params = if let Some(config) = presets_config {
            let cached_model_for_preset = cached_model_id
                .and_then(|id| settings.cached_models.iter().find(|m| m.id == id))
                .or_else(|| {
                    settings
                        .cached_models
                        .iter()
                        .find(|m| m.model_id == model && m.provider_id == actual_provider.id)
                });

            let preset_params = crate::managers::model_preset::resolve_preset_params(
                prompt.param_preset.as_deref(),
                cached_model_for_preset.and_then(|m| m.model_family.as_deref()),
                &model,
                &config,
            );

            if preset_params.is_empty() {
                None
            } else {
                let merged = crate::managers::model_preset::merge_params(
                    preset_params,
                    cached_model_for_preset.and_then(|m| m.extra_params.as_ref()),
                );
                Some(merged)
            }
        } else {
            None
        };

        // Decide whether this call should honor the configured model chain.
        // Prompt-specific model IDs bypass the chain; otherwise fall back to
        // settings.selected_prompt_model so race/staggered/serial strategies apply.
        let chain: Option<crate::fallback::ModelChain> = if prompt.model_id.is_some() {
            None
        } else {
            settings.selected_prompt_model.clone()
        };

        // Track the model actually used for the returned result (may differ
        // from `model` under race/staggered strategies where fallback wins).
        let mut captured_model_id = model.clone();
        let mut captured_provider_id = actual_provider.id.clone();
        let mut used_fallback = false;

        // Each LLM call is protected by a 10s timeout in execute_llm_request_with_messages.
        let polish_start = std::time::Instant::now();

        let (mut result, mut err, mut error_message, mut api_token_count) = if let Some(ref ch) =
            chain
        {
            if log_routing {
                log::info!(
                    "[PostProcess] Executing polish via chain: primary={} fallback={:?} strategy={:?}",
                    ch.primary_id, ch.fallback_id, ch.strategy
                );
            }
            // Clone once; each invocation of the closure re-clones per spawn.
            let app = app_handle.clone();
            let s_clone = settings.clone();
            let sys_msgs = built.system_messages.clone();
            let user_msg = built.user_message.clone();
            let extra = merged_extra_params.clone();
            let app_name_c = app_name.clone();
            let window_title_c = window_title.clone();
            let match_pattern_c = match_pattern.clone();
            let match_type_c = match_type;

            let fb = crate::fallback::execute_with_fallback(ch, |cached_id| {
                let app = app.clone();
                let s = s_clone.clone();
                let sys_msgs = sys_msgs.clone();
                let user_msg = user_msg.clone();
                let extra = extra.clone();
                let app_name_c = app_name_c.clone();
                let window_title_c = window_title_c.clone();
                let match_pattern_c = match_pattern_c.clone();
                let id_for_err = cached_id.clone();
                async move {
                    // Per-call 5s timeout (spec docs/specs/2026-05-26-polish-pipeline-timeout.spec.md).
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(super::core::PER_CALL_TIMEOUT_SECS),
                        async move {
                            let (prov, remote_model) =
                                super::routing::resolve_cached_model_to_provider_owned(
                                    &s, &cached_id,
                                )
                                .ok_or_else(|| format!("Model {} not resolvable", cached_id))?;
                            let (result, err, error_msg, token_count) =
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
                                    match_pattern_c,
                                    match_type_c,
                                    extra.as_ref(),
                                )
                                .await;
                            if err {
                                Err(error_msg.unwrap_or_else(|| "LLM error".into()))
                            } else {
                                let prov_id = prov.id.clone();
                                result
                                    .map(|text| (text, token_count, prov_id, remote_model))
                                    .ok_or_else(|| "Empty LLM response".into())
                            }
                        },
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(_) => {
                            log::warn!(
                                "[PostProcess] model '{}' exceeded {}s timeout",
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

            used_fallback = fb.is_fallback;
            if fb.is_fallback {
                log::warn!(
                    "[PostProcess] Chain: primary '{}' lost (error: {}), used '{}' instead (strategy={:?})",
                    ch.primary_id,
                    fb.primary_error.as_deref().unwrap_or("n/a"),
                    fb.actual_model_id,
                    ch.strategy,
                );
            } else if ch.fallback_id.is_some() && log_routing {
                log::info!(
                    "[PostProcess] Chain: primary '{}' won (strategy={:?})",
                    fb.actual_model_id,
                    ch.strategy,
                );
            }

            match fb.result {
                Ok((text, tokens, prov_id, remote_model)) => {
                    captured_model_id = remote_model;
                    captured_provider_id = prov_id;
                    (Some(text), false, None, tokens)
                }
                Err(e) => {
                    // Every attempt failed — surface overlay manually since we
                    // ran the chain through the silent executor.
                    let _ = app_handle.emit(
                        "overlay-error",
                        serde_json::json!({
                            "code": "llm_error",
                            "message": e.clone(),
                        }),
                    );
                    if let Some((prov, remote_model)) =
                        super::routing::resolve_cached_model_to_provider_owned(
                            settings,
                            &fb.actual_model_id,
                        )
                    {
                        captured_model_id = remote_model;
                        captured_provider_id = prov.id;
                    }
                    (None, true, Some(e), None)
                }
            }
        } else {
            // No chain — direct single call, loud variant surfaces overlay on failure.
            super::core::execute_llm_request_with_messages(
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
                match_pattern.clone(),
                match_type,
                merged_extra_params.as_ref(),
            )
            .await
        };

        let _ = used_fallback;
        let _ = (
            &mut result,
            &mut err,
            &mut error_message,
            &mut api_token_count,
        );
        let polish_duration_ms = polish_start.elapsed().as_millis() as u64;

        let prompt_text = format!(
            "{}\n{}",
            built.system_messages.join("\n"),
            built.user_message.as_deref().unwrap_or_default()
        );
        let computed_token_count: i64 =
            api_token_count.unwrap_or_else(|| match tiktoken_rs::cl100k_base() {
                Ok(bpe) => {
                    let prompt_tokens = bpe.encode_with_special_tokens(&prompt_text).len() as i64;
                    let response_tokens = result
                        .as_ref()
                        .map(|r| bpe.encode_with_special_tokens(r).len() as i64)
                        .unwrap_or(0);
                    prompt_tokens + response_tokens
                }
                Err(_) => 0,
            });

        // Report the model that actually produced the result (race/staggered
        // may substitute the fallback).
        let effective_model = captured_model_id.clone();
        let _ = model; // original primary label no longer needed after chain
        final_result = result;
        last_model = Some(effective_model);
        last_prompt_id = Some(prompt.id.clone());
        last_err = err;
        last_error_message = error_message;
        last_token_count = Some(computed_token_count + routing_token_count);
        last_metrics_model_id = Some(captured_model_id);
        last_metrics_provider_id = Some(captured_provider_id);
        last_metrics_duration_ms = Some(polish_duration_ms);

        if let Some(final_text) = final_result.as_deref() {
            if log_routing {
                info!(
                    "[PostProcess] FinalResult prompt_id={} model={} len={}",
                    prompt.id,
                    last_model.as_deref().unwrap_or_default(),
                    final_text.chars().count()
                );
            }
        } else {
            info!(
                "[PostProcess] FinalResult prompt_id={} model={} is empty",
                prompt.id,
                last_model.as_deref().unwrap_or_default()
            );
        }

        break;
    }

    // 1 for the main LLM call + any routing/polish calls
    let total_call_count = Some(1 + routing_call_count);

    (
        final_result,
        last_model,
        last_prompt_id,
        last_err,
        last_error_message,
        last_token_count,
        total_call_count,
        last_metrics_model_id,
        last_metrics_provider_id,
        last_metrics_duration_ms,
    )
}

/// Detect repetition patterns in text that indicate ASR stuttering or filler sounds.
///
/// Catches:
/// - Consecutive identical characters: "啊啊啊", "嗯嗯", "呃呃"
/// Map the freeform error string produced by the legacy polish call paths
/// (LlmError::message(), per-call timeout wrappers, fallback exhaustion) to a
/// stable `error_type` code. The History UI uses this code to render a tooltip
/// and detail panel, so the codes must stay stable.
pub(super) fn classify_error_message(msg: &str) -> &'static str {
    let lower = msg.to_lowercase();
    // Per-call timeout wrappers in pipeline.rs / routing.rs / extensions.rs all
    // emit "Model <id> exceeded <N>s timeout".
    if lower.contains("exceeded") && lower.contains("timeout") {
        return "llm_timeout";
    }
    // LlmError::message() encodes status as `status=<code>`.
    if let Some(idx) = lower.find("status=") {
        let rest = &lower[idx + "status=".len()..];
        let code: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        return match code.as_str() {
            "401" | "403" => "llm_auth_failed",
            "404" => "llm_model_not_found",
            "429" => "llm_rate_limited",
            "400" => "llm_bad_request",
            c if c.len() == 3 => "llm_api_error",
            _ => "llm_api_error",
        };
    }
    if lower.contains("网络请求失败") || lower.contains("network") || lower.contains("connection")
    {
        return "llm_network_error";
    }
    if lower.contains("客户端初始化失败") || lower.contains("client init") {
        return "llm_init_failed";
    }
    if lower.contains("响应解析失败") || lower.contains("parse") {
        return "llm_parse_error";
    }
    if lower.contains("empty result") || lower.contains("empty llm response") {
        return "llm_empty_result";
    }
    if lower.contains("not found") || lower.contains("not resolvable") {
        return "llm_config_error";
    }
    "llm_unknown"
}

/// Detects common repetition patterns in text that indicate stuttering or filler sounds.
/// Triggers when text contains:
/// - Repeated word patterns (ABAB): "好的好的", "对对对"
/// - Repeated filler sounds: "um um", "uh uh"
pub(super) fn has_repetition_pattern(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }

    let chars: Vec<char> = text.chars().collect();

    // Check for consecutive identical characters (≥2 non-punctuation)
    let mut repeat_count = 1;
    for i in 1..chars.len() {
        if chars[i] == chars[i - 1] && !chars[i].is_whitespace() && !chars[i].is_ascii_punctuation()
        {
            repeat_count += 1;
            if repeat_count >= 2 {
                return true;
            }
        } else {
            repeat_count = 1;
        }
    }

    // Check for repeated word/segment patterns (ABAB)
    // Split into words and check for consecutive duplicates
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() >= 2 {
        let mut dup_count = 1;
        for i in 1..words.len() {
            if words[i].eq_ignore_ascii_case(words[i - 1]) {
                dup_count += 1;
                if dup_count >= 2 {
                    return true;
                }
            } else {
                dup_count = 1;
            }
        }
    }

    // Check CJK repeated segments: split into 1-3 char segments and look for ABAB
    let cjk_chars: Vec<char> = chars
        .iter()
        .filter(|c| matches!(**c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}'))
        .copied()
        .collect();
    for seg_len in 1..=3 {
        if cjk_chars.len() >= seg_len * 2 {
            for i in 0..=cjk_chars.len() - seg_len * 2 {
                let seg1: String = cjk_chars[i..i + seg_len].iter().collect();
                let seg2: String = cjk_chars[i + seg_len..i + seg_len * 2].iter().collect();
                if seg1 == seg2 {
                    return true;
                }
            }
        }
    }

    false
}

fn log_pipeline_decision(
    app_handle: &AppHandle,
    record: &crate::managers::pipeline_log::PipelineDecisionRecord,
) {
    if let Some(manager) =
        app_handle.try_state::<std::sync::Arc<crate::managers::pipeline_log::PipelineLogManager>>()
    {
        manager.log_decision(record);
    }
}

#[cfg(test)]
mod tests {
    use super::{build_rewrite_user_message, has_repetition_pattern};

    #[test]
    fn test_has_repetition_pattern() {
        // Filler sounds
        assert!(has_repetition_pattern("啊啊啊"));
        assert!(has_repetition_pattern("嗯嗯"));
        assert!(has_repetition_pattern("呃呃呃"));

        // Repeated words
        assert!(has_repetition_pattern("好的好的"));
        assert!(has_repetition_pattern("对对对"));

        // Normal text
        assert!(!has_repetition_pattern("你好"));
        assert!(!has_repetition_pattern("今天天气不错"));
        assert!(!has_repetition_pattern("hello world"));
    }

    #[test]
    fn test_build_rewrite_user_message_excludes_app_context() {
        let message = build_rewrite_user_message(
            "当前文稿",
            "口述指令",
            "[热词 reference]\n术语缩写类热词：ASR、JSON",
            None,
        );

        assert!(message.contains("[current_document]"));
        assert!(message.contains("[spoken_instruction]"));
        assert!(message.contains("[term_reference]"));
        assert!(!message.contains("[app_context]"));
        assert!(!message.contains("window="));
        assert!(!message.contains("app="));
    }
}
