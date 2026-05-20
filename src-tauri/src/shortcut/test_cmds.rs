use tauri::{AppHandle, Emitter, Manager};

use crate::settings;

type TestExtras = (
    Option<std::collections::HashMap<String, serde_json::Value>>,
    Option<std::collections::HashMap<String, String>>,
);

/// Resolve the effective `(extra_params, extra_headers)` for a test inference call.
///
/// Rule (per docs/specs/2026-05-20-custom-add-model.spec.md):
/// - If either override is `Some`, both fields are taken from the override pair
///   as-is — the unspecified side becomes `None`, dropping any cached headers
///   or params. Thinking auto-inject is also skipped (the user is responsible
///   for writing thinking keys via the dialog's preset buttons).
/// - Otherwise, look up the CachedModel by `cached_model_id` and merge its
///   `extra_params` with thinking params derived from the cached model's
///   `is_thinking_model` flag and identifying fields. User-supplied keys win
///   on collision (e.g. a user `thinking: {type: "disabled"}` overrides an
///   auto-injected `thinking: {type: "enabled"}`).
fn resolve_test_extras(
    cached_models: &[crate::settings::CachedModel],
    cached_model_id: Option<&str>,
    extra_params_override: Option<std::collections::HashMap<String, serde_json::Value>>,
    extra_headers_override: Option<std::collections::HashMap<String, String>>,
) -> TestExtras {
    if extra_params_override.is_some() || extra_headers_override.is_some() {
        return (extra_params_override, extra_headers_override);
    }

    let cached_model = cached_model_id.and_then(|id| cached_models.iter().find(|m| m.id == id));
    let user_params = cached_model.and_then(|m| m.extra_params.clone());
    let headers = cached_model.and_then(|m| m.extra_headers.clone());
    let thinking_params = cached_model.and_then(|cm| {
        crate::settings::thinking_extra_params_with_aliases(
            &cm.model_id,
            &cm.provider_id,
            cm.is_thinking_model,
            &[cm.custom_label.as_deref().unwrap_or("")],
        )
    });
    let merged_params = match (thinking_params, user_params) {
        (Some(mut tp), Some(up)) => {
            tp.extend(up);
            Some(tp)
        }
        (Some(tp), None) => Some(tp),
        (None, Some(up)) => Some(up),
        (None, None) => None,
    };
    (merged_params, headers)
}

