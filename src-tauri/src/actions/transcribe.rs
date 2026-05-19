use super::post_process::maybe_convert_chinese_variant;
use super::ShortcutAction;
use crate::active_window;
use crate::audio_feedback::{play_feedback_sound, play_feedback_sound_blocking, SoundType};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::history::HistoryManager;
use crate::managers::model::ModelManager;
use crate::managers::transcription::TranscriptionManager;
use crate::overlay::{
    show_inserting_overlay, show_recording_overlay, show_recording_overlay_rewrite,
    show_transcribing_overlay, show_translation_overlay,
};
use crate::settings::{get_settings, AppSettings, CachedModel};
use crate::shortcut;
use crate::tray::{change_tray_icon, TrayIconState};
use crate::utils;
use crate::window_context::VotypeInputMode;
use anyhow::anyhow;
use log::{debug, error, info};
use once_cell::sync::Lazy;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::time::{sleep, timeout};

pub(super) struct TranscribeAction {
    pub skill_mode: bool,
}

const EFFECTIVE_RECORDING_TOO_SHORT_MS: i64 = 500;
const WHISPER_SAMPLE_RATE: usize = 16000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordingDisposition {
    Proceed { effective_duration_ms: i64 },
    IgnoredTooShort { effective_duration_ms: i64 },
}

fn effective_recording_duration_ms(samples: &[f32]) -> i64 {
    ((samples.len() as f64 / WHISPER_SAMPLE_RATE as f64) * 1000.0) as i64
}

/// Persist raw ASR samples to disk for offline replay/debugging.
/// Writes to `<app_data_dir>/debug_audio/<timestamp>-<tag>.wav`.
/// Returns the path on success so the caller can log it.
fn save_debug_audio(
    app_handle: &AppHandle,
    samples: &[f32],
    tag: &str,
) -> Option<std::path::PathBuf> {
    use std::io::Write;

    let data_dir = app_handle.path().app_data_dir().ok()?;
    let dir = data_dir.join("debug_audio");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!(
            "[ASR-diag] failed to create debug_audio dir {}: {}",
            dir.display(),
            e
        );
        return None;
    }

    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
    let safe_tag: String = tag
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = dir.join(format!("{}-{}.wav", ts, safe_tag));

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: WHISPER_SAMPLE_RATE as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = match hound::WavWriter::create(&path, spec) {
        Ok(w) => w,
        Err(e) => {
            log::warn!("[ASR-diag] failed to create wav {}: {}", path.display(), e);
            return None;
        }
    };
    for &s in samples {
        let v = (s * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        if let Err(e) = writer.write_sample(v) {
            log::warn!("[ASR-diag] failed to write sample: {}", e);
            return None;
        }
    }
    if let Err(e) = writer.finalize() {
        log::warn!("[ASR-diag] failed to finalize wav: {}", e);
        return None;
    }

    // Also drop a small sidecar with context so we know what this file is.
    let meta_path = path.with_extension("txt");
    if let Ok(mut f) = std::fs::File::create(&meta_path) {
        let _ = writeln!(
            f,
            "tag={}\nsamples={}\nseconds={:.2}\nsample_rate={}",
            tag,
            samples.len(),
            samples.len() as f64 / WHISPER_SAMPLE_RATE as f64,
            WHISPER_SAMPLE_RATE,
        );
    }

    Some(path)
}

