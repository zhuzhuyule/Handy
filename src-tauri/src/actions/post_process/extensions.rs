use crate::managers::history::HistoryManager;
use crate::settings::{AppSettings, LLMPrompt, MultiModelPostProcessItem};
use async_openai::types::ChatCompletionRequestMessage;
use ferrous_opencc::{config::BuiltinConfig, OpenCC};
use futures_util::stream::{FuturesUnordered, StreamExt};
use log::{error, info};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::time::{sleep_until, Instant as TokioInstant};

pub async fn maybe_convert_chinese_variant(
    settings: &AppSettings,
    transcription: &str,
) -> Option<String> {
    let is_simplified = settings.selected_language == "zh-Hans";
    let is_traditional = settings.selected_language == "zh-Hant";

    if !is_simplified && !is_traditional {
        return None;
    }

    let config = if is_simplified {
        BuiltinConfig::Tw2sp
    } else {
        BuiltinConfig::S2tw
    };

    match OpenCC::from_config(config) {
        Ok(converter) => Some(converter.convert(transcription)),
        Err(e) => {
            error!("Failed to initialize OpenCC converter: {}", e);
            None
        }
    }
}

#[allow(dead_code)]
/// Execute multi-model parallel post-processing
/// Returns all results when all models complete, or partial results if cancelled
pub async fn multi_post_process_transcription(
    _app_handle: &AppHandle,
    settings: &AppSettings,
    transcription: &str,
    streaming_transcription: Option<&str>,
    _history_id: Option<i64>,
    app_name: Option<String>,
    window_title: Option<String>,
    override_prompt_id: Option<String>,
) -> super::MultiModelCollectedResults {
    // Check if multi-model post-processing is enabled

    if !settings.multi_model_post_process_enabled {
        info!("[MultiModel] Multi-model post-processing is disabled");
        return super::MultiModelCollectedResults {
            results: Vec::new(),
            auto_selected_id: None,
        };
    }

    // Prefer checkbox-based selection; fall back to legacy items
    let built_items = settings.build_multi_model_items_from_selection();
    let mut items_owned: Vec<MultiModelPostProcessItem>;
    let items: Vec<&MultiModelPostProcessItem>;
    if !built_items.is_empty() {
        items_owned = built_items;
    } else {
        items_owned = settings
            .enabled_multi_model_items()
            .into_iter()
            .cloned()
            .collect();
    }

    // Override prompt_id when app-specific prompt is configured
    if let Some(ref oid) = override_prompt_id {
        info!("[MultiModel] Overriding prompt_id to: {}", oid);
        for item in &mut items_owned {
            item.prompt_id = oid.clone();
        }
    } else {
        info!(
            "[MultiModel] No override_prompt_id, using default: {:?}",
            items_owned.first().map(|i| &i.prompt_id)
        );
    }

    items = items_owned.iter().collect();
    if items.is_empty() {
        info!("[MultiModel] No enabled multi-model items configured");
        return super::MultiModelCollectedResults {
            results: Vec::new(),
            auto_selected_id: None,
        };
    }

    info!(
        "[MultiModel] Starting multi-model post-processing with {} models, prompt_id: {:?}",
        items.len(),
        items.first().map(|i| &i.prompt_id)
    );

    // Emit start event
    let _ = _app_handle.emit(
        "multi-post-process-start",
        serde_json::json!({
            "total": items.len(),
            "transcription_length": transcription.len()
        }),
    );

    // Pre-resolve prompts once (shared across all models using the same prompt_id)
    let skill_manager = crate::managers::skill::SkillManager::new(_app_handle);
    let external_skills = skill_manager.load_all_external_skills();
    let mut all_prompts = settings.post_process_prompts.clone();
    for file_skill in external_skills {
        if !all_prompts.iter().any(|p| p.id == file_skill.id) {
            all_prompts.push(file_skill);
        }
    }

    // Inject built-in lite polish prompt if referenced
    if items.iter().any(|i| i.prompt_id == "__LITE_POLISH__")
        && !all_prompts.iter().any(|p| p.id == "__LITE_POLISH__")
    {
        let prompt_manager =
            _app_handle.state::<std::sync::Arc<crate::managers::prompt::PromptManager>>();
        let lite_instructions = prompt_manager
            .get_prompt(_app_handle, "system_lite_polish")
            .unwrap_or_else(|_| "Fix minor ASR errors. Output corrected text only.".to_string());
        let mut lite_prompt = LLMPrompt::default();
        lite_prompt.id = "__LITE_POLISH__".to_string();
        lite_prompt.name = "轻量润色".to_string();
        lite_prompt.instructions = lite_instructions;
        all_prompts.push(lite_prompt);
    }

    let unique_prompt_ids: Vec<String> = {
        let mut ids: Vec<String> = items.iter().map(|i| i.prompt_id.clone()).collect();
        ids.sort();
        ids.dedup();
        ids
    };

    let mut resolved_prompts: HashMap<String, LLMPrompt> = HashMap::new();
    for pid in &unique_prompt_ids {
        match all_prompts.iter().find(|p| &p.id == pid) {
            Some(p) => {
                info!(
                    "[MultiModel] Resolved prompt: id={}, name=\"{}\", instructions_len={}",
                    p.id,
                    p.name,
                    p.instructions.len()
                );
                resolved_prompts.insert(pid.clone(), p.clone());
            }
            None => {
                error!(
                    "[MultiModel] Prompt not found: {}, available: {:?}",
                    pid,
                    all_prompts.iter().map(|p| &p.id).collect::<Vec<_>>()
                );
            }
        }
    }

    if resolved_prompts.is_empty() {
        error!("[MultiModel] No prompts could be resolved, aborting");
        return super::MultiModelCollectedResults {
            results: Vec::new(),
            auto_selected_id: None,
        };
    }

    let resolved_prompts = Arc::new(resolved_prompts);

    // Build hotword injection (shared across all models)
    let shared_hotword_injection = if settings.post_process_hotword_injection_enabled {
        if let Some(hm) = _app_handle.try_state::<Arc<HistoryManager>>() {
            let hotword_manager = crate::managers::hotword::HotwordManager::new(hm.db_path.clone());
            let scenario = crate::actions::post_process::pipeline::detect_scenario(&app_name);
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
                        "[MultiModel] Hotwords injected: scenario={:?}, terms={}",
                        effective_scenario, total_terms
                    );
                    log::debug!(
                        "[MultiModel] Hotword summary:\n{}",
                        crate::managers::hotword::HotwordManager::summarize_injection(&injection)
                    );
                    Some(injection)
                }
                Ok(_) => {
                    info!("[MultiModel] Hotword injection skipped: no active matches");
                    None
                }
                Err(e) => {
                    error!("[MultiModel] Failed to build hotword injection: {}", e);
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    // Build history context (shared across all models)
    let shared_history_entries: Vec<String> = if settings.post_process_context_enabled {
        if let Some(app) = &app_name {
            if let Some(hm) = _app_handle.try_state::<Arc<HistoryManager>>() {
                match hm.get_recent_history_texts_for_app(
                    app,
                    window_title.as_deref(),
                    None,
                    None,
                    settings.post_process_context_limit as usize,
                    _history_id,
                ) {
                    Ok(history) if !history.is_empty() => {
                        info!(
                            "[MultiModel] Fetched {} history entries as context",
                            history.len()
                        );
                        history
                    }
                    Ok(_) => Vec::new(),
                    Err(e) => {
                        error!("[MultiModel] Failed to fetch history context: {}", e);
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

    // Measure "speed" from the user's perspective: from batch start until the
    // full result for each model is available.
    let batch_start_time = Instant::now();

    // Create futures for each model
    let mut futures: FuturesUnordered<_> = items
        .iter()
        .map(|item| {
            let settings = settings.clone();
            let app_handle = _app_handle.clone();
            let transcription = transcription.to_string();
            let streaming = streaming_transcription.map(|s| s.to_string());
            let history = shared_history_entries.clone();
            let hotwords = shared_hotword_injection.clone();
            let batch_start_time = batch_start_time;
            let prompts = Arc::clone(&resolved_prompts);
            let app_name_clone = app_name.clone();

            async move {
                let result = execute_single_model_post_process(
                    &app_handle,
                    &settings,
                    item,
                    &transcription,
                    streaming.as_deref(),
                    history.clone(),
                    hotwords.clone(),
                    &prompts,
                    app_name_clone.as_deref(),
                )
                .await;

                let elapsed = batch_start_time.elapsed().as_millis() as u64;

                let text = result.0.clone().unwrap_or_default();
                let ready = result.0.is_some();

                let (model_label, provider_label) = get_item_labels(&settings, item);
                let output_speed = if ready && elapsed > 0 {
                    let token_estimate = estimate_tokens(&text);
                    Some((token_estimate / elapsed as f64) * 1000.0) // tokens per second
                } else {
                    None
                };
                let result_struct = super::MultiModelPostProcessResult {
                    id: item.id.clone(),
                    label: model_label,
                    provider_label,
                    text,
                    confidence: None,
                    processing_time_ms: elapsed,
                    error: result.1,
                    ready,
                    token_count: result.2,
                    output_speed,
                };

                // Log to metrics
                if let Some(metrics) = app_handle
                    .try_state::<std::sync::Arc<crate::managers::llm_metrics::LlmMetricsManager>>()
                {
                    if let Err(e) = metrics.log_call(&crate::managers::llm_metrics::LlmCallRecord {
                        history_id: _history_id,
                        model_id: item.model_id.clone(),
                        provider: item.provider_id.clone(),
                        call_type: "multi_model".to_string(),
                        input_tokens: None,
                        output_tokens: None,
                        total_tokens: result_struct.token_count,
                        token_estimate: output_speed.map(|s| s * elapsed as f64 / 1000.0),
                        duration_ms: elapsed as i64,
                        tokens_per_sec: output_speed,
                        error: result_struct.error.clone(),
                        is_fallback: false,
                    }) {
                        log::error!("[LlmMetrics] Failed to log multi-model call: {}", e);
                    }
                }

                result_struct
            }
        })
        .collect();

    let mut all_results: Vec<super::MultiModelPostProcessResult> = Vec::new();
    let mut auto_selected_id: Option<String> = None;
    let total = items.len();

    let strategy = settings.multi_model_strategy.as_str();

    let preferred_model_id = if strategy == "lazy" || strategy == "manual" {
        settings
            .multi_model_preferred_id
            .clone()
            .filter(|id| items.iter().any(|item| item.id == *id))
            .or_else(|| {
                // Infer preferred model from manual pick history
                settings
                    .multi_model_manual_pick_counts
                    .iter()
                    .filter(|(id, _)| items.iter().any(|item| &item.id == *id))
                    .max_by_key(|(_, count)| *count)
                    .map(|(id, _)| id.clone())
            })
            .or_else(|| {
                settings
                    .selected_prompt_model
                    .as_ref()
                    .map(|c| c.primary_id.clone())
                    .filter(|id| items.iter().any(|item| item.id == *id))
            })
    } else {
        None
    };
    let timeout_deadline = TokioInstant::now() + Duration::from_secs(3);
    let mut timeout_elapsed = false;

    // Collect results as they complete
    while !futures.is_empty() {
        let next_result = if strategy == "lazy" {
            tokio::select! {
                result = futures.next() => result,
                _ = sleep_until(timeout_deadline), if !timeout_elapsed => {
                    timeout_elapsed = true;
                    if let Some(preferred) = preferred_model_id
                        .as_ref()
                        .and_then(|id| find_ready_result(&all_results, id))
                    {
                        info!("[MultiModel] Lazy mode selected preferred model at timeout: {}", preferred.id);
                        if auto_selected_id.is_none() {
                            auto_selected_id = Some(preferred.id.clone());
                            let _ = _app_handle.emit(
                                "multi-post-process-auto-selected",
                                serde_json::json!({ "id": preferred.id }),
                            );
                        }
                        // Continue collecting remaining results
                    } else if let Some(best) = find_latest_ready_result(&all_results) {
                        info!("[MultiModel] Lazy mode selected latest ready result at timeout: {}", best.id);
                        if auto_selected_id.is_none() {
                            auto_selected_id = Some(best.id.clone());
                            let _ = _app_handle.emit(
                                "multi-post-process-auto-selected",
                                serde_json::json!({ "id": best.id }),
                            );
                        }
                        // Continue collecting remaining results
                    }
                    continue;
                }
            }
        } else {
            futures.next().await
        };

        let Some(result) = next_result else {
            break;
        };

        all_results.push(result.clone());
        let completed = all_results.len();

        info!(
            "[MultiModel] Progress: {}/{} completed (id: {})",
            completed, total, result.id
        );

        // Keep REVIEW_EDITOR_CONTENT in sync with the first successful candidate
        // so voice rewrite can freeze it at any time.
        if completed == 1 && result.ready && result.error.is_none() {
            crate::review_window::set_review_editor_content(result.text.clone());
        }

        // Emit progress event
        let _ = _app_handle.emit(
            "multi-post-process-progress",
            super::MultiModelProgressEvent {
                total,
                completed,
                results: all_results.clone(),
                done: completed >= total,
            },
        );

        if strategy == "race"
            && auto_selected_id.is_none()
            && result.ready
            && result.error.is_none()
        {
            info!(
                "[MultiModel] Race mode auto-selected: id={}, completed={}/{}",
                result.id, completed, total
            );
            auto_selected_id = Some(result.id.clone());
            let _ = _app_handle.emit(
                "multi-post-process-auto-selected",
                serde_json::json!({ "id": result.id }),
            );
            // Continue collecting remaining results (don't return early)
        }

        // Manual mode: auto-select when the favorite model's result arrives
        if strategy == "manual" && auto_selected_id.is_none() {
            if let Some(ref preferred_id) = preferred_model_id {
                if result.ready && result.error.is_none() && &result.id == preferred_id {
                    info!("[MultiModel] Manual mode favorite arrived: {}", result.id);
                    auto_selected_id = Some(result.id.clone());
                    let _ = _app_handle.emit(
                        "multi-post-process-auto-selected",
                        serde_json::json!({ "id": result.id }),
                    );
                }
            }
        }

        if strategy == "lazy" {
            if !timeout_elapsed {
                if let Some(preferred_id) = preferred_model_id.as_ref() {
                    if result.ready && result.error.is_none() && &result.id == preferred_id {
                        info!(
                            "[MultiModel] Lazy mode preferred model arrived within timeout: {}",
                            result.id
                        );
                        if auto_selected_id.is_none() {
                            auto_selected_id = Some(result.id.clone());
                            let _ = _app_handle.emit(
                                "multi-post-process-auto-selected",
                                serde_json::json!({ "id": result.id }),
                            );
                        }
                        // Continue collecting remaining results
                    }
                }
            } else if result.ready && result.error.is_none() {
                if auto_selected_id.is_none() {
                    info!(
                        "[MultiModel] Lazy mode selected first result after timeout: {}",
                        result.id
                    );
                    auto_selected_id = Some(result.id.clone());
                    let _ = _app_handle.emit(
                        "multi-post-process-auto-selected",
                        serde_json::json!({ "id": result.id }),
                    );
                }
                // Continue collecting remaining results
            } else if auto_selected_id.is_none() {
                if let Some(best) = find_latest_ready_result(&all_results) {
                    info!(
                        "[MultiModel] Lazy mode selected latest ready result after timeout progress: {}",
                        best.id
                    );
                    auto_selected_id = Some(best.id.clone());
                    let _ = _app_handle.emit(
                        "multi-post-process-auto-selected",
                        serde_json::json!({ "id": best.id }),
                    );
                    // Continue collecting remaining results
                }
            }
        }
    }

    if strategy == "lazy" && auto_selected_id.is_none() {
        if let Some(preferred) = preferred_model_id
            .as_ref()
            .and_then(|id| find_ready_result(&all_results, id))
        {
            info!(
                "[MultiModel] Lazy mode selected preferred model at completion: {}",
                preferred.id
            );
            auto_selected_id = Some(preferred.id.clone());
            let _ = _app_handle.emit(
                "multi-post-process-auto-selected",
                serde_json::json!({ "id": preferred.id }),
            );
        } else if let Some(best) = find_latest_ready_result(&all_results) {
            info!(
                "[MultiModel] Lazy mode selected latest ready result at completion: {}",
                best.id
            );
            auto_selected_id = Some(best.id.clone());
            let _ = _app_handle.emit(
                "multi-post-process-auto-selected",
                serde_json::json!({ "id": best.id }),
            );
        }
    }

    // Manual mode fallback: if favorite never completed successfully, select first ready result
    if strategy == "manual" && auto_selected_id.is_none() {
        if let Some(first_ready) = all_results.iter().find(|r| r.ready && r.error.is_none()) {
            info!(
                "[MultiModel] Manual mode fallback: favorite unavailable, selecting {}",
                first_ready.id
            );
            auto_selected_id = Some(first_ready.id.clone());
            let _ = _app_handle.emit(
                "multi-post-process-auto-selected",
                serde_json::json!({ "id": first_ready.id }),
            );
        }
    }

    info!("[MultiModel] All {} models completed", total);

    emit_multi_complete(_app_handle, total, all_results.len(), all_results.clone());
    super::MultiModelCollectedResults {
        results: all_results,
        auto_selected_id,
    }
}

fn find_ready_result<'a>(
    results: &'a [super::MultiModelPostProcessResult],
    id: &str,
) -> Option<&'a super::MultiModelPostProcessResult> {
    results
        .iter()
        .find(|r| r.id == id && r.ready && r.error.is_none())
}

fn find_latest_ready_result(
    results: &[super::MultiModelPostProcessResult],
) -> Option<super::MultiModelPostProcessResult> {
    results
        .iter()
        .rev()
        .find(|r| r.ready && r.error.is_none())
        .cloned()
}

fn emit_multi_complete(
    app_handle: &AppHandle,
    total: usize,
    completed: usize,
    results: Vec<super::MultiModelPostProcessResult>,
) {
    let _ = app_handle.emit(
        "multi-post-process-complete",
        super::MultiModelProgressEvent {
            total,
            completed,
            results,
            done: true,
        },
    );
}

#[allow(dead_code)]
/// Execute post-processing for a single model using PromptBuilder
async fn execute_single_model_post_process(
    _app_handle: &AppHandle,
    settings: &AppSettings,
    item: &MultiModelPostProcessItem,
    transcription: &str,
    streaming_transcription: Option<&str>,
    history_entries: Vec<String>,
    hotword_injection: Option<crate::managers::hotword::HotwordInjection>,
    resolved_prompts: &HashMap<String, LLMPrompt>,
    app_name: Option<&str>,
) -> (Option<String>, Option<String>, Option<i64>) {
    // Get provider
    let provider = match settings.post_process_provider(&item.provider_id) {
        Some(p) => p,
        None => {
            error!("[MultiModel] Provider not found: {}", item.provider_id);
            return (None, Some("Provider not found".to_string()), None);
        }
    };

    // Use the model_id from the item directly (each item specifies its own model)
    let model = item.model_id.clone();

    // Use pre-resolved prompt
    let prompt = match resolved_prompts.get(&item.prompt_id) {
        Some(p) => p,
        None => {
            error!(
                "[MultiModel] Prompt not found in pre-resolved map: {}",
                item.prompt_id,
            );
            return (None, Some("Prompt not found".to_string()), None);
        }
    };

    // Resolve convention-based references
    let app_category = app_name
        .map(crate::app_category::from_app_name)
        .unwrap_or("Other");
    let resolved_refs = super::reference_resolver::resolve_references(
        prompt.file_path.as_deref(),
        app_name,
        app_category,
    );
    let refs_content = if resolved_refs.count > 0 {
        Some(resolved_refs.content)
    } else {
        None
    };

    // Use PromptBuilder for unified variable processing
    let session_ctx = app_name
        .map(super::recent_context::get_for_app)
        .unwrap_or_default();

    let built = super::prompt_builder::PromptBuilder::new(prompt, transcription)
        .streaming_transcription(streaming_transcription)
        .app_name(app_name)
        .history_entries(history_entries)
        .hotword_injection(hotword_injection)
        .resolved_references(refs_content)
        .session_context(session_ctx)
        .app_language(&settings.app_language)
        .injection_policy(super::prompt_builder::InjectionPolicy::for_post_process(
            settings,
        ))
        .build();

    // Build messages
    let mut messages: Vec<ChatCompletionRequestMessage> = Vec::new();

    // 1. Structured system messages
    let prompt_role =
        super::core::resolve_prompt_message_role(settings, &provider.id, None, &model);
    if crate::DEBUG_LOG_POST_PROCESS.load(std::sync::atomic::Ordering::Relaxed) {
        for (index, system_prompt) in built.system_messages.iter().enumerate() {
            super::core::preview_multiline(
                &format!("MultiModel[{}].SystemPrompt[{}]", item.id, index),
                system_prompt,
            );
        }
        if let Some(user_content) = built.user_message.as_deref() {
            super::core::preview_multiline(
                &format!("MultiModel[{}].UserMessage", item.id),
                user_content,
            );
        }
    }
    for system_prompt in built.system_messages {
        if let Some(msg) = super::core::build_instruction_message(prompt_role, system_prompt) {
            messages.push(msg);
        }
    }

    // 2. Single user message
    if let Some(user_content) = built.user_message {
        if let Some(msg) = super::core::build_user_message(user_content) {
            messages.push(msg);
        }
    }

    if messages.is_empty() {
        return (None, Some("Failed to build messages".to_string()), None);
    }

    // Resolve CachedModel to get extra_params
    // item.id equals cached_model.id (set in build_multi_model_items_from_selection)
    let cached_model = settings
        .cached_models
        .iter()
        .find(|m| m.id == item.id)
        .or_else(|| {
            settings
                .cached_models
                .iter()
                .find(|m| m.model_id == model && m.provider_id == provider.id)
        });
    // Resolve preset params
    let presets_config = _app_handle
        .try_state::<std::sync::Arc<crate::managers::model_preset::ModelPresetsConfig>>();
    let effective_extra_params = if let Some(config) = presets_config {
        let preset_params = crate::managers::model_preset::resolve_preset_params(
            prompt.param_preset.as_deref(),
            cached_model.and_then(|m| m.model_family.as_deref()),
            &model,
            &config,
        );
        if preset_params.is_empty() {
            cached_model.and_then(|m| m.extra_params.clone())
        } else {
            Some(crate::managers::model_preset::merge_params(
                preset_params,
                cached_model.and_then(|m| m.extra_params.as_ref()),
            ))
        }
    } else {
        cached_model.and_then(|m| m.extra_params.clone())
    };

    let extra_params = effective_extra_params.as_ref();

    // Build request body JSON (same approach as core.rs for extra_params support)
    let messages_json: Vec<serde_json::Value> = messages
        .into_iter()
        .filter_map(|m| serde_json::to_value(m).ok())
        .collect();

    let mut body = serde_json::json!({
        "model": model,
        "messages": messages_json,
    });

    // Merge extra params if provided
    if let Some(extras) = extra_params {
        if let Some(obj) = body.as_object_mut() {
            for (k, v) in extras {
                obj.insert(k.clone(), v.clone());
            }
        }
    }

    let extra_keys: Vec<&str> = body
        .as_object()
        .map(|obj| {
            obj.keys()
                .filter(|k| *k != "model" && *k != "messages")
                .map(|k| k.as_str())
                .collect()
        })
        .unwrap_or_default();
    if crate::DEBUG_LOG_POST_PROCESS.load(std::sync::atomic::Ordering::Relaxed) {
        info!(
            "[MultiModel] Request: item_id={} provider={} model={} extra_params={:?}",
            item.id, provider.id, model, extra_keys
        );
        if log::log_enabled!(log::Level::Debug) {
            if let Ok(pretty_body) = serde_json::to_string_pretty(&body) {
                log::debug!(
                    "[MultiModel] RequestBody item_id={} provider={} model={}:\n{}",
                    item.id,
                    provider.id,
                    model,
                    pretty_body
                );
            }
        }
    }

    // Manual HTTP request (supports extra_params and longer timeout for thinking models).
    // Content-Type lives on the shared client; everything else goes per-request.
    let base_url = provider.base_url.trim_end_matches('/');
    let url = format!("{}/chat/completions", base_url);

    let mut base_headers = reqwest::header::HeaderMap::new();
    if provider.id == "anthropic" {
        base_headers.insert(
            "anthropic-version",
            reqwest::header::HeaderValue::from_static("2023-06-01"),
        );
    }
    if let Some(custom) = &provider.custom_headers {
        for (k, v) in custom {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(v),
            ) {
                base_headers.insert(name, val);
            }
        }
    }
    if let Some(custom) = cached_model.and_then(|m| m.extra_headers.as_ref()) {
        for (k, v) in custom {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(v),
            ) {
                base_headers.insert(name, val);
            }
        }
    }

    let is_thinking_model = cached_model.map(|m| m.is_thinking_model).unwrap_or(false);
    let timeout_secs = if is_thinking_model { 120 } else { 60 };

    let effective_proxy = crate::settings::resolve_proxy(settings, &provider);
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
        _app_handle,
        settings,
        crate::provider_gateway::ExecutionPlan {
            provider_id: provider.id.clone(),
            cached_model_id: item.id.clone(),
            remote_model_id: model.clone(),
            max_attempts,
        },
        {
            let provider = provider.clone();
            let model = model.clone();
            let item_id = item.id.clone();
            let url = url.clone();
            let body = body.clone();
            let base_headers = base_headers.clone();
            let effective_proxy = effective_proxy.clone();

            move |api_key| {
                let api_key = api_key.to_string();
                let provider = provider.clone();
                let model = model.clone();
                let item_id = item_id.clone();
                let url = url.clone();
                let body = body.clone();
                let headers = base_headers.clone();
                let effective_proxy = effective_proxy.clone();
                let provider_base = provider.base_url.trim_end_matches('/').to_string();

                async move {
                    let http_client = match crate::http_client::get_shared_client(
                        effective_proxy.as_deref(),
                    ) {
                        Ok(client) => client,
                        Err(e) => {
                            let detail = format!("Client creation failed: {:?}", e);
                            error!("[MultiModel] {}", detail);
                            return Err(crate::provider_gateway::AttemptError::Fatal {
                                status: None,
                                detail,
                                kind: crate::provider_gateway::AttemptErrorKind::ClientInit,
                            });
                        }
                    };

                    let mut waf_warmup_done = false;
                    let resp = loop {
                        // Per-request: Authorization + provider/model headers + timeout.
                        let mut req = http_client
                            .post(&url)
                            .timeout(std::time::Duration::from_secs(timeout_secs))
                            .headers(headers.clone());
                        req = if provider.id == "anthropic" {
                            req.header("x-api-key", &api_key)
                        } else {
                            req.bearer_auth(&api_key)
                        };
                        let candidate = match req.json(&body).send().await {
                            Ok(resp) => resp,
                            Err(e) => {
                                let detail = format!("LLM request failed: {:?}", e);
                                error!("[MultiModel] {}", detail);
                                return Err(crate::provider_gateway::AttemptError::Retryable {
                                    status: None,
                                    detail,
                                    kind: crate::provider_gateway::AttemptErrorKind::Network,
                                });
                            }
                        };

                        // WAF cookie challenge: 403/405/406 + empty body → warm up via GET /models.
                        if !waf_warmup_done {
                            let status_u16 = candidate.status().as_u16();
                            if matches!(status_u16, 403 | 405 | 406) {
                                let content_len = candidate
                                    .content_length()
                                    .unwrap_or(0);
                                if content_len == 0 {
                                    waf_warmup_done = true;
                                    let warmup_url = format!("{}/models", provider_base);
                                    log::warn!(
                                        "[MultiModel] WAF cookie challenge (status={}) for provider='{}' — warming up via GET {}",
                                        status_u16, provider.id, warmup_url
                                    );
                                    let _ = http_client
                                        .get(&warmup_url)
                                        .timeout(std::time::Duration::from_secs(10))
                                        .send()
                                        .await;
                                    // discard candidate, retry POST
                                    drop(candidate);
                                    continue;
                                }
                            }
                        }
                        break candidate;
                    };

                    if !resp.status().is_success() {
                        let status = resp.status();
                        let error_text = resp.text().await.unwrap_or_default();
                        let detail = format!("API error ({}): {}", status, error_text);
                        error!("[MultiModel] {}", detail);
                        return Err(super::core::classify_http_status_for_failover(
                            status.as_u16(),
                            detail,
                        ));
                    }

                    let json_resp: serde_json::Value = match resp.json().await {
                        Ok(json_resp) => json_resp,
                        Err(e) => {
                            let detail = format!("Response parse failed: {:?}", e);
                            error!("[MultiModel] {}", detail);
                            return Err(crate::provider_gateway::AttemptError::Fatal {
                                status: None,
                                detail,
                                kind: crate::provider_gateway::AttemptErrorKind::Parse,
                            });
                        }
                    };
                    if crate::DEBUG_LOG_POST_PROCESS.load(std::sync::atomic::Ordering::Relaxed)
                        && log::log_enabled!(log::Level::Debug)
                    {
                        if let Ok(pretty_resp) = serde_json::to_string_pretty(&json_resp) {
                            log::debug!(
                                "[MultiModel] ResponseBody item_id={} provider={} model={}:\n{}",
                                item_id,
                                provider.id,
                                model,
                                pretty_resp
                            );
                        }
                    }

                    let message_obj = &json_resp["choices"][0]["message"];
                    let raw_content = message_obj["content"]
                        .as_str()
                        .unwrap_or_default()
                        .replace('\u{200B}', "")
                        .replace('\u{200C}', "")
                        .replace('\u{200D}', "")
                        .replace('\u{FEFF}', "");

                    let reasoning = message_obj["reasoning_content"]
                        .as_str()
                        .or_else(|| message_obj["reasoning"].as_str())
                        .or_else(|| message_obj["thinking"].as_str());
                    let has_think_tags =
                        raw_content.contains("<think>") || raw_content.contains("</think>");
                    let is_thinking = reasoning.is_some() || has_think_tags;

                    let text = super::core::extract_llm_text(&raw_content);

                    if crate::DEBUG_LOG_POST_PROCESS.load(std::sync::atomic::Ordering::Relaxed) {
                        info!(
                            "[MultiModel] Model {} completed: len={} thinking={} reasoning_len={}",
                            item_id,
                            text.len(),
                            is_thinking,
                            reasoning.map(|r| r.len()).unwrap_or(0),
                        );
                        info!(
                            "[MultiModel] FinalResult item_id={} provider={} model={}",
                            item_id, provider.id, model
                        );
                        super::core::preview_multiline(
                            "MultiModelResponseContentRaw",
                            &raw_content,
                        );
                        if let Some(reasoning_text) = reasoning {
                            super::core::preview_multiline(
                                "MultiModelResponseReasoning",
                                reasoning_text,
                            );
                        }
                        if text != raw_content {
                            super::core::preview_multiline("MultiModelResponseText", &text);
                        }
                    }

                    let token_count = json_resp
                        .get("usage")
                        .and_then(|u| u.get("total_tokens"))
                        .and_then(|t| t.as_i64())
                        .or_else(|| {
                            tiktoken_rs::cl100k_base().ok().map(|bpe| {
                                let prompt_tokens =
                                    bpe.encode_with_special_tokens(
                                        &serde_json::to_string(&body).unwrap_or_default(),
                                    )
                                    .len() as i64;
                                let response_tokens =
                                    bpe.encode_with_special_tokens(&text).len() as i64;
                                prompt_tokens + response_tokens
                            })
                        });

                    Ok((text, token_count))
                }
            }
        },
    )
    .await;

    match outcome {
        crate::provider_gateway::ExecutionOutcome::Success((text, token_count)) => {
            (Some(text), None, token_count)
        }
        crate::provider_gateway::ExecutionOutcome::Fatal { detail, .. } => {
            (None, Some(detail), None)
        }
        crate::provider_gateway::ExecutionOutcome::Exhausted {
            provider_id,
            attempts,
            last_error: attempt_error,
        } => {
            let fallback = match attempt_error {
                crate::provider_gateway::AttemptError::Retryable { detail, .. }
                | crate::provider_gateway::AttemptError::Fatal { detail, .. } => detail,
            };
            let detail = format!(
                "Provider {} exhausted after {} attempts: {}",
                provider_id, attempts, fallback
            );
            (None, Some(detail), None)
        }
    }
}

#[allow(dead_code)]
/// Get display label and provider label for a multi-model item.
/// Returns (model_label, provider_label).
fn get_item_labels(settings: &AppSettings, item: &MultiModelPostProcessItem) -> (String, String) {
    let provider_label = settings
        .post_process_provider(&item.provider_id)
        .map(|p| p.label.clone())
        .unwrap_or_else(|| item.provider_id.clone());

    let model_label = item
        .custom_label
        .clone()
        .unwrap_or_else(|| item.model_id.clone());

    (model_label, provider_label)
}

/// Estimate token count from text using a simple heuristic.
/// Chinese characters ≈ 1 token each; English words ≈ 1.3 tokens each.
pub(crate) fn estimate_tokens(text: &str) -> f64 {
    let mut tokens = 0.0;
    let mut ascii_word_chars = 0;

    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            ascii_word_chars += 1;
        } else {
            if ascii_word_chars > 0 {
                // Flush accumulated ASCII word: ~1.3 tokens per word
                tokens += 1.3;
                ascii_word_chars = 0;
            }
            if ch.is_ascii_whitespace() || ch.is_ascii_punctuation() {
                // Whitespace/punctuation: shared across tokens, negligible
            } else {
                // CJK and other non-ASCII chars: ~1 token each
                tokens += 1.0;
            }
        }
    }
    // Flush remaining ASCII word
    if ascii_word_chars > 0 {
        tokens += 1.3;
    }

    if tokens < 1.0 {
        1.0
    } else {
        tokens
    }
}