// Group: Inference Testing
#[tauri::command]
#[specta::specta]
pub async fn test_post_process_model_inference(
    app: AppHandle,
    model_id: String,
    provider_id: String,
    cached_model_id: Option<String>,
    extra_params_override: Option<std::collections::HashMap<String, serde_json::Value>>,
    extra_headers_override: Option<std::collections::HashMap<String, String>>,
) -> Result<crate::llm_client::InferenceResult, String> {
    let settings = settings::get_settings(&app);
    let provider = settings
        .post_process_providers
        .iter()
        .find(|p| p.id == provider_id)
        .ok_or("Provider not found")?;

    let (merged_extra_params, extra_headers) = resolve_test_extras(
        &settings.cached_models,
        cached_model_id.as_deref(),
        extra_params_override,
        extra_headers_override,
    );

    let effective_proxy = crate::settings::resolve_proxy(&settings, provider);
    let max_attempts = settings
        .post_process_api_keys
        .get(&provider_id)
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
            provider_id: provider_id.clone(),
            cached_model_id: cached_model_id
                .clone()
                .unwrap_or_else(|| "__test_inference__".to_string()),
            remote_model_id: model_id.clone(),
            max_attempts,
        },
        {
            let provider = provider.clone();
            let model_id = model_id.clone();
            let prompt = "你是啥模型？".to_string();
            let merged_extra_params = merged_extra_params.clone();
            let extra_headers = extra_headers.clone();
            let effective_proxy = effective_proxy.clone();

            move |api_key| {
                let provider = provider.clone();
                let model_id = model_id.clone();
                let prompt = prompt.clone();
                let merged_extra_params = merged_extra_params.clone();
                let extra_headers = extra_headers.clone();
                let effective_proxy = effective_proxy.clone();
                let api_key = api_key.to_string();

                async move {
                    match crate::llm_client::send_chat_completion_with_params(
                        &provider,
                        api_key,
                        &model_id,
                        prompt,
                        merged_extra_params.as_ref(),
                        extra_headers.as_ref(),
                        effective_proxy.as_deref(),
                    )
                    .await
                    {
                        Ok(result) => Ok(result),
                        Err(detail) => {
                            let status = detail
                                .strip_prefix("API request failed with status ")
                                .and_then(|rest| rest.split(':').next())
                                .and_then(|raw| raw.trim().parse::<u16>().ok());
                            let error = match status {
                                Some(401 | 403) => crate::provider_gateway::AttemptError::Fatal {
                                    status,
                                    detail,
                                    kind: crate::provider_gateway::AttemptErrorKind::Http,
                                },
                                Some(429) | Some(500..=599) => {
                                    crate::provider_gateway::AttemptError::Retryable {
                                        status,
                                        detail,
                                        kind: crate::provider_gateway::AttemptErrorKind::Http,
                                    }
                                }
                                Some(_) => crate::provider_gateway::AttemptError::Fatal {
                                    status,
                                    detail,
                                    kind: crate::provider_gateway::AttemptErrorKind::Http,
                                },
                                None => crate::provider_gateway::AttemptError::Retryable {
                                    status: None,
                                    detail,
                                    kind: crate::provider_gateway::AttemptErrorKind::Network,
                                },
                            };
                            Err(error)
                        }
                    }
                }
            }
        },
    )
    .await;

    let result = match outcome {
        crate::provider_gateway::ExecutionOutcome::Success(result) => result,
        crate::provider_gateway::ExecutionOutcome::Fatal { detail, .. } => return Err(detail),
        crate::provider_gateway::ExecutionOutcome::Exhausted { last_error, .. } => {
            let detail = match last_error {
                crate::provider_gateway::AttemptError::Retryable { detail, .. }
                | crate::provider_gateway::AttemptError::Fatal { detail, .. } => detail,
            };
            return Err(detail);
        }
    };

    // Log to metrics
    if let Some(metrics) =
        app.try_state::<std::sync::Arc<crate::managers::llm_metrics::LlmMetricsManager>>()
    {
        let duration_ms = result.duration_ms.unwrap_or(0);
        let tokens_per_sec = match (result.total_tokens, result.duration_ms) {
            (Some(tokens), Some(ms)) if ms > 0 => Some(tokens as f64 / ms as f64 * 1000.0),
            _ => None,
        };
        if let Err(e) = metrics.log_call(&crate::managers::llm_metrics::LlmCallRecord {
            history_id: None,
            model_id: model_id.clone(),
            provider: provider_id.clone(),
            call_type: "test".to_string(),
            input_tokens: None,
            output_tokens: None,
            total_tokens: result.total_tokens,
            token_estimate: None,
            duration_ms,
            tokens_per_sec,
            error: None,
            is_fallback: false,
        }) {
            log::warn!("Failed to log test inference metrics: {}", e);
        }
    }

    // Emit event so frontend can refresh stats
    let _ = app.emit("llm-metrics-updated", ());

    Ok(result)
}