fn classify_recording_samples(samples: &[f32]) -> RecordingDisposition {
    let effective_duration_ms = effective_recording_duration_ms(samples);
    if effective_duration_ms < EFFECTIVE_RECORDING_TOO_SHORT_MS {
        RecordingDisposition::IgnoredTooShort {
            effective_duration_ms,
        }
    } else {
        RecordingDisposition::Proceed {
            effective_duration_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PostProcessOutcome {
    final_text: String,
    failed: bool,
}

fn resolve_post_process_outcome(
    transcription_text: &str,
    processed_text: Option<&str>,
    err: bool,
) -> PostProcessOutcome {
    match processed_text
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        Some(text) => PostProcessOutcome {
            final_text: text.to_string(),
            failed: err,
        },
        None => PostProcessOutcome {
            final_text: transcription_text.to_string(),
            failed: true,
        },
    }
}

fn should_show_post_process_review_window(
    votype_mode: VotypeInputMode,
    override_prompt_id: Option<&str>,
    app_policy: crate::settings::AppReviewPolicy,
    output_mode: crate::settings::PromptOutputMode,
    manual_review: bool,
) -> bool {
    if matches!(
        votype_mode,
        VotypeInputMode::MainPolishInput
            | VotypeInputMode::MainSelectedEdit
            | VotypeInputMode::ReviewRewrite
    ) {
        return false;
    }

    if override_prompt_id == Some("__SKIP_POST_PROCESS__") {
        return false;
    }

    if output_mode == crate::settings::PromptOutputMode::Chat {
        return true;
    }

    if manual_review {
        return true;
    }

    app_policy != crate::settings::AppReviewPolicy::Never
}

fn direct_paste_target_pid(
    active_window_snapshot: Option<&crate::active_window::ActiveWindowInfo>,
) -> Option<u64> {
    active_window_snapshot
        .map(|info| info.process_id)
        .filter(|pid| *pid != 0)
}

fn restore_focus_before_direct_paste(
    active_window_snapshot: Option<&crate::active_window::ActiveWindowInfo>,
) {
    let Some(pid) = direct_paste_target_pid(active_window_snapshot) else {
        return;
    };

    if let Err(e) = crate::active_window::focus_app_by_pid(pid) {
        log::warn!(
            "[DirectPaste] Failed to restore focus to original app pid {}: {}",
            pid,
            e
        );
    } else {
        std::thread::sleep(Duration::from_millis(120));
    }
}

fn app_profile_translate_to_english_on_insert(
    settings: &AppSettings,
    active_window_snapshot: Option<&crate::active_window::ActiveWindowInfo>,
) -> bool {
    let Some(info) = active_window_snapshot else {
        return false;
    };

    let Some(profile_id) = settings
        .app_to_profile
        .iter()
        .find(|(app_name, _)| app_name.eq_ignore_ascii_case(&info.app_name))
        .map(|(_, profile_id)| profile_id)
    else {
        return false;
    };

    settings
        .app_profiles
        .iter()
        .find(|profile| &profile.id == profile_id)
        .map(|profile| profile.translate_to_english_on_insert)
        .unwrap_or(false)
}

async fn maybe_translate_text_for_insert(
    app_handle: &AppHandle,
    should_translate: bool,
    skill_mode: bool,
    text: String,
) -> String {
    // Translation is a polish-flow capability only. Skill outputs follow the
    // skill's own insertion semantics — never auto-translate them, even if the
    // app-profile flag is on.
    if skill_mode || !should_translate || text.trim().is_empty() {
        return text;
    }

    show_translation_overlay(app_handle);

    match crate::commands::text::translate_text_to_english(app_handle, &text).await {
        Ok(translated) if !translated.trim().is_empty() => translated,
        Ok(_) => {
            log::warn!("[InsertTranslation] Translation returned empty text, using original text");
            text
        }
        Err(err) => {
            log::warn!(
                "[InsertTranslation] Translation failed, using original text: {}",
                err
            );
            text
        }
    }
}

/// 在 `start()` 按键瞬间捕获到的活动窗口快照，配对 `transcription_id`。
/// `stop()` 取回时必须校验 id，避免被后续录音覆盖时仍把旧快照交给已 race 的旧 pipeline。
// `dead_code` allowed while Task 1 lands the slot ahead of Tasks 2-4 wiring the call sites.
#[allow(dead_code)]
static START_SNAPSHOT: Lazy<StdMutex<Option<(u64, crate::active_window::ActiveWindowInfo)>>> =
    Lazy::new(|| StdMutex::new(None));

/// 写入 start 时的活动窗口快照。`info = None`（fetch 失败）时不占用 slot，
/// stop 取回时会按既有兜底再次 `fetch_active_window()`。
#[allow(dead_code)] // removed in Task 2 when start() wires this up
pub(super) fn set_start_snapshot(
    transcription_id: u64,
    info: Option<crate::active_window::ActiveWindowInfo>,
) {
    let mut slot = START_SNAPSHOT.lock().expect("START_SNAPSHOT poisoned");
    match info {
        Some(info) => {
            *slot = Some((transcription_id, info));
        }
        None => {
            // 不写 None 占位；若 slot 里存着另一个 id 的旧值，让 owner 的清理来处理。
        }
    }
}

/// 若 slot 中存有指定 `transcription_id` 的快照则取出并清空，否则返回 None 且保留 slot。
#[allow(dead_code)] // removed in Task 3 when stop() wires this up
pub(super) fn take_start_snapshot(
    transcription_id: u64,
) -> Option<crate::active_window::ActiveWindowInfo> {
    let mut slot = START_SNAPSHOT.lock().expect("START_SNAPSHOT poisoned");
    match slot.as_ref() {
        Some((stored_id, _)) if *stored_id == transcription_id => slot.take().map(|(_, info)| info),
        _ => None,
    }
}

/// FinishGuard 兜底用：若 slot 仍属于本次录音则清掉，避免长期残留。
#[allow(dead_code)] // removed in Task 4 when FinishGuard::drop wires this up
pub(super) fn clear_start_snapshot_if_matches(transcription_id: u64) {
    let mut slot = START_SNAPSHOT.lock().expect("START_SNAPSHOT poisoned");
    if matches!(slot.as_ref(), Some((stored_id, _)) if *stored_id == transcription_id) {
        *slot = None;
    }
}

impl TranscribeAction {
    pub fn new(skill_mode: bool) -> Self {
        Self { skill_mode }
    }
}

fn online_asr_max_attempts(settings: &AppSettings, provider_id: &str) -> usize {
    settings
        .post_process_api_keys
        .get(provider_id)
        .map(|keys| {
            keys.iter()
                .filter(|entry| entry.enabled && !entry.key.is_empty())
                .count()
                .clamp(1, 3)
        })
        .unwrap_or(1)
}

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

    if lower.contains("failed to deserialize transcription response")
        || lower.contains("transcription missing text field")
    {
        return AttemptError::Fatal {
            status: None,
            detail: detail.to_string(),
            kind: AttemptErrorKind::Parse,
        };
    }

    AttemptError::Retryable {
        status: None,
        detail: detail.to_string(),
        kind: AttemptErrorKind::Other,
    }
}

async fn execute_online_asr_request(
    app_handle: &AppHandle,
    settings: &AppSettings,
    cached_model: &CachedModel,
    audio_samples: Vec<f32>,
    lang: String,
    is_fallback: bool,
    metrics: Option<std::sync::Arc<crate::managers::llm_metrics::LlmMetricsManager>>,
    history_id: Option<i64>,
    audio_ms: i64,
) -> anyhow::Result<String> {
    let provider = settings
        .post_process_providers
        .iter()
        .find(|p| p.id == cached_model.provider_id)
        .cloned()
        .ok_or_else(|| anyhow!("ASR provider not found for model {}", cached_model.name))?;
    let provider_id_for_log = cached_model.provider_id.clone();
    let cached_id_for_log = cached_model.model_id.clone();
    let remote_model_id = cached_model.model_id.clone();
    let audio_samples = Arc::new(audio_samples);
    let lang = Arc::new(lang);
    let start = Instant::now();

    let outcome = crate::provider_gateway::execute_with_failover(
        app_handle,
        settings,
        crate::provider_gateway::ExecutionPlan {
            provider_id: provider.id.clone(),
            cached_model_id: cached_model.id.clone(),
            remote_model_id: remote_model_id.clone(),
            max_attempts: online_asr_max_attempts(settings, &provider.id),
        },
        {
            let provider = provider.clone();
            let remote_model_id = remote_model_id.clone();
            let audio_samples = audio_samples.clone();
            let lang = lang.clone();

            move |api_key| {
                let provider = provider.clone();
                let remote_model_id = remote_model_id.clone();
                let audio_samples = audio_samples.clone();
                let lang = lang.clone();
                let api_key = api_key.to_string();

                async move {
                    let join_result = tokio::task::spawn_blocking(move || {
                        let client = crate::online_asr::OnlineAsrClient::new(
                            16000,
                            Duration::from_secs(120),
                        );
                        let language = if lang.as_str() == "auto" {
                            None
                        } else {
                            Some(lang.as_str())
                        };
                        client.transcribe(
                            &provider,
                            Some(api_key),
                            &remote_model_id,
                            language,
                            audio_samples.as_slice(),
                        )
                    })
                    .await;

                    match join_result {
                        Ok(Ok(text)) => Ok(text),
                        Ok(Err(err)) => Err(classify_online_asr_attempt_error(&err.to_string())),
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

    let result = match outcome {
        crate::provider_gateway::ExecutionOutcome::Success(text) => {
            // Apply hotword force replacements on the raw online-ASR output
            // BEFORE returning, so every downstream consumer (DB, post-process,
            // review) sees the already-substituted text — matching the local
            // engine path which replaces inside TranscriptionManager.
            let replaced =
                crate::managers::hotword::apply_force_replacements_via_state(app_handle, text);
            Ok(replaced)
        }
        crate::provider_gateway::ExecutionOutcome::Fatal { detail, .. } => Err(anyhow!(detail)),
        crate::provider_gateway::ExecutionOutcome::Exhausted { last_error, .. } => {
            let detail = match last_error {
                crate::provider_gateway::AttemptError::Retryable { detail, .. }
                | crate::provider_gateway::AttemptError::Fatal { detail, .. } => detail,
            };
            Err(anyhow!(detail))
        }
    };

    let elapsed_ms = start.elapsed().as_millis() as i64;
    if let Some(ref metrics) = metrics {
        let (out_chars, err_msg) = match &result {
            Ok(text) => (text.chars().count() as i64, None),
            Err(e) => (0i64, Some(e.to_string())),
        };
        let speed = if elapsed_ms > 0 && out_chars > 0 {
            Some(out_chars as f64 / elapsed_ms as f64 * 1000.0)
        } else {
            None
        };
        let _ = metrics.log_call(&crate::managers::llm_metrics::LlmCallRecord {
            history_id,
            model_id: cached_id_for_log,
            provider: provider_id_for_log,
            call_type: "asr".to_string(),
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            token_estimate: Some(audio_ms as f64),
            duration_ms: elapsed_ms,
            tokens_per_sec: speed,
            error: err_msg,
            is_fallback,
        });
    }

    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScriptType {
    Latin,
    Han,
    Other,
}

#[derive(Clone, Debug)]
struct DiffToken {
    normalized: String,
    script: ScriptType,
}

fn detect_script(value: &str) -> ScriptType {
    if value
        .chars()
        .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch))
    {
        return ScriptType::Han;
    }
    if value.chars().any(|ch| ch.is_ascii_alphabetic()) {
        return ScriptType::Latin;
    }
    ScriptType::Other
}

fn tokenize_diff_words(text: &str) -> Vec<DiffToken> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ('\u{4e00}'..='\u{9fff}').contains(&ch) {
            if !current.is_empty() {
                let raw = std::mem::take(&mut current);
                tokens.push(DiffToken {
                    normalized: raw.to_lowercase(),
                    script: detect_script(&raw),
                });
            }
            let raw = ch.to_string();
            tokens.push(DiffToken {
                normalized: raw.clone(),
                script: detect_script(&raw),
            });
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                let raw = std::mem::take(&mut current);
                tokens.push(DiffToken {
                    normalized: raw.to_lowercase(),
                    script: detect_script(&raw),
                });
            }
            continue;
        }
        if ch.is_alphanumeric() {
            current.push(ch);
        } else {
            if !current.is_empty() {
                let raw = std::mem::take(&mut current);
                tokens.push(DiffToken {
                    normalized: raw.to_lowercase(),
                    script: detect_script(&raw),
                });
            }
            let raw = ch.to_string();
            tokens.push(DiffToken {
                normalized: raw.clone(),
                script: detect_script(&raw),
            });
        }
    }
    if !current.is_empty() {
        let raw = current;
        tokens.push(DiffToken {
            normalized: raw.to_lowercase(),
            script: detect_script(&raw),
        });
    }
    tokens
}

fn count_sentence_markers(text: &str) -> usize {
    let mut count = 0usize;
    for ch in text.chars() {
        if matches!(ch, '。' | '！' | '？' | '.' | '!' | '?') {
            count += 1;
        }
    }
    if count == 0 && !text.trim().is_empty() {
        1
    } else {
        count
    }
}

pub(crate) fn compute_change_percent(source: &str, target: &str) -> i8 {
    let source_tokens = tokenize_diff_words(source);
    let target_tokens = tokenize_diff_words(target);
    let source_len = source_tokens.len();
    let target_len = target_tokens.len();
    let max_len = source_len.max(target_len);
    if max_len == 0 {
        return 0;
    }

    let mut dp = vec![vec![0usize; target_len + 1]; source_len + 1];
    for i in 1..=source_len {
        for j in 1..=target_len {
            if source_tokens[i - 1].normalized == target_tokens[j - 1].normalized {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    let lcs = dp[source_len][target_len];
    let edits = max_len.saturating_sub(lcs);

    let mut i = source_len;
    let mut j = target_len;
    let mut pending_deletes: Vec<DiffToken> = Vec::new();
    let mut script_switches = 0usize;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && source_tokens[i - 1].normalized == target_tokens[j - 1].normalized {
            pending_deletes.clear();
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            if let Some(prev) = pending_deletes.pop() {
                let current = &target_tokens[j - 1];
                if prev.script != ScriptType::Other
                    && current.script != ScriptType::Other
                    && prev.script != current.script
                {
                    script_switches += 1;
                }
            }
            j -= 1;
        } else if i > 0 {
            pending_deletes.push(source_tokens[i - 1].clone());
            i -= 1;
        }
    }

    let token_change_rate = edits as f64 / max_len as f64;
    let script_switch_rate = script_switches as f64 / max_len as f64;
    let source_sentences = count_sentence_markers(source);
    let target_sentences = count_sentence_markers(target);
    let sentence_change_rate = if source_sentences == 0 && target_sentences == 0 {
        0.0
    } else {
        ((source_sentences as i64 - target_sentences as i64).unsigned_abs() as f64)
            / (source_sentences.max(target_sentences) as f64)
    };

    let score = 0.6 * token_change_rate + 0.3 * script_switch_rate + 0.1 * sentence_change_rate;
    let magnitude = (score * 100.0).round() as i32;
    let magnitude = magnitude.clamp(0, 100);
    // Negative when content was reduced
    if target.len() < source.len() {
        -(magnitude as i8)
    } else {
        magnitude as i8
    }
}

impl ShortcutAction for TranscribeAction {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        let start_time = Instant::now();
        debug!("TranscribeAction::start called for binding: {}", binding_id);

        let ppm = app.state::<Arc<crate::managers::post_processing::PostProcessingManager>>();
        ppm.cancel_current_task();

        let settings_for_load = get_settings(app);

        // Realtime preview should be available whenever the current recording flow
        // has a local model path that can produce partial text.
        // - Local ASR: always enable realtime preview
        // - Online ASR: enable only when a secondary local candidate is configured
        let enable_realtime = settings_for_load.realtime_transcription_enabled
            && (!settings_for_load.online_asr_enabled
                || settings_for_load.post_process_use_secondary_output);

        let rm = app.state::<Arc<AudioRecordingManager>>();

        if settings_for_load.online_asr_enabled && enable_realtime {
            // Online is primary but realtime preview wants a local model —
            // load the configured secondary, NOT `selected_model` (which may
            // point to an online model id or stale value in this mode).
            let secondary = settings_for_load
                .post_process_secondary_model_id
                .as_ref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());

            match secondary {
                Some(model_id) => {
                    log::info!(
                        "[Realtime] loading secondary local model '{}' for preview",
                        model_id
                    );
                    let tm = app.state::<Arc<TranscriptionManager>>();
                    tm.initiate_model_load_for(&model_id);
                }
                None => {
                    log::warn!(
                        "[Realtime] enabled but post_process_secondary_model_id is empty — preview will not render"
                    );
                }
            }
        } else if !settings_for_load.online_asr_enabled {
            let tm = app.state::<Arc<TranscriptionManager>>();
            tm.initiate_model_load();
        } else {
            debug!("Online ASR enabled and realtime preview off: skip preloading local model");
        }

        // Pre-load VAD in parallel with ASR model
        {
            let rm_clone = Arc::clone(&rm);
            std::thread::spawn(move || {
                if let Err(e) = rm_clone.preload_vad() {
                    log::debug!("VAD pre-load failed (will retry on stream open): {}", e);
                }
            });
        }

        let binding_id = binding_id.to_string();
        change_tray_icon(app, TrayIconState::Recording);
        if shortcut_str == "review-window-local" {
            let rewrite_count = crate::review_window::increment_rewrite_count();
            // Marks the entire rewrite lifecycle (recording + pipeline) so the
            // global ESC handler knows to spare the review window.
            crate::review_window::set_review_rewrite_in_progress(true);
            let _ = app.emit("review-rewrite-state", true);
            show_recording_overlay_rewrite(app, rewrite_count);
        } else {
            // A non-rewrite recording is taking over — clear any stale rewrite
            // flag left by a previous run that was preempted before its
            // FinishGuard could reset it.
            crate::review_window::set_review_rewrite_in_progress(false);
            let _ = app.emit("review-rewrite-state", false);
            show_recording_overlay(app);
        }

        // Pre-warm the review webview so its 484 KB bundle is already loaded by
        // the time recording finishes. First review of the day is otherwise
        // noticeably delayed by the cold start.
        crate::review_window::prewarm_review_window(app);

        // Setup channel for receiving audio frames for realtime simulation if using local model
        let (realtime_tx, realtime_rx) = std::sync::mpsc::channel::<Vec<f32>>();
        if enable_realtime {
            rm.set_speech_frame_sender(Some(realtime_tx));
        } else {
            rm.set_speech_frame_sender(None);
        }

        rm.set_online_transcription_receiver(None);
        let _mm = app.state::<Arc<ModelManager>>();

        let new_id = rm.increment_transcription_id();
        debug!("Starting new transcription session with ID: {}", new_id);

        let settings = get_settings(app);
        let is_always_on = settings.always_on_microphone;
        debug!("Microphone mode - always_on: {}", is_always_on);

        // Online ASR can optionally run a secondary local model for realtime captions and for
        // post-processing fusion input.
        let use_secondary_local_realtime =
            settings.online_asr_enabled && settings.post_process_use_secondary_output;
        let _secondary_local_model_id = if use_secondary_local_realtime {
            settings
                .post_process_secondary_model_id
                .clone()
                .filter(|id| !id.trim().is_empty())
                .unwrap_or_default()
        } else {
            String::new()
        };

        let mut recording_started = false;
        if is_always_on {
            let rm_clone = Arc::clone(&rm);
            let app_clone = app.clone();
            std::thread::spawn(move || {
                play_feedback_sound_blocking(&app_clone, SoundType::Start);
                rm_clone.apply_mute();
            });

            recording_started = rm.try_start_recording(&binding_id, 0);
            debug!("Recording started: {}", recording_started);
        } else {
            let recording_start_time = Instant::now();
            if rm.try_start_recording(&binding_id, 0) {
                recording_started = true;
                debug!("Recording started in {:?}", recording_start_time.elapsed());
                let app_clone = app.clone();
                let rm_clone = Arc::clone(&rm);
                std::thread::spawn(move || {
                    play_feedback_sound_blocking(&app_clone, SoundType::Start);
                    rm_clone.apply_mute();
                });
            } else {
                debug!("Failed to start recording");
            }
        }

        if recording_started {
            shortcut::register_cancel_shortcut(app);

            if enable_realtime {
                let tm_realtime = app.state::<Arc<TranscriptionManager>>().inner().clone();
                let app_handle_realtime = app.clone();
                let interval_ms = settings_for_load.offline_vad_force_interval_ms;
                let window_secs = settings_for_load.offline_vad_force_window_seconds;

                // Pre-load punct model in background so anchored punctuation is ready
                let tm_punct = tm_realtime.clone();
                std::thread::spawn(move || {
                    tm_punct.ensure_punct_model_loaded();
                });

                std::thread::spawn(move || {
                    realtime_worker_loop(
                        realtime_rx,
                        &tm_realtime,
                        &app_handle_realtime,
                        interval_ms,
                        window_secs,
                    );
                });
            }
        }

        debug!(
            "TranscribeAction::start completed in {:?}",
            start_time.elapsed()
        );
    }

    fn stop(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        let stop_time = Instant::now();
        debug!("TranscribeAction::stop called for binding: {}", binding_id);

        let ah = app.clone();
        let rm = Arc::clone(&app.state::<Arc<AudioRecordingManager>>());
        let tm = Arc::clone(&app.state::<Arc<TranscriptionManager>>());
        let hm = Arc::clone(&app.state::<Arc<HistoryManager>>());
        let ppm = Arc::clone(
            &app.state::<Arc<crate::managers::post_processing::PostProcessingManager>>(),
        );

        // Drop the speech frame sender so the realtime worker exits
        // before the final transcription grabs the engine.
        rm.set_speech_frame_sender(None);

        rm.remove_mute();
        play_feedback_sound(app, SoundType::Stop);

        let current_transcription_id = rm.get_current_transcription_id();
        let binding_id = binding_id.to_string();
        let shortcut_str = shortcut_str.to_string();
        let review_editor_active = shortcut_str == "review-window-local"
            && (crate::review_window::is_review_editor_active()
                || crate::review_window::is_review_window_active());
        let skill_mode = self.skill_mode;
        let ppm_outer = Arc::clone(&ppm);

        let pipeline_handle = tauri::async_runtime::spawn(async move {
            // RAII guard that fires on every exit path of this async block —
            // whether it returns normally, is aborted, or panics.
            struct FinishGuard {
                app: AppHandle,
                transcription_id: u64,
            }
            impl Drop for FinishGuard {
                fn drop(&mut self) {
                    let rm = self.app.state::<Arc<AudioRecordingManager>>();
                    if rm.get_current_transcription_id() == self.transcription_id {
                        shortcut::unregister_cancel_shortcut(&self.app);
                        // Mirror the shortcut-unregister gate: only this run's
                        // own end may clear the rewrite flag. If a newer run
                        // preempted us, it owns the flag now.
                        if crate::review_window::is_review_rewrite_in_progress() {
                            crate::review_window::set_review_rewrite_in_progress(false);
                            let _ = self.app.emit("review-rewrite-state", false);
                        }
                    }

                    // Always notify coordinator that processing is done
                    if let Some(coordinator) =
                        self.app.try_state::<crate::transcription_coordinator::TranscriptionCoordinator>()
                    {
                        coordinator.notify_processing_finished();
                    }

                    // Clean up UI state — keep overlay visible when skill confirmation is pending (Mode C)
                    let has_pending_skill = if let Some(state) =
                        self.app
                            .try_state::<crate::ManagedPendingSkillConfirmation>()
                    {
                        state.lock().map(|g| g.skill_id.is_some()).unwrap_or(false)
                    } else {
                        false
                    };

                    if !has_pending_skill {
                        utils::hide_recording_overlay(&self.app);
                    }
                    change_tray_icon(&self.app, TrayIconState::Idle);
                }
            }
            let _finish_guard = FinishGuard {
                app: ah.clone(),
                transcription_id: current_transcription_id,
            };

            debug!(
                "Starting async transcription task for binding: {} (ID: {})",
                binding_id, current_transcription_id
            );

            let stop_recording_time = Instant::now();
            if let Some(samples) = rm.stop_recording(&binding_id) {
                match classify_recording_samples(&samples) {
                    RecordingDisposition::IgnoredTooShort {
                        effective_duration_ms,
                    } => {
                        info!(
                            "[Transcribe] ignored/too_short: effective_duration_ms={} samples={}",
                            effective_duration_ms,
                            samples.len()
                        );
                        return;
                    }
                    RecordingDisposition::Proceed { .. } => {}
                }

                change_tray_icon(&ah, TrayIconState::Transcribing);
                // Rewrite count was already incremented in start()
                show_transcribing_overlay(&ah);

                debug!(
                    "Recording stopped and samples retrieved in {:?}, sample count: {}",
                    stop_recording_time.elapsed(),
                    samples.len()
                );

                let settings = get_settings(&ah);
                let duration_ms = effective_recording_duration_ms(&samples);

                let active_window_snapshot = active_window::fetch_active_window().ok();
                if let Some(info) = &active_window_snapshot {
                    debug!(
                        "Active window (snapshot): app='{}' title='{}' pid={} window_id={}",
                        info.app_name, info.title, info.process_id, info.window_id
                    );
                }
                let selected_text = crate::clipboard::get_selected_text(&ah).ok();
                // Cursor context: mutually exclusive with selected_text
                let cursor_context = if selected_text.is_some() {
                    None
                } else {
                    crate::clipboard::get_cursor_context(&ah).ok()
                };
                let review_document_text = if shortcut_str == "review-window-local" {
                    crate::review_window::take_frozen_review_editor_content()
                        .or_else(crate::review_window::current_review_editor_content)
                } else {
                    None
                };
                let votype_mode = if shortcut_str == "review-window-local" {
                    VotypeInputMode::ReviewRewrite
                } else {
                    crate::window_context::resolve_votype_input_mode(
                        active_window_snapshot
                            .as_ref()
                            .map(|info| info.app_name.as_str()),
                        active_window_snapshot
                            .as_ref()
                            .map(|info| info.title.as_str()),
                        review_editor_active,
                        selected_text
                            .as_ref()
                            .map(|text| !text.trim().is_empty())
                            .unwrap_or(false),
                    )
                };

                // Capture selected text for Mode C (auto-routing)
                // [DEBUG] Log selected text at recording stop
                match &selected_text {
                    Some(text) if !text.trim().is_empty() => {
                        let preview: String = text.chars().take(100).collect();
                        let suffix = if text.chars().count() > 100 {
                            "..."
                        } else {
                            ""
                        };
                        info!(
                            "[Selection] Recording stopped - captured {} chars: \"{}{}\"",
                            text.len(),
                            preview,
                            suffix
                        );
                    }
                    Some(_) => {
                        info!("[Selection] Recording stopped - captured text is empty/whitespace only");
                    }
                    None => {
                        info!("[Selection] Recording stopped - no text captured (get_selected_text returned None)");
                    }
                }

                // Pre-save audio with empty placeholder so the recording is preserved
                // even if the pipeline is aborted during transcription/post-processing.
                let asr_model_for_presave = if settings.online_asr_enabled {
                    settings
                        .selected_asr_model
                        .as_ref()
                        .map(|c| c.primary_id.clone())
                        .unwrap_or_else(|| "online".to_string())
                } else {
                    settings.selected_model.clone()
                };
                let streaming_asr_model_for_presave =
                    if settings.online_asr_enabled && settings.post_process_use_secondary_output {
                        settings
                            .post_process_secondary_model_id
                            .clone()
                            .or(Some(settings.selected_model.clone()))
                    } else if !settings.online_asr_enabled {
                        Some(settings.selected_model.clone())
                    } else {
                        None
                    };
                let presave_history_id = match hm
                    .save_transcription(
                        samples.clone(),
                        String::new(), // empty placeholder
                        None,
                        streaming_asr_model_for_presave,
                        None,
                        None,
                        None,
                        None,
                        Some(duration_ms),
                        None, // transcription_ms not known yet
                        Some(settings.selected_language.clone()),
                        Some(asr_model_for_presave),
                        active_window_snapshot
                            .as_ref()
                            .map(|info| info.app_name.clone()),
                        active_window_snapshot
                            .as_ref()
                            .map(|info| info.title.clone()),
                    )
                    .await
                {
                    Ok(id) => Some(id),
                    Err(e) => {
                        error!("Failed to pre-save audio to history: {}", e);
                        None
                    }
                };

                let transcription_time = Instant::now();
                // Streaming workers no longer exist without Sherpa
                let incremental_result: Option<String> = None;

                let use_parallel_online_secondary = settings.online_asr_enabled
                    && settings.post_process_enabled
                    && settings.post_process_use_secondary_output;

                // Always compute the final transcription from full audio on stop.
                // If online ASR is enabled, use OnlineAsrClient for the primary transcription.
                // If a secondary local candidate is also requested, run both concurrently.
                let (transcription_result, secondary_result): (
                    anyhow::Result<String>,
                    Option<String>,
                ) = if settings.online_asr_enabled {
                    // --- Online ASR with fallback chain support ---
                    // Grab metrics manager for per-call ASR logging
                    let asr_metrics: Option<std::sync::Arc<crate::managers::llm_metrics::LlmMetricsManager>> =
                        ah.try_state::<std::sync::Arc<crate::managers::llm_metrics::LlmMetricsManager>>()
                            .map(|m| (*m).clone());
                    let asr_history_id = presave_history_id;
                    let asr_audio_duration_ms = duration_ms;
                    let asr_app_handle = ah.clone();

                    // Helper: spawn an online ASR task for a given cached_model_id.
                    // Each task self-logs its metrics (timing, chars, error) so that
                    // every participant in Race/Staggered/Serial is individually recorded.
                    let spawn_asr_task = move |cached_model_id: &str,
                                               settings: &crate::settings::AppSettings,
                                               audio_samples: Vec<f32>,
                                               lang: String,
                                               is_fallback: bool,
                                               metrics: Option<
                        std::sync::Arc<crate::managers::llm_metrics::LlmMetricsManager>,
                    >,
                                               history_id: Option<i64>,
                                               audio_ms: i64|
                          -> Option<
                        tokio::task::JoinHandle<anyhow::Result<String>>,
                    > {
                        let cached = settings
                            .cached_models
                            .iter()
                            .find(|m| {
                                m.model_type == crate::settings::ModelType::Asr
                                    && m.id == cached_model_id
                            })?
                            .clone();
                        let app_handle = asr_app_handle.clone();
                        let settings = settings.clone();

                        Some(tokio::spawn(async move {
                            execute_online_asr_request(
                                &app_handle,
                                &settings,
                                &cached,
                                audio_samples,
                                lang,
                                is_fallback,
                                metrics,
                                history_id,
                                audio_ms,
                            )
                            .await
                        }))
                    };

                    let chain = settings.selected_asr_model.clone();
                    let primary_model_id = chain
                        .as_ref()
                        .map(|c| c.primary_id.clone())
                        .unwrap_or_else(|| "online".to_string());
                    let fallback_model_id = chain.as_ref().and_then(|c| c.fallback_id.clone());
                    let strategy = chain
                        .as_ref()
                        .map(|c| c.strategy.clone())
                        .unwrap_or(crate::fallback::ModelChainStrategy::Serial);

                    let language = settings.selected_language.clone();

                    // Spawn primary ASR task
                    let primary_handle = if let Some(h) = spawn_asr_task(
                        &primary_model_id,
                        &settings,
                        samples.clone(),
                        language.clone(),
                        false,
                        asr_metrics.clone(),
                        asr_history_id,
                        asr_audio_duration_ms,
                    ) {
                        h
                    } else {
                        // No cached model found, fall back to local
                        let tm_fallback = Arc::clone(&tm);
                        let samples_for_local = samples.clone();
                        tokio::task::spawn_blocking(move || {
                            tm_fallback.transcribe(samples_for_local)
                        })
                    };

                    // Apply fallback strategy: resolve primary_handle into a single result
                    // When race/staggered is used, fallback is already handled — skip downstream serial fallback
                    let mut fallback_already_handled = false;
                    let primary_handle = if let Some(ref fb_id) = fallback_model_id {
                        match strategy {
                            crate::fallback::ModelChainStrategy::Race => {
                                // Race: spawn fallback immediately, select first success
                                log::info!("[ASR] Race mode: {} vs {}", primary_model_id, fb_id);
                                fallback_already_handled = true;
                                if let Some(fallback_handle) = spawn_asr_task(
                                    fb_id,
                                    &settings,
                                    samples.clone(),
                                    language.clone(),
                                    true,
                                    asr_metrics.clone(),
                                    asr_history_id,
                                    asr_audio_duration_ms,
                                ) {
                                    let primary_id_log = primary_model_id.clone();
                                    let fb_id_log = fb_id.clone();
                                    let audio_secs = samples.len() as f64 / 16000.0;
                                    tokio::spawn(async move {
                                        let mut p = primary_handle;
                                        let mut f = fallback_handle;

                                        // Quality gate: for long audio (>15s), expect at least ~2 chars/sec.
                                        // If the race winner returns suspiciously short text, wait for the
                                        // other model and pick the longer result.
                                        let min_expected_chars = if audio_secs > 15.0 {
                                            (audio_secs * 2.0) as usize
                                        } else {
                                            0
                                        };

                                        tokio::select! {
                                            p_res = &mut p => {
                                                let res = p_res.unwrap_or_else(|e| Err(anyhow::anyhow!("{}", e)));
                                                if let Ok(ref text) = res {
                                                    let char_count = text.chars().count();
                                                    if min_expected_chars == 0 || char_count >= min_expected_chars {
                                                        log::info!("[ASR] Race winner: primary ({}, {} chars)", primary_id_log, char_count);
                                                        return res;
                                                    }
                                                    log::warn!(
                                                        "[ASR] Race: primary ({}) text too short ({} chars < {} min for {:.0}s audio), waiting for fallback",
                                                        primary_id_log, char_count, min_expected_chars, audio_secs
                                                    );
                                                    if let Ok(Ok(fb_text)) = f.await {
                                                        if fb_text.chars().count() > char_count {
                                                            log::info!(
                                                                "[ASR] Race: fallback ({}) longer ({} > {} chars), using fallback",
                                                                fb_id_log, fb_text.chars().count(), char_count
                                                            );
                                                            return Ok(fb_text);
                                                        }
                                                    }
                                                    log::info!("[ASR] Race: fallback unavailable or shorter, using primary ({})", primary_id_log);
                                                    return res;
                                                }
                                                log::warn!("[ASR] Race: primary failed, waiting for fallback");
                                                f.await.unwrap_or_else(|e| Err(anyhow::anyhow!("{}", e)))
                                            }
                                            f_res = &mut f => {
                                                let res = f_res.unwrap_or_else(|e| Err(anyhow::anyhow!("{}", e)));
                                                if let Ok(ref text) = res {
                                                    let char_count = text.chars().count();
                                                    if min_expected_chars == 0 || char_count >= min_expected_chars {
                                                        log::info!("[ASR] Race winner: fallback ({}, {} chars)", fb_id_log, char_count);
                                                        return res;
                                                    }
                                                    log::warn!(
                                                        "[ASR] Race: fallback ({}) text too short ({} chars < {} min for {:.0}s audio), waiting for primary",
                                                        fb_id_log, char_count, min_expected_chars, audio_secs
                                                    );
                                                    if let Ok(Ok(p_text)) = p.await {
                                                        if p_text.chars().count() > char_count {
                                                            log::info!(
                                                                "[ASR] Race: primary ({}) longer ({} > {} chars), using primary",
                                                                primary_id_log, p_text.chars().count(), char_count
                                                            );
                                                            return Ok(p_text);
                                                        }
                                                    }
                                                    log::info!("[ASR] Race: primary unavailable or shorter, using fallback ({})", fb_id_log);
                                                    return res;
                                                }
                                                log::warn!("[ASR] Race: fallback failed, waiting for primary");
                                                p.await.unwrap_or_else(|e| Err(anyhow::anyhow!("{}", e)))
                                            }
                                        }
                                    })
                                } else {
                                    primary_handle
                                }
                            }
                            crate::fallback::ModelChainStrategy::Staggered => {
                                // Staggered: wait 2s for primary, then also start fallback
                                log::info!(
                                    "[ASR] Staggered mode: {} (2s delay before {})",
                                    primary_model_id,
                                    fb_id
                                );
                                fallback_already_handled = true;
                                let fb_id_owned = fb_id.clone();
                                let settings_clone = settings.clone();
                                let samples_clone = samples.clone();
                                let language_clone = language.clone();
                                let primary_id_log = primary_model_id.clone();
                                let stg_fb_id_log = fb_id.clone();
                                let stg_metrics = asr_metrics.clone();
                                let stg_history_id = asr_history_id;
                                let stg_audio_ms = asr_audio_duration_ms;
                                let stg_audio_secs = samples.len() as f64 / 16000.0;
                                let stg_app_handle = ah.clone();
                                let stg_samples_for_debug = samples.clone();
                                tokio::spawn(async move {
                                    let mut p = primary_handle;

                                    // Relaxed heuristic: only flag "suspiciously short" when
                                    // audio is long enough AND text is under ~1 char/sec.
                                    // Previous threshold (2x) fired on normal English speech.
                                    let min_expected_chars = if stg_audio_secs > 30.0 {
                                        stg_audio_secs as usize
                                    } else {
                                        0
                                    };

                                    // Cap on how long we'll block for a second candidate after
                                    // the first one arrives — prevents the outer 30s timeout
                                    // from being our only safety net.
                                    const SECOND_CANDIDATE_WAIT: Duration = Duration::from_secs(3);

                                    // Shared diagnostic helper: emit a side-by-side comparison
                                    // log and persist the source audio for offline replay.
                                    let diag =
                                        |primary_text: &str, fallback_text: &str, reason: &str| {
                                            let p_cc = primary_text.chars().count();
                                            let f_cc = fallback_text.chars().count();
                                            log::warn!(
                                            "[ASR-diag] {} audio={:.1}s primary={} chars={} fallback={} chars={}",
                                            reason,
                                            stg_audio_secs,
                                            primary_id_log,
                                            p_cc,
                                            stg_fb_id_log,
                                            f_cc,
                                        );
                                            log::warn!(
                                                "[ASR-diag] primary_text={:?}",
                                                primary_text
                                            );
                                            log::warn!(
                                                "[ASR-diag] fallback_text={:?}",
                                                fallback_text
                                            );
                                            if let Some(path) = save_debug_audio(
                                                &stg_app_handle,
                                                &stg_samples_for_debug,
                                                &format!("staggered-{}", reason),
                                            ) {
                                                log::warn!(
                                                    "[ASR-diag] saved audio: {}",
                                                    path.display()
                                                );
                                            }
                                        };

                                    // Wait for primary or stagger delay
                                    tokio::select! {
                                        p_res = &mut p => {
                                            let res = p_res.unwrap_or_else(|e| Err(anyhow::anyhow!("{}", e)));
                                            if res.is_ok() {
                                                return res;
                                            }
                                            // Primary failed fast, start fallback
                                            log::warn!("[ASR] Staggered: primary failed fast, starting fallback");
                                            if let Some(fh) = spawn_asr_task(&fb_id_owned, &settings_clone, samples_clone, language_clone, true, stg_metrics, stg_history_id, stg_audio_ms) {
                                                return fh.await.unwrap_or_else(|e| Err(anyhow::anyhow!("{}", e)));
                                            }
                                            res
                                        }
                                        _ = tokio::time::sleep(Duration::from_millis(crate::fallback::STAGGERED_DELAY_MS)) => {
                                            // Delay elapsed, start fallback and race
                                            log::info!("[ASR] Staggered: primary {} slow, starting fallback {}", primary_id_log, fb_id_owned);
                                            if let Some(mut f) = spawn_asr_task(&fb_id_owned, &settings_clone, samples_clone, language_clone, true, stg_metrics, stg_history_id, stg_audio_ms) {
                                                tokio::select! {
                                                    p_res = &mut p => {
                                                        let res = p_res.unwrap_or_else(|e| Err(anyhow::anyhow!("{}", e)));
                                                        if let Ok(ref text) = res {
                                                            let cc = text.chars().count();
                                                            if min_expected_chars == 0 || cc >= min_expected_chars {
                                                                return res;
                                                            }
                                                            log::warn!("[ASR] Staggered: primary text short ({} < {}), waiting up to {}s for fallback", cc, min_expected_chars, SECOND_CANDIDATE_WAIT.as_secs());
                                                            match timeout(SECOND_CANDIDATE_WAIT, f).await {
                                                                Ok(join_res) => {
                                                                    if let Ok(Ok(fb_text)) = join_res {
                                                                        diag(text, &fb_text, "primary-short");
                                                                        if fb_text.chars().count() > cc {
                                                                            log::info!("[ASR] Staggered: using fallback ({} > {} chars)", fb_text.chars().count(), cc);
                                                                            return Ok(fb_text);
                                                                        }
                                                                    }
                                                                }
                                                                Err(_) => {
                                                                    log::warn!("[ASR] Staggered: fallback wait timed out, keeping primary");
                                                                    diag(text, "", "primary-short-fallback-timeout");
                                                                }
                                                            }
                                                            return res;
                                                        }
                                                        match timeout(SECOND_CANDIDATE_WAIT, f).await {
                                                            Ok(join_res) => join_res.unwrap_or_else(|e| Err(anyhow::anyhow!("{}", e))),
                                                            Err(_) => {
                                                                log::warn!("[ASR] Staggered: primary failed and fallback wait timed out");
                                                                Err(anyhow::anyhow!("Staggered: both candidates unavailable"))
                                                            }
                                                        }
                                                    }
                                                    f_res = &mut f => {
                                                        let res = f_res.unwrap_or_else(|e| Err(anyhow::anyhow!("{}", e)));
                                                        if let Ok(ref text) = res {
                                                            let cc = text.chars().count();
                                                            if min_expected_chars == 0 || cc >= min_expected_chars {
                                                                return res;
                                                            }
                                                            log::warn!("[ASR] Staggered: fallback ({}) text short ({} < {}), waiting up to {}s for primary", stg_fb_id_log, cc, min_expected_chars, SECOND_CANDIDATE_WAIT.as_secs());
                                                            match timeout(SECOND_CANDIDATE_WAIT, p).await {
                                                                Ok(join_res) => {
                                                                    if let Ok(Ok(p_text)) = join_res {
                                                                        diag(&p_text, text, "fallback-short");
                                                                        if p_text.chars().count() > cc {
                                                                            log::info!("[ASR] Staggered: using primary ({} > {} chars)", p_text.chars().count(), cc);
                                                                            return Ok(p_text);
                                                                        }
                                                                    }
                                                                }
                                                                Err(_) => {
                                                                    log::warn!("[ASR] Staggered: primary wait timed out, keeping fallback");
                                                                    diag("", text, "fallback-short-primary-timeout");
                                                                }
                                                            }
                                                            return res;
                                                        }
                                                        match timeout(SECOND_CANDIDATE_WAIT, p).await {
                                                            Ok(join_res) => join_res.unwrap_or_else(|e| Err(anyhow::anyhow!("{}", e))),
                                                            Err(_) => {
                                                                log::warn!("[ASR] Staggered: fallback failed and primary wait timed out");
                                                                Err(anyhow::anyhow!("Staggered: both candidates unavailable"))
                                                            }
                                                        }
                                                    }
                                                }
                                            } else {
                                                p.await.unwrap_or_else(|e| Err(anyhow::anyhow!("{}", e)))
                                            }
                                        }
                                    }
                                })
                            }
                            crate::fallback::ModelChainStrategy::Serial => {
                                // Serial: handled later in timeout paths (existing inline fallback)
                                primary_handle
                            }
                        }
                    } else {
                        primary_handle
                    };

                    // --- Optional secondary local candidate ---
                    let secondary_handle = if use_parallel_online_secondary {
                        let incremental_for_secondary = incremental_result.clone();
                        let model_id_for_secondary = settings
                            .post_process_secondary_model_id
                            .as_ref()
                            .filter(|id| !id.trim().is_empty())
                            .cloned()
                            .unwrap_or_default();
                        let tm_for_secondary = Arc::clone(&tm);
                        let samples_for_secondary = samples.clone();

                        Some(tokio::task::spawn_blocking(move || -> Option<String> {
                            if let Some(text) = incremental_for_secondary {
                                if !text.trim().is_empty() {
                                    return Some(text);
                                }
                            }

                            if model_id_for_secondary.trim().is_empty() {
                                return None;
                            }

                            // Only load if a different model is needed; skip if already loaded
                            let needs_load = tm_for_secondary
                                .get_current_model()
                                .map_or(true, |current| current != model_id_for_secondary);
                            if needs_load {
                                if let Err(e) = tm_for_secondary.load_model(&model_id_for_secondary)
                                {
                                    log::warn!(
                                        "Failed to load secondary model '{}': {}",
                                        model_id_for_secondary,
                                        e
                                    );
                                    return None;
                                }
                            }

                            match tm_for_secondary.transcribe(samples_for_secondary) {
                                Ok(local_text) => {
                                    if local_text.trim().is_empty() {
                                        None
                                    } else {
                                        Some(local_text)
                                    }
                                }
                                Err(e) => {
                                    log::warn!(
                                        "Failed to compute secondary local candidate: {}",
                                        e
                                    );
                                    None
                                }
                            }
                        }))
                    } else {
                        None
                    };

                    // Cache model info for potential retry in online-only timeout path
                    let cached_model_for_retry = settings
                        .cached_models
                        .iter()
                        .find(|m| {
                            m.model_type == crate::settings::ModelType::Asr
                                && settings
                                    .selected_asr_model
                                    .as_ref()
                                    .map(|c| c.primary_id.as_str())
                                    == Some(m.id.as_str())
                        })
                        .cloned();

                    // Determine if we have a local fallback running in parallel
                    let has_local_fallback = secondary_handle.is_some();
                    // 10s chosen as reasonable upper bound for network ASR latency;
                    // the internal HTTP client timeout is 120s.
                    const ONLINE_TIMEOUT_WITH_LOCAL: u64 = 10;

                    if has_local_fallback {
                        // Race: wait up to 10s for online, otherwise use local
                        let secondary_handle = secondary_handle.unwrap();

                        match timeout(
                            Duration::from_secs(ONLINE_TIMEOUT_WITH_LOCAL),
                            primary_handle,
                        )
                        .await
                        {
                            Ok(join_result) => {
                                // Online finished within timeout
                                let mut primary = match join_result {
                                    Ok(res) => res,
                                    Err(e) => Err(anyhow::anyhow!("Online ASR task failed: {}", e)),
                                };

                                // If primary failed and fallback not already handled (race/staggered), try serial fallback
                                if primary.is_err() && !fallback_already_handled {
                                    if let Some(fallback_id) = settings
                                        .selected_asr_model
                                        .as_ref()
                                        .and_then(|c| c.fallback_id.as_ref())
                                    {
                                        log::warn!("[ASR] Primary online model failed, trying fallback: {}", fallback_id);

                                        let fallback_cached = settings
                                            .cached_models
                                            .iter()
                                            .find(|m| {
                                                m.model_type == crate::settings::ModelType::Asr
                                                    && m.id == *fallback_id
                                            })
                                            .cloned();

                                        if let Some(fallback_model) = fallback_cached {
                                            match execute_online_asr_request(
                                                &ah,
                                                &settings,
                                                &fallback_model,
                                                samples.clone(),
                                                settings.selected_language.clone(),
                                                true,
                                                asr_metrics.clone(),
                                                asr_history_id,
                                                asr_audio_duration_ms,
                                            )
                                            .await
                                            {
                                                Ok(text) => {
                                                    log::info!(
                                                        "[ASR] Fallback online model succeeded"
                                                    );
                                                    primary = Ok(text);
                                                }
                                                Err(e) => log::warn!(
                                                    "[ASR] Fallback online model also failed: {}",
                                                    e
                                                ),
                                            }
                                        }
                                    }
                                }

                                // Still await secondary (it's likely done or almost done)
                                let secondary = match secondary_handle.await {
                                    Ok(res) => res,
                                    Err(e) => {
                                        log::warn!("Secondary transcription task failed: {}", e);
                                        None
                                    }
                                };
                                (primary, secondary)
                            }
                            Err(_elapsed) => {
                                // Online timed out — use local result.
                                // Note: dropping primary_handle does NOT cancel the blocking thread;
                                // the online request continues until its internal 120s HTTP timeout.
                                log::warn!(
                                    "[ASR] Online timeout ({}s), using local result",
                                    ONLINE_TIMEOUT_WITH_LOCAL
                                );
                                // Await the secondary (local) result — local transcription is expected
                                // to be fast (sub-second) so no additional timeout is applied here.
                                let secondary = match secondary_handle.await {
                                    Ok(res) => res,
                                    Err(e) => {
                                        log::warn!("Secondary transcription task failed: {}", e);
                                        None
                                    }
                                };
                                // Return online as error so fallback logic picks up secondary
                                (
                                    Err(anyhow::anyhow!(
                                        "Online ASR timed out after {}s, fell back to local",
                                        ONLINE_TIMEOUT_WITH_LOCAL
                                    )),
                                    secondary,
                                )
                            }
                        }
                    } else {
                        // No local fallback — online-only mode.
                        // If Race/Staggered already spawned an online fallback alongside the primary,
                        // the outer budget can be tight (10s). Otherwise fall back to the 30s prompt.
                        const ONLINE_TIMEOUT_NO_LOCAL: u64 = 30;
                        const ONLINE_TIMEOUT_WITH_PARALLEL_FALLBACK: u64 = 10;
                        let online_timeout_secs = if fallback_already_handled {
                            ONLINE_TIMEOUT_WITH_PARALLEL_FALLBACK
                        } else {
                            ONLINE_TIMEOUT_NO_LOCAL
                        };

                        match timeout(Duration::from_secs(online_timeout_secs), primary_handle)
                            .await
                        {
                            Ok(join_result) => {
                                let mut primary = match join_result {
                                    Ok(res) => res,
                                    Err(e) => Err(anyhow::anyhow!("Online ASR task failed: {}", e)),
                                };

                                // If primary failed, try fallback online model
                                if primary.is_err() {
                                    if let Some(fallback_id) = settings
                                        .selected_asr_model
                                        .as_ref()
                                        .and_then(|c| c.fallback_id.as_ref())
                                    {
                                        log::warn!("[ASR] Primary online model failed, trying fallback: {}", fallback_id);

                                        let fallback_cached = settings
                                            .cached_models
                                            .iter()
                                            .find(|m| {
                                                m.model_type == crate::settings::ModelType::Asr
                                                    && m.id == *fallback_id
                                            })
                                            .cloned();

                                        if let Some(fallback_model) = fallback_cached {
                                            match execute_online_asr_request(
                                                &ah,
                                                &settings,
                                                &fallback_model,
                                                samples.clone(),
                                                settings.selected_language.clone(),
                                                true,
                                                asr_metrics.clone(),
                                                asr_history_id,
                                                asr_audio_duration_ms,
                                            )
                                            .await
                                            {
                                                Ok(text) => {
                                                    log::info!(
                                                        "[ASR] Fallback online model succeeded"
                                                    );
                                                    primary = Ok(text);
                                                }
                                                Err(e) => log::warn!(
                                                    "[ASR] Fallback online model also failed: {}",
                                                    e
                                                ),
                                            }
                                        }
                                    }
                                }

                                (primary, None)
                            }
                            Err(_elapsed) => {
                                log::warn!(
                                    "[ASR] Online-only ASR timed out after {}s, prompting user",
                                    online_timeout_secs
                                );
                                if let Some(path) =
                                    save_debug_audio(&ah, &samples, "online-only-timeout")
                                {
                                    log::warn!(
                                        "[ASR-diag] saved audio on outer timeout: {}",
                                        path.display()
                                    );
                                }
                                let _ = ah.emit(
                                    "asr-online-timeout",
                                    serde_json::json!({
                                        "has_local_fallback": false
                                    }),
                                );

                                // Wait for user decision via oneshot channel
                                let (tx, rx) = tokio::sync::oneshot::channel::<String>();
                                {
                                    let state = ah.state::<crate::AsrTimeoutResponseSender>();
                                    let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
                                    *guard = Some(tx);
                                }

                                // Give user up to 120s to respond; auto-cancel if no response
                                match timeout(Duration::from_secs(120), rx).await {
                                    Err(_elapsed) => {
                                        log::warn!("[ASR] User did not respond to timeout prompt within 120s, auto-cancelling");
                                        (
                                            Err(anyhow::anyhow!("ASR timeout: no user response")),
                                            None,
                                        )
                                    }
                                    Ok(channel_result) => {
                                        match channel_result {
                                            Ok(action) => match action.as_str() {
                                                "continue" | "retry" => {
                                                    log::info!(
                                                        "[ASR] User chose to {} online ASR",
                                                        action
                                                    );
                                                    // Re-send the online ASR request (original handle was consumed by timeout)
                                                    if let Some(cached) =
                                                        cached_model_for_retry.as_ref()
                                                    {
                                                        let primary = execute_online_asr_request(
                                                            &ah,
                                                            &settings,
                                                            cached,
                                                            samples.clone(),
                                                            settings.selected_language.clone(),
                                                            false,
                                                            asr_metrics.clone(),
                                                            asr_history_id,
                                                            asr_audio_duration_ms,
                                                        )
                                                        .await;
                                                        (primary, None)
                                                    } else {
                                                        (
                                                            Err(anyhow::anyhow!(
                                                                "No cached model for retry"
                                                            )),
                                                            None,
                                                        )
                                                    }
                                                }
                                                _ => {
                                                    // "cancel" or unknown
                                                    log::info!("[ASR] User cancelled online ASR");
                                                    (
                                                        Err(anyhow::anyhow!(
                                                            "User cancelled online ASR"
                                                        )),
                                                        None,
                                                    )
                                                }
                                            },
                                            Err(_) => {
                                                log::warn!(
                                            "[ASR] Timeout response channel dropped, cancelling"
                                        );
                                                (
                                                    Err(anyhow::anyhow!(
                                                        "ASR timeout response channel dropped"
                                                    )),
                                                    None,
                                                )
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    (tm.transcribe(samples.clone()), None)
                };

                let mut primary_error: Option<String> = None;
                let mut primary_text: Option<String> = match transcription_result {
                    Ok(text) => {
                        let trimmed = text.trim().to_string();
                        if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed)
                        }
                    }
                    Err(err) => {
                        primary_error = Some(err.to_string());
                        None
                    }
                };

                // === Long-audio cross-check (A+B) ============================
                // For audio >30s, GLM-ASR-class providers have been observed
                // returning truncated content. We:
                //   A) Unconditionally log primary vs. local-fusion side-by-side
                //      and save the source WAV for offline replay.
                //   B) If local-fusion is significantly longer than primary
                //      (≥1.3x chars), swap primary_text to the local-fusion
                //      candidate — primary's content was very likely truncated.
                let audio_secs_for_check = samples.len() as f64 / 16000.0;
                if audio_secs_for_check > 30.0 {
                    let primary_for_log = primary_text.as_deref().unwrap_or("");
                    let secondary_for_log =
                        secondary_result.as_deref().map(|s| s.trim()).unwrap_or("");
                    let p_cc = primary_for_log.chars().count();
                    let s_cc = secondary_for_log.chars().count();

                    log::info!(
                        "[ASR-longcheck] audio={:.1}s primary_chars={} secondary_chars={}",
                        audio_secs_for_check,
                        p_cc,
                        s_cc
                    );
                    log::info!("[ASR-longcheck] primary_text={:?}", primary_for_log);
                    log::info!("[ASR-longcheck] secondary_text={:?}", secondary_for_log);
                    if let Some(path) = save_debug_audio(&ah, &samples, "longaudio-crosscheck") {
                        log::info!("[ASR-longcheck] saved audio: {}", path.display());
                    }

                    // B: prefer the substantially-longer candidate.
                    // Trigger only when primary is non-empty (avoid masking
                    // pure failure cases — those have their own fallback path).
                    if p_cc > 0 && s_cc as f64 > (p_cc as f64) * 1.3 {
                        log::warn!(
                            "[ASR-longcheck] primary likely truncated ({} chars) — swapping in local-fusion ({} chars, +{:.0}%)",
                            p_cc,
                            s_cc,
                            (s_cc as f64 / p_cc as f64 - 1.0) * 100.0
                        );
                        primary_text = Some(secondary_for_log.to_string());
                    }
                }
                // ================================================================

                // Fallback priority:
                // 1) primary transcription (online or local)
                // 2) secondary local transcription result (if enabled/available)
                // 3) incremental worker snapshot (if available)
                if primary_text.is_none() {
                    primary_text = secondary_result
                        .clone()
                        .filter(|t| !t.trim().is_empty())
                        .or_else(|| incremental_result.clone().filter(|t| !t.trim().is_empty()));
                }

                // Last-resort: if online ASR failed and we have any local model id, try a local-only transcription.
                if primary_text.is_none() && settings.online_asr_enabled {
                    let model_id = settings
                        .post_process_secondary_model_id
                        .as_ref()
                        .filter(|id| !id.trim().is_empty())
                        .cloned()
                        .unwrap_or_else(|| settings.selected_model.clone());
                    if !model_id.trim().is_empty() {
                        let tm_fallback = Arc::clone(&tm);
                        let samples_fallback = samples.clone();
                        match tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                            tm_fallback.load_model(&model_id)?;
                            tm_fallback.transcribe(samples_fallback)
                        })
                        .await
                        {
                            Ok(Ok(local_text)) => {
                                if !local_text.trim().is_empty() {
                                    primary_text = Some(local_text.trim().to_string());
                                }
                            }
                            Ok(Err(e)) => {
                                log::warn!("Local fallback transcription failed: {}", e);
                            }
                            Err(e) => {
                                log::warn!("Local fallback task failed: {}", e);
                            }
                        }
                    }
                }

                let transcription_ms = transcription_time.elapsed().as_millis() as i64;
                let _streaming_text = secondary_result
                    .clone()
                    .or_else(|| incremental_result.clone())
                    .filter(|t| !t.trim().is_empty());

                // Log ASR call metrics for local ASR only
                // (online ASR calls are self-logged inside spawn_asr_task / inline fallback)
                if !settings.online_asr_enabled {
                    let output_chars = primary_text
                        .as_ref()
                        .map(|t| t.chars().count() as i64)
                        .unwrap_or(0);
                    let chars_per_sec = if transcription_ms > 0 && output_chars > 0 {
                        Some(output_chars as f64 / transcription_ms as f64 * 1000.0)
                    } else {
                        None
                    };

                    if let Some(metrics) = ah
                        .try_state::<std::sync::Arc<crate::managers::llm_metrics::LlmMetricsManager>>(
                        )
                    {
                        if let Err(e) = metrics.log_call(
                            &crate::managers::llm_metrics::LlmCallRecord {
                                history_id: presave_history_id,
                                model_id: settings.selected_model.clone(),
                                provider: "local".to_string(),
                                call_type: "asr".to_string(),
                                input_tokens: None,
                                output_tokens: None,
                                total_tokens: None,
                                token_estimate: Some(duration_ms as f64),
                                duration_ms: transcription_ms,
                                tokens_per_sec: chars_per_sec,
                                error: primary_error.clone(),
                                is_fallback: false,
                            },
                        ) {
                            log::error!("[LlmMetrics] Failed to log ASR call: {}", e);
                        }
                    }
                }

                if let Some(transcription) = primary_text.clone() {
                    debug!(
                        "Transcription completed in {:?}: '{}'",
                        transcription_time.elapsed(),
                        transcription
                    );

                    let transcription_clone = transcription.clone();

                    let asr_model = if settings.online_asr_enabled {
                        settings
                            .selected_asr_model
                            .as_ref()
                            .map(|c| c.primary_id.clone())
                            .unwrap_or_else(|| "online".to_string())
                    } else {
                        settings.selected_model.clone()
                    };

                    // Update the pre-saved placeholder with actual transcription content
                    let history_id = if let Some(pid) = presave_history_id {
                        let char_count = transcription.chars().count() as i64;
                        if let Err(e) = hm
                            .update_transcription_content(
                                pid,
                                transcription.clone(),
                                asr_model,
                                settings.selected_language.clone(),
                                duration_ms,
                                transcription_ms,
                                char_count,
                            )
                            .await
                        {
                            error!("Failed to update transcription content: {}", e);
                        }
                        Some(pid)
                    } else {
                        None
                    };

                    if rm.get_current_transcription_id() != current_transcription_id {
                        info!(
                                    "New recording started during transcription (ID mismatch: {} != {}). Skipping paste/post-processing.",
                                    rm.get_current_transcription_id(),
                                    current_transcription_id
                                );
                        return;
                    }

                    if settings.post_process_enabled {
                        let ah_clone = ah.clone();
                        let settings_clone = settings.clone();
                        let rm_clone = Arc::clone(&rm);
                        let hm_clone = Arc::clone(&hm);
                        let secondary_result_for_post = secondary_result.clone();
                        let incremental_result_for_post = incremental_result.clone();

                        let mut active_window_snapshot_for_review = active_window_snapshot.clone();
                        if shortcut_str == "review-window-local" {
                            if let Some(info) = active_window_snapshot_for_review.as_mut() {
                                info.app_name = "Votype".to_string();
                                info.title = "Votype Review".to_string();
                            } else {
                                active_window_snapshot_for_review =
                                    Some(crate::active_window::ActiveWindowInfo {
                                        title: "Votype Review".to_string(),
                                        app_name: "Votype".to_string(),
                                        window_id: "review_window".to_string(),
                                        process_id: std::process::id() as u64,
                                        process_path: String::new(),
                                        position: crate::active_window::WindowPosition {
                                            x: 0.0,
                                            y: 0.0,
                                            width: 0.0,
                                            height: 0.0,
                                        },
                                    });
                            }
                        }
                        let task = tokio::spawn(async move {
                            let mut final_text = transcription_clone.clone();
                            let mut post_process_prompt_text = String::new();
                            let mut post_process_prompt_name = String::new();
                            let mut post_process_prompt_id: Option<String> = None;
                            let mut used_model: Option<String> = None;
                            let mut error_shown = false;

                            // 1. Try Chinese variant conversion first
                            let mut chinese_converted_text = transcription_clone.clone();
                            if let Some(converted_text) =
                                maybe_convert_chinese_variant(&settings_clone, &transcription_clone)
                                    .await
                            {
                                final_text = converted_text.clone();
                                chinese_converted_text = converted_text;
                                used_model = Some("OpenCC".to_string());
                            }

                            // 2. Apply LLM post-processing if enabled
                            {
                                let secondary = if settings_clone.post_process_use_secondary_output
                                {
                                    let mut secondary = secondary_result_for_post
                                        .clone()
                                        .or_else(|| incremental_result_for_post.clone());
                                    if secondary.is_none() {
                                        secondary = Some(transcription_clone.clone());
                                    }
                                    secondary
                                } else {
                                    None
                                };

                                // Lookup profile for the active app with title rule matching
                                let (app_profile, matched_rule) = active_window_snapshot_for_review
                                    .as_ref()
                                    .map(|info| {
                                        // Find profile for this app (case-insensitive)
                                        let profile_id = settings_clone
                                            .app_to_profile
                                            .iter()
                                            .find(|(k, _)| k.eq_ignore_ascii_case(&info.app_name))
                                            .map(|(_, v)| v);
                                        let profile = profile_id.and_then(|pid| {
                                            settings_clone
                                                .app_profiles
                                                .iter()
                                                .find(|p| &p.id == pid)
                                        });

                                        if let Some(p) = profile {
                                            // Find all matching rules
                                            let mut matched_rules = Vec::new();

                                            for rule in &p.rules {
                                                let matched = match rule.match_type {
                                                    crate::settings::TitleMatchType::Text => info
                                                        .title
                                                        .to_lowercase()
                                                        .contains(&rule.pattern.to_lowercase()),
                                                    crate::settings::TitleMatchType::Regex => {
                                                        regex::Regex::new(&rule.pattern)
                                                            .map(|re| re.is_match(&info.title))
                                                            .unwrap_or(false)
                                                    }
                                                };
                                                if matched {
                                                    matched_rules.push(rule);
                                                }
                                            }

                                            // Select the best match (longest pattern length wins)
                                            // This allows specific rules (e.g. "Matt M") to override generic ones (e.g. "Slack")
                                            if let Some(best_rule) = matched_rules
                                                .into_iter()
                                                .max_by_key(|r| r.pattern.chars().count())
                                            {
                                                (Some(p), Some(best_rule))
                                            } else {
                                                // No rule matched, use profile defaults
                                                (Some(p), None)
                                            }
                                        } else {
                                            (None, None)
                                        }
                                    })
                                    .unwrap_or((None, None));

                                // Determine policy and prompt_id from matched rule or profile defaults
                                let app_policy = matched_rule
                                    .map(|r| r.policy)
                                    .or_else(|| app_profile.map(|p| p.policy))
                                    .unwrap_or(crate::settings::AppReviewPolicy::Never);

                                let override_prompt_id = matched_rule
                                    .and_then(|r| r.prompt_id.clone())
                                    .or_else(|| app_profile.and_then(|p| p.prompt_id.clone()));

                                info!(
                                    "[Transcribe] App profile resolution: app_profile={:?}, matched_rule={:?}, override_prompt_id={:?}, app_policy={:?}, global_selected_prompt_id={:?}",
                                    app_profile.map(|p| &p.id),
                                    matched_rule.map(|r| &r.pattern),
                                    override_prompt_id,
                                    app_policy,
                                    settings_clone.post_process_selected_prompt_id,
                                );

                                // ═══════════════════════════════════════════
                                // Unified post-processing pipeline
                                // ═══════════════════════════════════════════
                                let pipeline_result =
                                    crate::actions::post_process::unified_post_process(
                                        &ah_clone,
                                        &settings_clone,
                                        &chinese_converted_text,
                                        secondary.as_deref(),
                                        true,
                                        override_prompt_id.clone(),
                                        active_window_snapshot_for_review
                                            .as_ref()
                                            .map(|info| info.app_name.clone()),
                                        active_window_snapshot_for_review
                                            .as_ref()
                                            .map(|info| info.title.clone()),
                                        matched_rule.map(|r| r.pattern.clone()),
                                        matched_rule.map(|r| r.match_type),
                                        history_id,
                                        skill_mode,
                                        review_editor_active,
                                        selected_text.clone(),
                                        review_document_text.clone(),
                                        cursor_context.clone(),
                                    )
                                    .await;

                                // Handle pipeline result
                                let mut post_process_failed = false;
                                match pipeline_result {
                                    crate::actions::post_process::PipelineResult::Skipped => {
                                        // No post-processing — use original transcription
                                        used_model = Some("无模型".to_string());
                                    }

                                    crate::actions::post_process::PipelineResult::Cached {
                                        text,
                                        model,
                                        prompt_id,
                                    } => {
                                        final_text = text;
                                        if model.is_some() {
                                            used_model = model;
                                        }
                                        if prompt_id.is_some() {
                                            post_process_prompt_id = prompt_id;
                                        }
                                    }

                                    crate::actions::post_process::PipelineResult::PassThrough {
                                        text,
                                        ..
                                    } => {
                                        final_text = text;
                                        used_model = Some("无模型".to_string());
                                        post_process_prompt_id =
                                            Some("__PASS_THROUGH__".to_string());
                                    }

                                    crate::actions::post_process::PipelineResult::PendingSkillConfirmation => {
                                        if let Some(pending_state) = ah_clone
                                            .try_state::<crate::ManagedPendingSkillConfirmation>()
                                        {
                                            if let Ok(mut guard) = pending_state.lock() {
                                                guard.process_id = active_window_snapshot_for_review
                                                    .as_ref()
                                                    .map(|info| info.process_id);
                                                guard.active_window =
                                                    active_window_snapshot_for_review.clone();
                                                guard.app_review_policy = Some(app_policy);
                                            }
                                        }
                                        info!("[PostProcess] Skill confirmation pending, keeping overlay visible");
                                        return;
                                    }

                                    crate::actions::post_process::PipelineResult::MultiModel {
                                        multi_items,
                                        prompt_id: effective_prompt_id,
                                        strategy,
                                        ..
                                    } => {
                                        // Determine output mode
                                        let output_mode = if let Some(ref pid) = effective_prompt_id {
                                            settings_clone
                                                .post_process_prompts
                                                .iter()
                                                .find(|p| &p.id == pid)
                                                .map(|p| p.output_mode)
                                                .or_else(|| {
                                                    let sm = crate::managers::skill::SkillManager::new(&ah_clone);
                                                    sm.get_all_skills()
                                                        .into_iter()
                                                        .find(|p| &p.id == pid)
                                                        .map(|p| p.output_mode)
                                                })
                                                .unwrap_or_default()
                                        } else {
                                            crate::settings::PromptOutputMode::default()
                                        };

                                        // Determine if review window should be shown
                                        let should_review = should_show_post_process_review_window(
                                            votype_mode,
                                            override_prompt_id.as_deref(),
                                            app_policy,
                                            output_mode,
                                            strategy == "manual",
                                        );

                                        // Build loading candidates
                                        let mut loading_candidates: Vec<crate::review_window::MultiModelCandidate> =
                                            multi_items
                                                .iter()
                                                .map(|item| {
                                                    let provider_label = settings_clone
                                                        .post_process_provider(&item.provider_id)
                                                        .map(|p| p.label.clone())
                                                        .unwrap_or_else(|| item.provider_id.clone());
                                                    let label = item
                                                        .custom_label
                                                        .clone()
                                                        .unwrap_or_else(|| item.model_id.clone());
                                                    crate::review_window::MultiModelCandidate {
                                                        id: item.id.clone(),
                                                        label,
                                                        provider_label,
                                                        text: String::new(),
                                                        confidence: None,
                                                        processing_time_ms: 0,
                                                        error: None,
                                                        ready: false,
                                                    }
                                                })
                                                .collect();

                                        // For manual strategy, reorder to put the favorite model first
                                        if strategy == "manual" {
                                            let favorite_id: Option<String> = settings_clone
                                                .multi_model_preferred_id
                                                .clone()
                                                .filter(|id| multi_items.iter().any(|item| &item.id == id))
                                                .or_else(|| {
                                                    settings_clone
                                                        .multi_model_manual_pick_counts
                                                        .iter()
                                                        .filter(|(id, _)| multi_items.iter().any(|item| &item.id == *id))
                                                        .max_by_key(|(_, count)| *count)
                                                        .map(|(id, _)| id.clone())
                                                });
                                            if let Some(ref fav_id) = favorite_id {
                                                if let Some(pos) = loading_candidates.iter().position(|c| &c.id == fav_id) {
                                                    if pos > 0 {
                                                        let fav = loading_candidates.remove(pos);
                                                        loading_candidates.insert(0, fav);
                                                    }
                                                }
                                            }
                                        }

                                        if should_review {
                                            crate::review_window::set_last_active_window(
                                                active_window_snapshot_for_review.clone(),
                                            );
                                            crate::review_window::show_review_window_with_candidates(
                                                &ah_clone,
                                                transcription_clone.clone(),
                                                loading_candidates,
                                                history_id,
                                                output_mode,
                                                None,
                                                effective_prompt_id.clone(),
                                                None, // auto_selected_id comes via event later
                                            );
                                            // Hide overlay now — review window takes over
                                            utils::hide_recording_overlay(&ah_clone);
                                            change_tray_icon(&ah_clone, TrayIconState::Idle);
                                        }
                                        // When !should_review, keep overlay visible during
                                        // multi-model execution; it will be hidden after paste.

                                        // Start multi-model processing (streams results via events)
                                        info!(
                                            "[MultiModel] Starting multi-model post-processing ({} models, strategy={})",
                                            multi_items.len(), strategy
                                        );
                                        let collected =
                                            crate::actions::post_process::multi_post_process_transcription(
                                                &ah_clone,
                                                &settings_clone,
                                                &chinese_converted_text,
                                                secondary.as_deref(),
                                                history_id,
                                                active_window_snapshot_for_review
                                                    .as_ref()
                                                    .map(|info| info.app_name.clone()),
                                                active_window_snapshot_for_review
                                                    .as_ref()
                                                    .map(|info| info.title.clone()),
                                                override_prompt_id.clone(),
                                            )
                                            .await;

                                        // Save best result to history
                                        if let Some(best) = collected.results.iter().find(|r| r.ready && r.error.is_none()) {
                                            let model_name = multi_items
                                                .iter()
                                                .find(|item| item.id == best.id)
                                                .map(|item| item.model_id.clone())
                                                .unwrap_or_else(|| best.label.clone());
                                            if let Some(hid) = history_id {
                                                if let Err(e) = hm_clone
                                                    .save_post_processed_text(
                                                        hid,
                                                        best.text.clone(),
                                                        Some(model_name),
                                                        settings_clone.post_process_selected_prompt_id.clone(),
                                                        None,
                                                        None,
                                                    )
                                                    .await
                                                {
                                                    error!("Failed to save multi-model result to history: {}", e);
                                                }
                                            }
                                        }

                                        // If not showing review: auto-confirm by pasting the auto-selected result
                                        if !should_review {
                                            let auto_id = collected.auto_selected_id.as_deref();
                                            let auto_result = auto_id
                                                .and_then(|id| collected.results.iter().find(|r| r.id == id))
                                                .or_else(|| collected.results.iter().find(|r| r.ready && r.error.is_none()));

                                            if let Some(result) = auto_result {
                                                let paste_text = result.text.clone();
                                                info!(
                                                    "[MultiModel] Auto-confirm: pasting result from {} (strategy={})",
                                                    result.label, strategy
                                                );

                                                // Check if a new recording started during processing
                                                if rm_clone.get_current_transcription_id() != current_transcription_id {
                                                    info!("New recording started during multi-model processing; skipping paste.");
                                                    return;
                                                }

                                                show_inserting_overlay(&ah_clone);

                                                let ah_inner = ah_clone.clone();
                                                let ah_for_cleanup = ah_clone.clone();
                                                let active_window_for_paste =
                                                    active_window_snapshot_for_review.clone();
                                                ah_clone
                                                    .run_on_main_thread(move || {
                                                        restore_focus_before_direct_paste(
                                                            active_window_for_paste.as_ref(),
                                                        );
                                                        if let Err(e) = utils::paste(paste_text, ah_inner) {
                                                            error!("Failed to paste multi-model result: {}", e);
                                                        }
                                                        utils::hide_recording_overlay(&ah_for_cleanup);
                                                        change_tray_icon(&ah_for_cleanup, TrayIconState::Idle);
                                                    })
                                                    .unwrap_or_else(|e| {
                                                        error!("Failed to run paste on main thread: {:?}", e)
                                                    });
                                            } else {
                                                // No usable result — hide overlay anyway
                                                utils::hide_recording_overlay(&ah_clone);
                                                change_tray_icon(&ah_clone, TrayIconState::Idle);
                                            }
                                        }
                                        return;
                                    }

                                    crate::actions::post_process::PipelineResult::SingleModel {
                                        text: processed_text,
                                        model,
                                        prompt_id,
                                        token_count: tc,
                                        error: err,
                                        error_message,
                                        metrics_model_id,
                                        metrics_provider_id,
                                        metrics_duration_ms,
                                        metrics_tokens_per_sec,
                                        ..
                                    } => {
                                        let outcome = resolve_post_process_outcome(
                                            &transcription_clone,
                                            processed_text.as_deref(),
                                            err,
                                        );
                                        post_process_failed = outcome.failed;
                                        error_shown = error_shown || post_process_failed;

                                        if model.is_some() {
                                            used_model = model;
                                        }
                                        final_text = outcome.final_text;
                                        if prompt_id.is_some() {
                                            post_process_prompt_id = prompt_id;
                                        }
                                        if post_process_failed {
                                            if let Some(ref msg) = error_message {
                                                error!("Post-processing failed: {}", msg);
                                            } else if processed_text
                                                .as_deref()
                                                .map(str::trim)
                                                .is_some_and(|text| text.is_empty())
                                            {
                                                error!(
                                                    "Post-processing failed: model returned empty text"
                                                );
                                            }

                                            if !err {
                                                let _ = ah_clone.emit(
                                                    "overlay-error",
                                                    serde_json::json!({
                                                        "message": "润色失败：模型返回了空结果",
                                                    }),
                                                );
                                            }
                                        }

                                        // Log metrics
                                        if let (Some(m_id), Some(p_id)) = (metrics_model_id, metrics_provider_id) {
                                            if let Some(metrics) = ah_clone.try_state::<std::sync::Arc<crate::managers::llm_metrics::LlmMetricsManager>>() {
                                                let call_type = if review_document_text.is_some() { "rewrite" } else { "single_polish" };
                                                if let Err(e) = metrics.log_call(&crate::managers::llm_metrics::LlmCallRecord {
                                                    history_id,
                                                    model_id: m_id,
                                                    provider: p_id,
                                                    call_type: call_type.to_string(),
                                                    input_tokens: None,
                                                    output_tokens: None,
                                                    total_tokens: tc,
                                                    token_estimate: None,
                                                    duration_ms: metrics_duration_ms.unwrap_or(0) as i64,
                                                    tokens_per_sec: metrics_tokens_per_sec,
                                                    error: if err { error_message.clone() } else { None },
                                                    is_fallback: false,
                                                }) {
                                                    log::error!("[LlmMetrics] Failed to log single model call: {}", e);
                                                }
                                            }
                                        }
                                    }
                                }

                                // Capture the prompt content for history
                                if let Some(pid) = &post_process_prompt_id {
                                    if let Some(prompt) = settings_clone
                                        .post_process_prompts
                                        .iter()
                                        .find(|p| &p.id == pid)
                                    {
                                        post_process_prompt_text = prompt.instructions.clone();
                                        post_process_prompt_name = prompt.name.clone();
                                    }
                                }

                                let change_percent =
                                    compute_change_percent(&transcription_clone, &final_text);

                                // Get output_mode early to determine review behavior
                                // Default to Polish (not Chat) when no prompt is configured
                                let output_mode = if let Some(pid) = &post_process_prompt_id {
                                    settings_clone
                                        .post_process_prompts
                                        .iter()
                                        .find(|p| &p.id == pid)
                                        .map(|p| p.output_mode)
                                        .unwrap_or(crate::settings::PromptOutputMode::Polish)
                                } else {
                                    crate::settings::PromptOutputMode::Polish
                                };

                                // Always show review/confidence window for normal polish mode.
                                // Only skip for Votype internal modes and explicit skip markers.
                                let should_review = should_show_post_process_review_window(
                                    votype_mode,
                                    override_prompt_id.as_deref(),
                                    app_policy,
                                    output_mode,
                                    false,
                                );

                                if should_review {
                                    log::info!(
                                        "Review required (policy={:?}, change_percent={}), showing window",
                                        app_policy, change_percent
                                    );

                                    crate::review_window::set_last_active_window(
                                        active_window_snapshot_for_review.clone(),
                                    );
                                    // Persist LLM output so history always captures it
                                    if let Some(history_id) = history_id {
                                        if let Err(e) = hm_clone
                                            .update_transcription_post_processing(
                                                history_id,
                                                final_text.clone(),
                                                post_process_prompt_text.clone(),
                                                post_process_prompt_name.clone(),
                                                post_process_prompt_id.clone(),
                                                used_model.clone(),
                                                None,
                                                None,
                                            )
                                            .await
                                        {
                                            error!(
                                                "Failed to update transcription with post-processing before review: {}",
                                                e
                                            );
                                        }
                                    }

                                    // Single-model / PassThrough: use polish-style review window
                                    let skill_name_for_review =
                                        if skill_mode && !post_process_prompt_name.is_empty() {
                                            Some(post_process_prompt_name.clone())
                                        } else {
                                            None
                                        };
                                    crate::review_window::show_review_window(
                                        &ah_clone,
                                        transcription_clone.clone(),
                                        final_text.clone(),
                                        change_percent,
                                        history_id,
                                        None, // confidence_reason
                                        output_mode,
                                        skill_name_for_review,
                                        post_process_prompt_id
                                            .clone()
                                            .or(override_prompt_id.clone()),
                                        used_model.clone(),
                                    );
                                    // Hide the overlay since review window is now shown
                                    utils::hide_recording_overlay(&ah_clone);
                                    change_tray_icon(&ah_clone, TrayIconState::Idle);
                                    return;
                                }
                                if post_process_failed {
                                    // Error already logged in PipelineResult::SingleModel arm
                                }
                            }

                            let should_translate_on_insert =
                                app_profile_translate_to_english_on_insert(
                                    &settings_clone,
                                    active_window_snapshot_for_review.as_ref(),
                                );

                            // Handle Votype internal modes before history save (they return early)
                            if matches!(votype_mode, VotypeInputMode::MainPolishInput) {
                                let final_text = maybe_translate_text_for_insert(
                                    &ah_clone,
                                    should_translate_on_insert,
                                    skill_mode,
                                    final_text,
                                )
                                .await;
                                utils::hide_recording_overlay(&ah_clone);
                                change_tray_icon(&ah_clone, TrayIconState::Idle);
                                let _ = ah_clone.emit("votype-local-insert", final_text.clone());
                                return;
                            }

                            if matches!(votype_mode, VotypeInputMode::MainSelectedEdit) {
                                let final_text = maybe_translate_text_for_insert(
                                    &ah_clone,
                                    should_translate_on_insert,
                                    skill_mode,
                                    final_text,
                                )
                                .await;
                                utils::hide_recording_overlay(&ah_clone);
                                change_tray_icon(&ah_clone, TrayIconState::Idle);
                                let _ = ah_clone.emit("votype-local-insert", final_text.clone());
                                return;
                            }

                            if matches!(votype_mode, VotypeInputMode::ReviewRewrite) {
                                change_tray_icon(&ah_clone, TrayIconState::Idle);
                                // Update REVIEW_EDITOR_CONTENT synchronously BEFORE emitting.
                                // This ensures the next freeze_review_editor_content_snapshot()
                                // captures the latest text, avoiding the race condition where
                                // the frontend's async invoke hasn't arrived yet.
                                crate::review_window::set_review_editor_content(final_text.clone());
                                #[derive(serde::Serialize, Clone)]
                                struct RewriteApplyPayload {
                                    text: String,
                                    model: Option<String>,
                                }
                                let _ = ah_clone.emit(
                                    "review-window-rewrite-apply",
                                    RewriteApplyPayload {
                                        text: final_text.clone(),
                                        model: used_model,
                                    },
                                );
                                // Delay overlay hide to show operation-complete flash (~800ms)
                                let ah_for_hide = ah_clone.clone();
                                tauri::async_runtime::spawn(async move {
                                    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                                    utils::hide_recording_overlay(&ah_for_hide);
                                });
                                return;
                            }

                            // 3. Save the result to database (Available for both branches)
                            if let Some(history_id) = history_id {
                                if let Err(e) = hm_clone
                                    .update_transcription_post_processing(
                                        history_id,
                                        final_text.clone(),
                                        post_process_prompt_text,
                                        post_process_prompt_name,
                                        post_process_prompt_id,
                                        used_model,
                                        None,
                                        None,
                                    )
                                    .await
                                {
                                    error!(
                                        "Failed to update transcription with post-processing: {}",
                                        e
                                    );
                                }
                            }

                            if rm_clone.get_current_transcription_id() != current_transcription_id {
                                info!(
                                    "New recording started during post-processing; skipping paste."
                                );
                                return;
                            }

                            let final_text = maybe_translate_text_for_insert(
                                &ah_clone,
                                should_translate_on_insert,
                                skill_mode,
                                final_text,
                            )
                            .await;

                            show_inserting_overlay(&ah_clone);

                            let ah_clone_inner = ah_clone.clone();
                            let ah_for_cleanup = ah_clone.clone();
                            let active_window_for_paste = active_window_snapshot_for_review.clone();
                            ah_clone
                                .run_on_main_thread(move || {
                                    if error_shown {
                                        let ah_delayed = ah_clone_inner.clone();
                                        std::thread::spawn(move || {
                                            std::thread::sleep(Duration::from_millis(3000));
                                            utils::hide_recording_overlay(&ah_delayed);
                                            change_tray_icon(&ah_delayed, TrayIconState::Idle);
                                        });
                                    }

                                    restore_focus_before_direct_paste(
                                        active_window_for_paste.as_ref(),
                                    );
                                    if let Err(e) = utils::paste(final_text, ah_clone_inner) {
                                        error!("Failed to paste transcription: {}", e);
                                    }
                                    if !error_shown {
                                        utils::hide_recording_overlay(&ah_for_cleanup);
                                        change_tray_icon(&ah_for_cleanup, TrayIconState::Idle);
                                    }
                                })
                                .unwrap_or_else(|e| {
                                    error!("Failed to run paste on main thread: {:?}", e)
                                });
                        });

                        ppm.set_current_task(task.abort_handle());
                        // Await the inner task so the FinishGuard stays
                        // alive (keeping Esc registered) during post-processing.
                        let _ = task.await;
                    } else {
                        // Post-processing is disabled — check if app policy still
                        // requires showing the review window before pasting.
                        let votype_mode = if shortcut_str == "review-window-local" {
                            VotypeInputMode::ReviewRewrite
                        } else {
                            crate::window_context::resolve_votype_input_mode(
                                active_window_snapshot
                                    .as_ref()
                                    .map(|info| info.app_name.as_str()),
                                active_window_snapshot
                                    .as_ref()
                                    .map(|info| info.title.as_str()),
                                review_editor_active,
                                selected_text
                                    .as_ref()
                                    .map(|text| !text.trim().is_empty())
                                    .unwrap_or(false),
                            )
                        };
                        let (app_policy, resolved_prompt_id) = active_window_snapshot
                            .as_ref()
                            .and_then(|info| {
                                let profile_id = settings
                                    .app_to_profile
                                    .iter()
                                    .find(|(k, _)| k.eq_ignore_ascii_case(&info.app_name))
                                    .map(|(_, v)| v);
                                let profile = profile_id.and_then(|pid| {
                                    settings.app_profiles.iter().find(|p| &p.id == pid)
                                });
                                profile.map(|p| {
                                    // Check title rules for a more specific policy
                                    let matched_rule = p
                                        .rules
                                        .iter()
                                        .filter(|r| match r.match_type {
                                            crate::settings::TitleMatchType::Text => info
                                                .title
                                                .to_lowercase()
                                                .contains(&r.pattern.to_lowercase()),
                                            crate::settings::TitleMatchType::Regex => {
                                                regex::Regex::new(&r.pattern)
                                                    .map(|re| re.is_match(&info.title))
                                                    .unwrap_or(false)
                                            }
                                        })
                                        .max_by_key(|r| r.pattern.chars().count());
                                    let policy = matched_rule.map(|r| r.policy).unwrap_or(p.policy);
                                    let prompt_id = matched_rule
                                        .and_then(|r| r.prompt_id.clone())
                                        .or_else(|| p.prompt_id.clone());
                                    (policy, prompt_id)
                                })
                            })
                            .unwrap_or((crate::settings::AppReviewPolicy::Auto, None));

                        if matches!(votype_mode, VotypeInputMode::MainPolishInput) {
                            let ah_clone = ah.clone();
                            let should_translate_on_insert =
                                app_profile_translate_to_english_on_insert(
                                    &settings,
                                    active_window_snapshot.as_ref(),
                                );
                            let text_for_insert = maybe_translate_text_for_insert(
                                &ah,
                                should_translate_on_insert,
                                skill_mode,
                                transcription_clone,
                            )
                            .await;
                            ah.run_on_main_thread(move || {
                                utils::hide_recording_overlay(&ah_clone);
                                change_tray_icon(&ah_clone, TrayIconState::Idle);
                                let _ = ah_clone.emit("votype-local-insert", text_for_insert);
                            })
                            .unwrap_or_else(|e| {
                                error!("Failed to emit local insert on main thread: {:?}", e)
                            });
                        } else if matches!(votype_mode, VotypeInputMode::MainSelectedEdit) {
                            let ah_clone = ah.clone();
                            ah.run_on_main_thread(move || {
                                utils::hide_recording_overlay(&ah_clone);
                                change_tray_icon(&ah_clone, TrayIconState::Idle);
                                log::warn!(
                                    "Selected-edit mode requires post-processing; skipping raw insertion"
                                );
                            })
                            .unwrap_or_else(|e| {
                                error!("Failed to finish selected-edit fallback on main thread: {:?}", e)
                            });
                        } else if matches!(votype_mode, VotypeInputMode::ReviewRewrite) {
                            let ah_clone = ah.clone();
                            ah.run_on_main_thread(move || {
                                utils::hide_recording_overlay(&ah_clone);
                                change_tray_icon(&ah_clone, TrayIconState::Idle);
                                log::warn!(
                                    "Review rewrite mode requires post-processing; skipping raw document replacement"
                                );
                            })
                            .unwrap_or_else(|e| {
                                error!("Failed to finish review rewrite fallback on main thread: {:?}", e)
                            });
                        } else if app_policy == crate::settings::AppReviewPolicy::Always {
                            // Resolve output_mode from the prompt configured for this app,
                            // falling back to the global selected prompt.
                            let effective_prompt_id = resolved_prompt_id
                                .or_else(|| settings.post_process_selected_prompt_id.clone());
                            let output_mode = effective_prompt_id
                                .as_ref()
                                .and_then(|pid| {
                                    settings
                                        .post_process_prompts
                                        .iter()
                                        .find(|p| &p.id == pid)
                                        .map(|p| p.output_mode)
                                })
                                .unwrap_or(crate::settings::SkillOutputMode::Polish);

                            info!(
                                "[Transcribe] App policy is Always (no post-processing), showing review window (output_mode={:?})",
                                output_mode
                            );
                            crate::review_window::set_last_active_window(
                                active_window_snapshot.clone(),
                            );
                            let ah_clone = ah.clone();
                            crate::review_window::show_review_window(
                                &ah_clone,
                                transcription_clone.clone(),
                                transcription_clone.clone(),
                                0,
                                history_id,
                                None,
                                output_mode,
                                None,
                                effective_prompt_id,
                                None,
                            );
                            utils::hide_recording_overlay(&ah_clone);
                            change_tray_icon(&ah_clone, TrayIconState::Idle);
                        } else {
                            let ah_clone = ah.clone();
                            let active_window_for_paste = active_window_snapshot.clone();
                            let should_translate_on_insert =
                                app_profile_translate_to_english_on_insert(
                                    &settings,
                                    active_window_snapshot.as_ref(),
                                );
                            let text_for_insert = maybe_translate_text_for_insert(
                                &ah,
                                should_translate_on_insert,
                                skill_mode,
                                transcription_clone,
                            )
                            .await;
                            ah.run_on_main_thread(move || {
                                let ah_for_paste = ah_clone.clone();
                                show_inserting_overlay(&ah_clone);
                                restore_focus_before_direct_paste(active_window_for_paste.as_ref());
                                if let Err(e) = utils::paste(text_for_insert, ah_for_paste) {
                                    error!("Failed to paste transcription: {}", e);
                                }
                                utils::hide_recording_overlay(&ah_clone);
                                change_tray_icon(&ah_clone, TrayIconState::Idle);
                            })
                            .unwrap_or_else(|e| {
                                error!("Failed to run paste on main thread: {:?}", e)
                            });
                        }
                    }
                } else {
                    let err_msg = primary_error.unwrap_or_else(|| "unknown".to_string());
                    debug!("Global Shortcut Transcription error: {}", err_msg);

                    // Update the pre-saved placeholder with the failure message.
                    // Audio was already saved during pre-save so users can retry later.
                    let asr_model = if settings.online_asr_enabled {
                        settings
                            .selected_asr_model
                            .as_ref()
                            .map(|c| c.primary_id.clone())
                            .unwrap_or_else(|| "online".to_string())
                    } else {
                        settings.selected_model.clone()
                    };

                    let failure_text = format!("[Transcription failed] {}", err_msg);
                    if let Some(pid) = presave_history_id {
                        if let Err(e) = hm
                            .update_transcription_content(
                                pid,
                                failure_text,
                                asr_model,
                                settings.selected_language.clone(),
                                duration_ms,
                                transcription_ms,
                                0,
                            )
                            .await
                        {
                            error!("Failed to update failed transcription in history: {}", e);
                        }
                    }

                    let _ = ah.emit(
                        "overlay-error",
                        serde_json::json!({
                            "code": "transcription_failed_saved",
                        }),
                    );

                    // Give the user a moment to see the failure state, then close the overlay.
                    sleep(Duration::from_millis(2500)).await;
                }
            } else {
                debug!("No samples retrieved from recording stop");
            }
        });

        ppm_outer.set_pipeline_task(pipeline_handle);

        debug!(
            "TranscribeAction::stop completed in {:?}",
            stop_time.elapsed()
        );
    }

    fn mode(&self) -> super::ActionMode {
        super::ActionMode::Stateful
    }
}

fn realtime_worker_loop(
    rx: std::sync::mpsc::Receiver<Vec<f32>>,
    tm: &TranscriptionManager,
    app: &AppHandle,
    interval_ms: u64,
    window_seconds: u64,
) {
    let sample_rate = 16000;
    let max_window_samples = (window_seconds as usize) * sample_rate;
    let mut accumulated: Vec<f32> = Vec::new();
    let mut last_transcribe = Instant::now();
    let mut has_new_audio = false;
    // Segment-based: keep finalized text from previous segments, only re-transcribe current window
    let mut finalized_text = String::new();
    let mut finalized_samples: usize = 0;

    // Punctuation anchoring: run punct model every cycle, anchors as fallback when busy
    let mut punct_anchors: Vec<PunctAnchor> = Vec::new();
    // Track the last raw text so we can apply final punctuation on exit
    let mut last_raw_text = String::new();

    loop {
        match rx.recv_timeout(Duration::from_millis(interval_ms)) {
            Ok(frame) => {
                accumulated.extend_from_slice(&frame);
                has_new_audio = true;

                // Check if it's time to transcribe
                if last_transcribe.elapsed().as_millis() < interval_ms as u128 {
                    continue;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // No new speech frames — skip if no new audio since last transcription
                if !has_new_audio || accumulated.is_empty() {
                    continue;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // Recording ended — do one final punct pass on the full text
                if !last_raw_text.is_empty() {
                    if let Some(final_punctuated) = tm.try_add_punctuation(&last_raw_text) {
                        debug!("Realtime worker: final punct pass before exit");
                        let _ = app.emit(
                            "realtime-partial",
                            serde_json::json!({ "text": final_punctuated }),
                        );
                    }
                }
                debug!("Realtime worker: channel disconnected, exiting");
                break;
            }
        }

        if accumulated.is_empty() {
            continue;
        }

        // Only transcribe the current (un-finalized) window
        let current_audio = &accumulated[finalized_samples..];
        // Need at least ~1.5s of audio for meaningful transcription;
        // shorter clips produce garbage results.
        if current_audio.len() < sample_rate * 3 / 2 {
            continue;
        }

        let st = Instant::now();
        if let Some(current_text) = tm.try_transcribe_raw(current_audio.to_vec()) {
            let elapsed_ms = st.elapsed().as_millis();

            // Filter out non-displayable characters (rare model tokens that render
            // as replacement chars or control chars in UI/terminal).
            let clean_text: String = current_text
                .chars()
                .filter(|c| {
                    !c.is_control() && *c != '\u{FFFD}' && !('\u{E000}'..='\u{F8FF}').contains(c)
                    // Private Use Area
                })
                .collect();
            if clean_text.is_empty() {
                continue;
            }

            // Combine finalized segments with current window
            let raw_text = if finalized_text.is_empty() {
                clean_text.clone()
            } else {
                format!("{}{}", finalized_text, clean_text)
            };
            last_raw_text.clone_from(&raw_text);

            // Punctuation: run every cycle so display always has punctuation.
            // Anchors are fallback only when punct model is busy.
            let display_text = if let Some(punctuated) = tm.try_add_punctuation(&raw_text) {
                punct_anchors = extract_punct_anchors(&raw_text, &punctuated);
                punctuated
            } else {
                apply_punct_anchors(&raw_text, &punct_anchors)
            };

            if crate::DEBUG_LOG_TRANSCRIPTION.load(std::sync::atomic::Ordering::Relaxed) {
                let preview: String = display_text.chars().take(80).collect();
                let suffix = if display_text.chars().count() > 80 {
                    "…"
                } else {
                    ""
                };
                info!(
                    "Realtime partial ({}ms, {}s audio, len={}): {}{}",
                    elapsed_ms,
                    current_audio.len() / sample_rate,
                    display_text.chars().count(),
                    preview,
                    suffix,
                );
            }
            let _ = app.emit(
                "realtime-partial",
                serde_json::json!({ "text": display_text }),
            );
            last_transcribe = Instant::now();
            has_new_audio = false;

            // If current window exceeds max, finalize it and start fresh
            if current_audio.len() > max_window_samples {
                finalized_text = raw_text;
                finalized_samples = accumulated.len();
                debug!(
                    "Realtime: finalized segment, total finalized text len={}",
                    finalized_text.len()
                );
            }
        }
        // If engine is busy, skip this round — try again next interval
    }
}

// ─── Punctuation anchoring ──────────────────────────────────────────────────

/// A punctuation mark with its surrounding character context, used to
/// re-insert punctuation into new ASR output between punct-model runs.
struct PunctAnchor {
    /// Up to 2 characters immediately before the punctuation mark.
    before: String,
    /// Up to 1 character immediately after (empty if punct is at end of text).
    after: String,
    /// The punctuation character itself.
    punct: char,
}

/// Characters the CT-Transformer punct model may insert.
fn is_inserted_punct(c: char) -> bool {
    matches!(
        c,
        '，' | '。'
            | '！'
            | '？'
            | '；'
            | '：'
            | '、'
            | '\u{201C}' // "
            | '\u{201D}' // "
            | '\u{2018}' // '
            | '\u{2019}' // '
            | '（'
            | '）'
            | '《'
            | '》'
            | '【'
            | '】'
            | '…'
            | '—'
            | ','
            | '.'
            | '!'
            | '?'
            | ';'
            | ':'
    )
}

/// Compare raw text with its punctuated version and extract anchors
/// that record where each punctuation mark was inserted.
fn extract_punct_anchors(raw: &str, punctuated: &str) -> Vec<PunctAnchor> {
    let raw_chars: Vec<char> = raw.chars().collect();
    let punct_chars: Vec<char> = punctuated.chars().collect();
    let mut anchors = Vec::new();
    let mut ri = 0; // index into raw_chars
    let mut pi = 0; // index into punct_chars

    while pi < punct_chars.len() {
        if ri < raw_chars.len() && punct_chars[pi] == raw_chars[ri] {
            // Characters match — advance both
            ri += 1;
            pi += 1;
        } else if is_inserted_punct(punct_chars[pi]) {
            // Inserted punctuation — record surrounding context from raw text
            let before: String = raw_chars[..ri]
                .iter()
                .rev()
                .take(2)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .cloned()
                .collect();
            let after: String = raw_chars[ri..].iter().take(1).cloned().collect();

            if !before.is_empty() {
                anchors.push(PunctAnchor {
                    before,
                    after,
                    punct: punct_chars[pi],
                });
            }
            pi += 1;
        } else {
            // Non-punct mismatch — alignment is broken, stop
            break;
        }
    }

    anchors
}

/// Apply saved punctuation anchors to new raw text by matching the
/// surrounding character patterns and inserting punct marks.
fn apply_punct_anchors(raw: &str, anchors: &[PunctAnchor]) -> String {
    if anchors.is_empty() {
        return raw.to_string();
    }

    // Collect insertion points: (byte_position, punct_char)
    let mut insertions: Vec<(usize, char)> = Vec::new();
    let mut search_from: usize = 0;

    for anchor in anchors {
        if anchor.after.is_empty() {
            // Punct was at the end of text — only insert if text still ends with `before`
            if raw.ends_with(&anchor.before) {
                insertions.push((raw.len(), anchor.punct));
            }
        } else {
            let pattern = format!("{}{}", anchor.before, anchor.after);
            if let Some(rel_pos) = raw[search_from..].find(&pattern) {
                let abs_pos = search_from + rel_pos;
                let insert_pos = abs_pos + anchor.before.len();
                insertions.push((insert_pos, anchor.punct));
                search_from = insert_pos;
            }
        }
    }

    if insertions.is_empty() {
        return raw.to_string();
    }

    // Insert from right to left so earlier byte positions stay valid
    insertions.sort_by(|a, b| b.0.cmp(&a.0));

    let mut result = raw.to_string();
    for (pos, punct) in insertions {
        if pos <= result.len() {
            result.insert(pos, punct);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::{
        classify_recording_samples, clear_start_snapshot_if_matches, direct_paste_target_pid,
        effective_recording_duration_ms, resolve_post_process_outcome, set_start_snapshot,
        should_show_post_process_review_window, take_start_snapshot, PostProcessOutcome,
        RecordingDisposition, EFFECTIVE_RECORDING_TOO_SHORT_MS, WHISPER_SAMPLE_RATE,
    };
    use crate::settings::{AppReviewPolicy, PromptOutputMode};
    use crate::window_context::VotypeInputMode;

    #[test]
    fn short_effective_recording_is_ignored() {
        let samples = vec![0.0; WHISPER_SAMPLE_RATE / 4];

        assert_eq!(
            classify_recording_samples(&samples),
            RecordingDisposition::IgnoredTooShort {
                effective_duration_ms: 250,
            }
        );
    }

    #[test]
    fn threshold_duration_recording_proceeds() {
        let sample_count =
            (WHISPER_SAMPLE_RATE as i64 * EFFECTIVE_RECORDING_TOO_SHORT_MS / 1000) as usize;
        let samples = vec![0.0; sample_count];

        assert_eq!(
            classify_recording_samples(&samples),
            RecordingDisposition::Proceed {
                effective_duration_ms: EFFECTIVE_RECORDING_TOO_SHORT_MS,
            }
        );
    }

    #[test]
    fn effective_duration_uses_whisper_sample_rate() {
        let samples = vec![0.0; WHISPER_SAMPLE_RATE * 3 / 2];

        assert_eq!(effective_recording_duration_ms(&samples), 1500);
    }

    #[test]
    fn empty_post_process_result_falls_back_to_transcription_and_marks_failed() {
        let outcome = resolve_post_process_outcome("原始识别", Some(""), true);

        assert_eq!(
            outcome,
            PostProcessOutcome {
                final_text: "原始识别".to_string(),
                failed: true,
            }
        );
    }

    #[test]
    fn blank_post_process_result_falls_back_to_transcription_and_marks_failed() {
        let outcome = resolve_post_process_outcome("original asr", Some("   "), false);

        assert_eq!(
            outcome,
            PostProcessOutcome {
                final_text: "original asr".to_string(),
                failed: true,
            }
        );
    }

    #[test]
    fn chat_output_mode_overrides_never_review_policy() {
        assert!(should_show_post_process_review_window(
            VotypeInputMode::ExternalDefault,
            None,
            AppReviewPolicy::Never,
            PromptOutputMode::Chat,
            false
        ));
    }

    #[test]
    fn internal_votype_modes_still_skip_chat_review_window() {
        assert!(!should_show_post_process_review_window(
            VotypeInputMode::ReviewRewrite,
            None,
            AppReviewPolicy::Always,
            PromptOutputMode::Chat,
            false
        ));
    }

    #[test]
    fn polish_output_mode_still_respects_never_review_policy() {
        assert!(!should_show_post_process_review_window(
            VotypeInputMode::ExternalDefault,
            None,
            AppReviewPolicy::Never,
            PromptOutputMode::Polish,
            false
        ));
    }

    #[test]
    fn direct_paste_target_pid_uses_snapshot_pid() {
        let snapshot = crate::active_window::ActiveWindowInfo {
            title: "Codex".to_string(),
            app_name: "Codex".to_string(),
            window_id: "1".to_string(),
            process_id: 42,
            process_path: "/Applications/Codex.app".to_string(),
            position: crate::active_window::WindowPosition {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
        };

        assert_eq!(direct_paste_target_pid(Some(&snapshot)), Some(42));
        assert_eq!(direct_paste_target_pid(None), None);
    }

    // 全部 start-snapshot 相关的测试都改写同一个全局 slot。
    // cargo test 默认并行运行，必须用 test-side Mutex 串行化，否则不同测试间会互相覆盖
    // 导致 `set + take` 的对偶被打破。
    static SNAPSHOT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn make_test_snapshot(pid: u64, app: &str) -> crate::active_window::ActiveWindowInfo {
        crate::active_window::ActiveWindowInfo {
            title: format!("{app}-title"),
            app_name: app.to_string(),
            window_id: format!("win-{pid}"),
            process_id: pid,
            process_path: format!("/Applications/{app}.app"),
            position: crate::active_window::WindowPosition {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
        }
    }

    #[test]
    fn start_snapshot_take_returns_matching_id_and_clears_slot() {
        let _guard = SNAPSHOT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let id: u64 = 0xA001_0001;
        clear_start_snapshot_if_matches(id);

        set_start_snapshot(id, Some(make_test_snapshot(101, "AppA")));

        let taken = take_start_snapshot(id).expect("should return snapshot for matching id");
        assert_eq!(taken.process_id, 101);
        assert_eq!(taken.app_name, "AppA");

        assert!(take_start_snapshot(id).is_none());
    }

    #[test]
    fn start_snapshot_take_with_mismatching_id_returns_none_and_keeps_slot() {
        let _guard = SNAPSHOT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let id_owner: u64 = 0xA001_0002;
        let id_other: u64 = 0xA001_0003;
        clear_start_snapshot_if_matches(id_owner);
        clear_start_snapshot_if_matches(id_other);

        set_start_snapshot(id_owner, Some(make_test_snapshot(202, "AppB")));

        assert!(take_start_snapshot(id_other).is_none());

        let taken = take_start_snapshot(id_owner).expect("owner should still take its snapshot");
        assert_eq!(taken.app_name, "AppB");
    }

    #[test]
    fn clear_start_snapshot_if_matches_only_clears_owned_id() {
        let _guard = SNAPSHOT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let id_owner: u64 = 0xA001_0004;
        let id_other: u64 = 0xA001_0005;
        clear_start_snapshot_if_matches(id_owner);
        clear_start_snapshot_if_matches(id_other);

        set_start_snapshot(id_owner, Some(make_test_snapshot(303, "AppC")));

        clear_start_snapshot_if_matches(id_other);
        assert!(
            take_start_snapshot(id_owner).is_some(),
            "non-matching clear must not drop owner's snapshot"
        );

        set_start_snapshot(id_owner, Some(make_test_snapshot(303, "AppC")));
        clear_start_snapshot_if_matches(id_owner);
        assert!(take_start_snapshot(id_owner).is_none());
    }
}