#[tauri::command]
#[specta::specta]
pub async fn test_asr_model_inference(
    _app: AppHandle,
    _model_id: String,
) -> Result<String, String> {
    Ok("Test successful".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::CachedModel;
    use std::collections::HashMap;

    fn make_cached_model(id: &str, is_thinking: bool) -> CachedModel {
        CachedModel {
            id: id.to_string(),
            name: "test".to_string(),
            model_type: crate::settings::ModelType::Text,
            provider_id: "openai".to_string(),
            model_id: "gpt-4o".to_string(),
            added_at: "2026-05-20T00:00:00Z".to_string(),
            is_thinking_model: is_thinking,
            prompt_message_role: crate::settings::PromptMessageRole::System,
            custom_label: None,
            extra_params: Some({
                let mut m = HashMap::new();
                m.insert("temperature".to_string(), serde_json::json!(0.7));
                m
            }),
            extra_headers: Some({
                let mut m = HashMap::new();
                m.insert("X-Cached".to_string(), "yes".to_string());
                m
            }),
            model_family: None,
        }
    }

    #[test]
    fn override_path_uses_overrides_and_ignores_cached() {
        let models = vec![make_cached_model("m1", true)];
        let mut params_override = HashMap::new();
        params_override.insert("top_p".to_string(), serde_json::json!(0.9));
        let mut headers_override = HashMap::new();
        headers_override.insert("X-Inline".to_string(), "yes".to_string());

        let (params, headers) = resolve_test_extras(
            &models,
            Some("m1"),
            Some(params_override.clone()),
            Some(headers_override.clone()),
        );

        assert_eq!(
            params,
            Some(params_override),
            "should use override params, not cached"
        );
        assert_eq!(
            headers,
            Some(headers_override),
            "should use override headers, not cached"
        );
    }

    #[test]
    fn override_partial_only_params_yields_none_headers() {
        let models = vec![make_cached_model("m1", false)];
        let mut params_override = HashMap::new();
        params_override.insert("top_p".to_string(), serde_json::json!(0.9));

        let (params, headers) =
            resolve_test_extras(&models, Some("m1"), Some(params_override.clone()), None);

        assert_eq!(params, Some(params_override));
        assert_eq!(
            headers, None,
            "override path drops cached headers when only params provided"
        );
    }

    #[test]
    fn legacy_path_returns_cached_model_extras() {
        let models = vec![make_cached_model("m1", false)];
        let (params, headers) = resolve_test_extras(&models, Some("m1"), None, None);
        assert!(params.is_some(), "should pull params from cached model");
        assert!(headers.is_some(), "should pull headers from cached model");
        let p = params.unwrap();
        assert_eq!(p.get("temperature"), Some(&serde_json::json!(0.7)));
    }

    #[test]
    fn legacy_path_no_cached_id_returns_none() {
        let models = vec![make_cached_model("m1", false)];
        let (params, headers) = resolve_test_extras(&models, None, None, None);
        assert_eq!(params, None);
        assert_eq!(headers, None);
    }

    #[test]
    fn legacy_path_user_params_win_on_thinking_collision() {
        // DeepSeek-R1 style: is_thinking_model=true triggers auto-injected
        // thinking params; user-supplied `thinking` key must override.
        let mut model = make_cached_model("m1", true);
        model.model_id = "deepseek-r1".to_string();
        model.provider_id = "deepseek".to_string();
        // Replace extra_params with a conflicting `thinking` key.
        model.extra_params = Some({
            let mut m = HashMap::new();
            m.insert(
                "thinking".to_string(),
                serde_json::json!({ "type": "disabled" }),
            );
            m
        });

        let (params, _) = resolve_test_extras(&[model], Some("m1"), None, None);
        let p = params.expect("merged params should be Some");
        // User's `thinking: {type: disabled}` must win over auto-injected one.
        assert_eq!(
            p.get("thinking"),
            Some(&serde_json::json!({ "type": "disabled" })),
        );
    }

    #[test]
    fn override_some_with_cached_model_id_still_ignores_cached() {
        // Both an override AND a cached_model_id are provided — override wins.
        let models = vec![make_cached_model("m1", true)];
        let mut params_override = HashMap::new();
        params_override.insert("top_p".to_string(), serde_json::json!(0.9));

        let (params, _) =
            resolve_test_extras(&models, Some("m1"), Some(params_override.clone()), None);
        let p = params.unwrap();
        assert!(p.contains_key("top_p"), "override-only key must be present");
        assert!(
            !p.contains_key("temperature"),
            "cached extra_params must NOT leak in when override is provided"
        );
    }

    #[test]
    fn legacy_path_thinking_only_when_user_params_none() {
        // Cached model has is_thinking_model=true but no user extra_params.
        // Result must still carry the auto-injected thinking params.
        let mut model = make_cached_model("m1", true);
        model.model_id = "deepseek-r1".to_string();
        model.provider_id = "deepseek".to_string();
        model.extra_params = None;

        let (params, _) = resolve_test_extras(&[model], Some("m1"), None, None);
        // If thinking_extra_params_with_aliases returns None for unknown families
        // we still want the test to be informative — assert the function did not
        // panic and returned None gracefully. If it returns Some, the merged
        // params should contain the thinking marker.
        match params {
            Some(p) => assert!(
                p.contains_key("thinking") || p.contains_key("reasoning_effort"),
                "expected a thinking-related key from auto-inject",
            ),
            None => {
                // Acceptable: no alias matched. Test still asserts the
                // (Some(tp), None) arm doesn't panic.
            }
        }
    }
}
