// Review Window - Independent floating window for reviewing low-confidence transcriptions
// This module handles the creation, display, and lifecycle of the review window

use crate::active_window::ActiveWindowInfo;
use crate::settings::PromptOutputMode;
use log::{debug, error};
use once_cell::sync::Lazy;
use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const REVIEW_WINDOW_WIDTH: f64 = 540.0;
const REVIEW_WINDOW_MIN_WIDTH: f64 = 600.0;
const REVIEW_WINDOW_MAX_WIDTH: f64 = 1080.0;
const REVIEW_WINDOW_HEIGHT: f64 = 480.0;
const REVIEW_WINDOW_MIN_HEIGHT: f64 = 120.0;
const REVIEW_WINDOW_MAX_HEIGHT: f64 = 920.0;

/// How long to keep the review webview alive after it's hidden so the next
/// invocation can reuse it (avoiding the 484 KB bundle re-load). After this
/// window elapses without another show, the webview is destroyed to free RAM.
///
/// Capped at 5 minutes because macOS may suspend long-idle webview processes,
/// after which the kept-alive instance still answers `get_webview_window()`
/// but its JS event loop no longer fires `review_window_ready` /
/// `review_window_content_ready` — so the window is created hidden and never
/// shown. Forcing a destroy + recreate after 5 min keeps the warm-cache win
/// for tight back-to-back recordings while sidestepping the dormancy bug.
const REVIEW_WINDOW_IDLE_DESTROY_SECS: u64 = 5 * 60;

/// Bumped every time the review window is shown or explicitly destroyed.
/// The idle-destroy timer captures the value at hide time and bails out if
/// anything else touched the window in the meantime.
static REVIEW_WINDOW_LIFECYCLE_GEN: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, serde::Serialize)]
struct ReviewWindowPayload {
    source_text: String,
    final_text: String,
    change_percent: i8,
    history_id: Option<i64>,
    reason: Option<String>,
    output_mode: PromptOutputMode,
    skill_name: Option<String>,
    /// The prompt_id that was actually used for this transcription (may be overridden by app rules)
    prompt_id: Option<String>,
    /// The model_id that was actually used for this transcription
    model_id: Option<String>,
}

/// Multi-model post-processing candidate result
#[derive(Clone, serde::Serialize)]
pub struct MultiModelCandidate {
    pub id: String,
    pub label: String,
    pub provider_label: String,
    pub text: String,
    pub confidence: Option<u8>,
    pub processing_time_ms: u64,
    pub error: Option<String>,
    pub ready: bool,
}

/// Review window payload for multi-candidate mode
#[allow(dead_code)]
#[derive(Clone, serde::Serialize)]
pub struct ReviewWindowMultiCandidatePayload {
    pub source_text: String,
    pub candidates: Vec<MultiModelCandidate>,
    pub history_id: Option<i64>,
    pub output_mode: PromptOutputMode,
    pub skill_name: Option<String>,
    /// The prompt_id that was actually used for this transcription (may be overridden by app rules)
    pub prompt_id: Option<String>,
    /// ID of the candidate that should be pre-selected (strategy-dependent).
    pub auto_selected_id: Option<String>,
}

#[derive(Clone, serde::Serialize)]
#[allow(dead_code)]
struct ReviewWindowHidePayload {
    history_id: Option<i64>,
}

static REVIEW_WINDOW_READY: AtomicBool = AtomicBool::new(false);
/// Monotonic sequence for payloads emitted directly into an already-READY
/// webview. `review_window_content_ready` acks by copying the current value
/// into REVIEW_CONTENT_ACK_SEQ; the show watchdog recovers when the ack never
/// lands (dormant webview — see REVIEW_WINDOW_IDLE_DESTROY_SECS comment).
static REVIEW_SHOW_ATTEMPT_SEQ: AtomicU64 = AtomicU64::new(0);
static REVIEW_CONTENT_ACK_SEQ: AtomicU64 = AtomicU64::new(0);
/// How long to wait for `review_window_content_ready` after a direct payload
/// emit before declaring the webview dormant and recreating it.
const REVIEW_CONTENT_ACK_TIMEOUT_MS: u64 = 1500;
/// How long a freshly created webview gets to fire `review_window_ready`
/// before the boot watchdog destroys and recreates it. Cold start is normally
/// 300-700 ms; 4 s leaves slack for slow disks / first-launch JIT.
const REVIEW_READY_TIMEOUT_MS: u64 = 4000;
/// Consecutive boot-watchdog recreate attempts. Reset when ready fires.
/// Capped so a structurally broken frontend bundle doesn't cause an endless
/// destroy/recreate churn loop.
static REVIEW_BOOT_WATCHDOG_RETRIES: AtomicU64 = AtomicU64::new(0);
static REVIEW_WINDOW_FORCED_ACTIVATION: AtomicBool = AtomicBool::new(false);
static REVIEW_WINDOW_FOCUS_TOKEN: AtomicU64 = AtomicU64::new(0);
static REVIEW_WINDOW_ACTIVE: AtomicBool = AtomicBool::new(false);
/// True from the moment a `review-window-local` (voice rewrite) recording
/// starts until its async transcribe + post-process pipeline finishes (success,
/// cancel, or panic). Used by `cancel_current_operation` to keep the review
/// window open when the user hits ESC to abort the rewrite — the prior review
/// content should survive a cancelled rewrite attempt.
static REVIEW_REWRITE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static HIDDEN_WINDOWS_BEFORE_REVIEW: Lazy<Mutex<HashSet<String>>> =
    Lazy::new(|| Mutex::new(HashSet::new()));
static PENDING_REVIEW_PAYLOAD: Lazy<Mutex<Option<ReviewWindowPayload>>> =
    Lazy::new(|| Mutex::new(None));
static PENDING_MULTI_CANDIDATE_PAYLOAD: Lazy<Mutex<Option<ReviewWindowMultiCandidatePayload>>> =
    Lazy::new(|| Mutex::new(None));
static LAST_REVIEW_PAYLOAD: Lazy<Mutex<Option<ReviewWindowPayload>>> =
    Lazy::new(|| Mutex::new(None));
static LAST_REVIEW_HISTORY_ID: Lazy<Mutex<Option<i64>>> = Lazy::new(|| Mutex::new(None));
static LAST_ACTIVE_WINDOW: Lazy<Mutex<Option<ActiveWindowInfo>>> = Lazy::new(|| Mutex::new(None));
static REVIEW_EDITOR_ACTIVE: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));
static REVIEW_EDITOR_CONTENT: Lazy<Mutex<String>> = Lazy::new(|| Mutex::new(String::new()));
static FROZEN_REVIEW_EDITOR_CONTENT: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

/// Role in a rewrite conversation turn.
#[derive(Clone, Debug)]
pub enum RewriteRole {
    User,
    Assistant,
}

/// A single message in the rewrite conversation history.
#[derive(Clone, Debug)]
pub struct RewriteMessage {
    pub role: RewriteRole,
    pub content: String,
}

/// Session-scoped conversation history for multi-turn voice rewrite.
struct RewriteConversation {
    session_id: u64,
    messages: Vec<RewriteMessage>,
}

static REWRITE_SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);
static REWRITE_COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static REWRITE_CONVERSATION: Lazy<Mutex<RewriteConversation>> = Lazy::new(|| {
    Mutex::new(RewriteConversation {
        session_id: 0,
        messages: Vec::new(),
    })
});

fn emit_review_payload(app_handle: &AppHandle, payload: ReviewWindowPayload) -> bool {
    if let Some(review_window) = app_handle.get_webview_window("review_window") {
        let emit_result = review_window.emit("review-window-show", payload);
        debug!("review_window.emit() result: {:?}", emit_result);
        if emit_result.is_ok() {
            REVIEW_WINDOW_ACTIVE.store(true, Ordering::SeqCst);
            return true;
        }
        return false;
    }

    error!("review_window not found! Window may not have been created.");
    false
}

fn estimate_line_count(text: &str, line_chars: usize) -> usize {
    let mut lines = 0usize;
    for line in text.lines() {
        let count = line.chars().count();
        let wrapped = (count / line_chars).max(1);
        lines += wrapped;
    }
    if lines == 0 {
        1
    } else {
        lines
    }
}

fn estimate_window_width(source_text: &str, final_text: &str) -> f64 {
    let max_line_len = source_text
        .lines()
        .chain(final_text.lines())
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    if max_line_len == 0 {
        return REVIEW_WINDOW_WIDTH;
    }
    let estimated = (max_line_len as f64 * 9.2 + 70.0).min(REVIEW_WINDOW_MAX_WIDTH);
    estimated.clamp(REVIEW_WINDOW_MIN_WIDTH, REVIEW_WINDOW_MAX_WIDTH)
}

fn estimate_window_height(source_text: &str, final_text: &str, width: f64) -> f64 {
    let text_width = (width - 70.0).max(200.0); // subtract panel margins
    let line_chars = (text_width / 9.2).floor().max(18.0) as usize;
    let source_lines = estimate_line_count(source_text, line_chars);
    let final_lines = estimate_line_count(final_text, line_chars);

    // Source inline frame: padding(14) + body(lines*20, max 120) + border(2)
    let source_body = (source_lines as f64 * 20.0).min(120.0);
    let source_frame = 14.0 + source_body + 2.0;
    // Output panel: header(30) + body(lines*24) + border(2)
    let output_panel = 30.0 + final_lines as f64 * 24.0 + 2.0;
    // Total: header(40) + content-padding(8) + source + gap(8) + output + footer(32)
    let total = 40.0 + 8.0 + source_frame + 8.0 + output_panel + 32.0;
    total.clamp(REVIEW_WINDOW_MIN_HEIGHT, REVIEW_WINDOW_MAX_HEIGHT)
}

fn was_app_active_before_review() -> bool {
    let pid = std::process::id() as u64;
    let last = LAST_ACTIVE_WINDOW.lock().unwrap();
    last.as_ref()
        .map(|info| info.process_id == pid)
        .unwrap_or(false)
}

fn record_hidden_windows(app_handle: &AppHandle) -> bool {
    let app_was_active = was_app_active_before_review();
    let mut hidden = HIDDEN_WINDOWS_BEFORE_REVIEW.lock().unwrap();
    hidden.clear();
    let mut any_visible = false;
    for (label, window) in app_handle.webview_windows() {
        // Never touch the review window itself.
        if label == "review_window" {
            continue;
        }
        // The recording overlay is intentionally always OS-visible (visibility
        // is controlled in-webview via CSS). If we add it to the hidden list
        // here, `schedule_hide_windows` will OS-hide it and the next show path
        // has to pay the slow Cocoa window.show() cost on Cmd+Enter.
        if label == "recording_overlay" {
            continue;
        }
        let is_visible = window.is_visible().unwrap_or(false);
        if is_visible {
            any_visible = true;
        }
        if !app_was_active || !is_visible {
            hidden.insert(label);
        }
    }
    any_visible
}

fn schedule_hide_windows(app_handle: AppHandle) {
    let labels: Vec<String> = {
        let hidden = HIDDEN_WINDOWS_BEFORE_REVIEW.lock().unwrap();
        hidden.iter().cloned().collect()
    };
    if labels.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for delay in [0_u64, 120, 350] {
            if delay > 0 {
                std::thread::sleep(Duration::from_millis(delay));
            }
            for label in &labels {
                if let Some(window) = app_handle.get_webview_window(label) {
                    if window.is_visible().unwrap_or(false) {
                        let _ = window.hide();
                    }
                }
            }
        }
    });
}

fn schedule_focus_review_window(app_handle: AppHandle, focus_token: u64) {
    std::thread::spawn(move || {
        for delay in [Duration::from_millis(120), Duration::from_millis(350)] {
            std::thread::sleep(delay);
            if let Some(window) = app_handle.get_webview_window("review_window") {
                if REVIEW_WINDOW_FOCUS_TOKEN.load(Ordering::SeqCst) != focus_token {
                    return;
                }
                if window.is_visible().unwrap_or(false) {
                    let _ = window.set_focus();
                }
            }
        }
    });
}

#[cfg(target_os = "macos")]
fn ensure_app_active_for_review(app_handle: &AppHandle, had_visible_windows: bool) {
    if had_visible_windows {
        log::info!(
            "[post-insert-diag] ensure_app_active_for_review: skip (had_visible_windows=true)"
        );
        return;
    }
    match app_handle.set_activation_policy(tauri::ActivationPolicy::Regular) {
        Ok(_) => {
            REVIEW_WINDOW_FORCED_ACTIVATION.store(true, Ordering::SeqCst);
            log::info!(
                "[post-insert-diag] ensure_app_active_for_review: policy=Regular (forced_activation=true)"
            );
        }
        Err(e) => {
            log::warn!(
                "[post-insert-diag] ensure_app_active_for_review: set_activation_policy(Regular) failed: {}",
                e
            );
        }
    }
}

#[allow(dead_code)]
fn maybe_restore_activation_policy(app_handle: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        if !REVIEW_WINDOW_FORCED_ACTIVATION.swap(false, Ordering::SeqCst) {
            log::info!(
                "[post-insert-diag] maybe_restore_activation_policy: skip (forced_activation was false)"
            );
            return;
        }
        let settings = crate::settings::get_settings(app_handle);
        if !settings.start_hidden {
            log::info!(
                "[post-insert-diag] maybe_restore_activation_policy: skip (start_hidden=false)"
            );
            return;
        }
        let has_visible_windows = app_handle.webview_windows().iter().any(|(label, window)| {
            label != "review_window" && window.is_visible().unwrap_or(false)
        });
        if !has_visible_windows {
            match app_handle.set_activation_policy(tauri::ActivationPolicy::Accessory) {
                Ok(_) => log::info!(
                    "[post-insert-diag] maybe_restore_activation_policy: policy=Accessory (restored)"
                ),
                Err(e) => log::warn!(
                    "[post-insert-diag] maybe_restore_activation_policy: set_activation_policy(Accessory) failed: {}",
                    e
                ),
            }
        } else {
            log::info!(
                "[post-insert-diag] maybe_restore_activation_policy: skip (other Votype windows visible)"
            );
        }
    }
}

/// Get the logical size of the monitor containing the cursor (or primary).
fn get_screen_logical_size(window: &tauri::WebviewWindow) -> (f64, f64) {
    let monitor = window.primary_monitor().ok().flatten();
    if let Some(m) = monitor {
        let scale = m.scale_factor();
        let size = m.size();
        return (size.width as f64 / scale, size.height as f64 / scale);
    }
    (1440.0, 900.0) // sensible fallback
}

fn position_window_near_cursor(window: &tauri::WebviewWindow, width: f64, height: f64) {
    use crate::active_window::fetch_cursor_position;

    let mut target_monitor = None;

    // 1. Try to find the monitor containing the cursor
    if let Ok(cursor) = fetch_cursor_position() {
        if let Ok(available_monitors) = window.available_monitors() {
            for monitor in available_monitors {
                let scale = monitor.scale_factor();
                // Enigo returns logical coordinates (points), so we must compare against logical monitor bounds
                let logical_pos = monitor.position().to_logical::<f64>(scale);
                let logical_size = monitor.size().to_logical::<f64>(scale);

                let x_start = logical_pos.x;
                let y_start = logical_pos.y;
                let x_end = x_start + logical_size.width;
                let y_end = y_start + logical_size.height;

                let cx = cursor.x as f64;
                let cy = cursor.y as f64;

                if cx >= x_start && cx < x_end && cy >= y_start && cy < y_end {
                    target_monitor = Some(monitor);
                    break;
                }
            }
        }
    }

    // 2. Fallback to primary monitor if not found
    if target_monitor.is_none() {
        target_monitor = window.primary_monitor().ok().flatten();
    }

    // 3. Position the window
    if let Some(monitor) = target_monitor {
        let scale = monitor.scale_factor();
        let position = monitor.position();
        let monitor_size = monitor.size();
        let monitor_width = monitor_size.width as f64 / scale;
        let monitor_height = monitor_size.height as f64 / scale;

        // Calculate centered position relative to the monitor's top-left
        let x = (monitor_width - width) / 2.0 + position.x as f64 / scale;
        let y = (monitor_height - height) / 2.0 + position.y as f64 / scale;

        let _ = window.set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
    }
}

/// Creates a fresh review window (hidden). Returns true on success.
fn ensure_review_window(app_handle: &AppHandle) -> bool {
    // Already exists – nothing to do
    if app_handle.get_webview_window("review_window").is_some() {
        return true;
    }

    // Reset ready flag so we wait for the new webview to initialise
    REVIEW_WINDOW_READY.store(false, Ordering::SeqCst);

    match tauri::WebviewWindowBuilder::new(
        app_handle,
        "review_window",
        tauri::WebviewUrl::App("src/review/index.html".into()),
    )
    .on_page_load(|_window, payload| {
        log::info!(
            "[ReviewWindow] page load {:?}: {}",
            payload.event(),
            payload.url()
        );
    })
    .title("Votype Review")
    .resizable(true)
    .inner_size(REVIEW_WINDOW_WIDTH, REVIEW_WINDOW_HEIGHT)
    .shadow(true)
    .min_inner_size(REVIEW_WINDOW_MIN_WIDTH, REVIEW_WINDOW_MIN_HEIGHT)
    .maximizable(false)
    .minimizable(false)
    .closable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .transparent(true)
    .visible(false)
    .accept_first_mouse(true)
    // 关键：prewarm 阶段创建窗口时 .focused(true) 会在 macOS 上让整个 Votype app
    // 被激活，把 settings 窗口连带拉到前台、抢走用户原 app 的焦点。后续的
    // show_review_window() 会显式 show() + set_focus()，所以这里设 false 不会
    // 破坏 review 流程，只是不在 prewarm 阶段触发 app activation。
    .focused(false)
    .build()
    {
        Ok(_window) => {
            debug!("Review window created on demand");
            true
        }
        Err(e) => {
            error!("Failed to create review window: {}", e);
            false
        }
    }
}

/// Destroys the review window webview to free memory (~80 MB).
fn destroy_review_window(app_handle: &AppHandle) {
    REVIEW_WINDOW_LIFECYCLE_GEN.fetch_add(1, Ordering::SeqCst);
    if let Some(window) = app_handle.get_webview_window("review_window") {
        let _ = window.hide();
        let _ = window.destroy();
        debug!("Review window destroyed");
    }
    REVIEW_WINDOW_READY.store(false, Ordering::SeqCst);
}

/// Hide the review webview but keep the JS context alive so subsequent
/// `show_review_window` calls can skip the 300-700 ms cold-start. Schedules
/// a lazy destroy after `REVIEW_WINDOW_IDLE_DESTROY_SECS` of idleness to
/// reclaim the ~80 MB RAM footprint eventually.
fn hide_review_window_keep_alive(app_handle: &AppHandle, history_id: Option<i64>) {
    let gen_at_hide = REVIEW_WINDOW_LIFECYCLE_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    log::debug!(
        "[overlay-trace] hide_review_window_keep_alive ENTER gen={}",
        gen_at_hide
    );

    if let Some(window) = app_handle.get_webview_window("review_window") {
        // Tell the React side to unmount the ReviewWindow component so
        // effects (ResizeObserver, translation debounce, etc.) stop running
        // while the webview is parked between sessions.
        let _ = window.emit(
            "review-window-hide",
            serde_json::json!({ "history_id": history_id }),
        );
        let hide_result = window.hide();
        log::debug!(
            "[overlay-trace] review_window.hide() result={:?}",
            hide_result
        );
    }

    let app_for_destroy = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(REVIEW_WINDOW_IDLE_DESTROY_SECS)).await;
        // If anything touched the window since we scheduled this destroy,
        // bail out and let the newer lifecycle owner handle it.
        if REVIEW_WINDOW_LIFECYCLE_GEN.load(Ordering::SeqCst) != gen_at_hide {
            return;
        }
        if REVIEW_WINDOW_ACTIVE.load(Ordering::SeqCst) {
            return;
        }
        destroy_review_window(&app_for_destroy);
    });
}

/// Shows the review window with the provided data
pub fn show_review_window(
    app_handle: &AppHandle,
    source_text: String,
    final_text: String,
    change_percent: i8,
    history_id: Option<i64>,
    reason: Option<String>,
    output_mode: PromptOutputMode,
    skill_name: Option<String>,
    prompt_id: Option<String>,
    model_id: Option<String>,
) {
    reset_rewrite_conversation();
    reset_rewrite_count();
    let had_visible_windows = record_hidden_windows(app_handle);
    #[cfg(target_os = "macos")]
    ensure_app_active_for_review(app_handle, had_visible_windows);
    debug!(
        "show_review_window called with change_percent: {}, text length: {}",
        change_percent,
        final_text.len()
    );
    let preview: String = final_text.chars().take(80).collect();
    log::info!(
        "review_window payload: history_id={:?}, change_percent={}, skill={:?}, preview=\"{}\"",
        history_id,
        change_percent,
        skill_name,
        preview
    );

    let source_for_layout = source_text.clone();
    let final_for_layout = final_text.clone();

    let payload = ReviewWindowPayload {
        source_text,
        final_text,
        change_percent,
        history_id,
        reason,
        output_mode,
        skill_name,
        prompt_id,
        model_id,
    };

    // Store payload and mark active BEFORE creating the window.
    // This way review_window_ready() can replay the payload once JS loads.
    {
        let mut pending = PENDING_REVIEW_PAYLOAD.lock().unwrap();
        *pending = Some(payload.clone());
    }
    {
        // Clear any pending multi-candidate payload
        let mut mc = PENDING_MULTI_CANDIDATE_PAYLOAD.lock().unwrap();
        *mc = None;
    }
    {
        let mut last_payload = LAST_REVIEW_PAYLOAD.lock().unwrap();
        *last_payload = Some(payload.clone());
    }
    {
        let mut last_id = LAST_REVIEW_HISTORY_ID.lock().unwrap();
        *last_id = history_id;
    }
    REVIEW_WINDOW_ACTIVE.store(true, Ordering::SeqCst);

    if !ensure_review_window(app_handle) {
        return;
    }

    if let Some(review_window) = app_handle.get_webview_window("review_window") {
        debug!("Found review_window, setting size/position and emitting event...");

        // Pre-calculate and set window size/position (but don't show yet)
        let width = estimate_window_width(&source_for_layout, &final_for_layout);
        let height = estimate_window_height(&source_for_layout, &final_for_layout, width);
        let _ = review_window.set_size(tauri::Size::Logical(tauri::LogicalSize { width, height }));
        position_window_near_cursor(&review_window, width, height);

        // If JS is already loaded, emit immediately; otherwise review_window_ready() will replay
        if REVIEW_WINDOW_READY.load(Ordering::SeqCst) {
            let _ = emit_review_payload(app_handle, payload);
            spawn_content_ack_watchdog(app_handle.clone());
        } else {
            spawn_ready_watchdog(app_handle.clone());
        }

        schedule_hide_windows(app_handle.clone());
    }
}

/// Watchdog for the fresh-create path: the payload sits in PENDING waiting
/// for the new webview to fire `review_window_ready`. If the webview never
/// initialises (load failure, suspended WebContent process, broken bundle),
/// nothing would ever show. Destroy and recreate up to 2 times, then give up
/// loudly so the log points at a frontend-side init failure.
fn spawn_ready_watchdog(app_handle: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(REVIEW_READY_TIMEOUT_MS)).await;
        if REVIEW_WINDOW_READY.load(Ordering::SeqCst) {
            REVIEW_BOOT_WATCHDOG_RETRIES.store(0, Ordering::SeqCst);
            return;
        }
        if !REVIEW_WINDOW_ACTIVE.load(Ordering::SeqCst) {
            return;
        }
        let prior_retries = REVIEW_BOOT_WATCHDOG_RETRIES.fetch_add(1, Ordering::SeqCst);
        if prior_retries >= 2 {
            log::error!(
                "[ReviewWindow] webview never fired review_window_ready after {} recreate attempts — giving up. Frontend bundle is likely failing to initialise; check the review webview console.",
                prior_retries
            );
            return;
        }
        log::warn!(
            "[ReviewWindow] review_window_ready not received within {}ms of creation — recreating webview (attempt {})",
            REVIEW_READY_TIMEOUT_MS,
            prior_retries + 1
        );
        destroy_review_window(&app_handle);
        if ensure_review_window(&app_handle) {
            spawn_ready_watchdog(app_handle);
        } else {
            log::error!("[ReviewWindow] boot watchdog failed to recreate review window");
        }
    });
}

/// macOS may suspend a kept-alive review webview: the window handle stays
/// valid and REVIEW_WINDOW_READY stays true, but the JS event loop is dead —
/// a directly-emitted payload never produces `review_window_content_ready`,
/// so `window.show()` is never called and the review window silently fails
/// to appear. Worse, the failed show bumps the lifecycle gen and cancels the
/// pending idle-destroy, so every subsequent show fails too.
///
/// This watchdog converts that silent failure into recovery: if no
/// content_ready ack arrives within the timeout, destroy the dormant webview
/// and recreate it — the fresh webview fires `review_window_ready`, which
/// replays the still-pending payload, then content_ready shows the window.
fn spawn_content_ack_watchdog(app_handle: AppHandle) {
    let attempt = REVIEW_SHOW_ATTEMPT_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(REVIEW_CONTENT_ACK_TIMEOUT_MS)).await;
        // Review already confirmed/dismissed — nothing to recover.
        if !REVIEW_WINDOW_ACTIVE.load(Ordering::SeqCst) {
            return;
        }
        // Webview answered — healthy.
        if REVIEW_CONTENT_ACK_SEQ.load(Ordering::SeqCst) >= attempt {
            return;
        }
        // A newer show attempt supersedes this watchdog.
        if REVIEW_SHOW_ATTEMPT_SEQ.load(Ordering::SeqCst) != attempt {
            return;
        }
        log::warn!(
            "[ReviewWindow] content_ready not received within {}ms — webview likely dormant, destroying and recreating",
            REVIEW_CONTENT_ACK_TIMEOUT_MS
        );
        destroy_review_window(&app_handle);
        if !ensure_review_window(&app_handle) {
            log::error!("[ReviewWindow] watchdog failed to recreate review window");
        }
        // The fresh webview will fire review_window_ready → replay the pending
        // payload → content_ready → show. Frontend re-measures and repositions
        // on replay, so no size/position work is needed here.
    });
}

/// Hides and destroys the review window to free memory
#[allow(dead_code)]
pub fn hide_review_window(app_handle: &AppHandle, history_id: Option<i64>) {
    hide_review_window_keep_alive(app_handle, history_id);
    REVIEW_WINDOW_ACTIVE.store(false, Ordering::SeqCst);
    {
        let mut pending = PENDING_REVIEW_PAYLOAD.lock().unwrap();
        *pending = None;
    }
    {
        let mut mc = PENDING_MULTI_CANDIDATE_PAYLOAD.lock().unwrap();
        *mc = None;
    }
    {
        let mut last_payload = LAST_REVIEW_PAYLOAD.lock().unwrap();
        *last_payload = None;
    }
    REVIEW_WINDOW_FOCUS_TOKEN.fetch_add(1, Ordering::SeqCst);
    schedule_hide_windows(app_handle.clone());
    maybe_restore_activation_policy(app_handle);
    reset_rewrite_conversation();
    let mut hidden = HIDDEN_WINDOWS_BEFORE_REVIEW.lock().unwrap();
    hidden.clear();
}

#[tauri::command]
pub fn review_window_ready(app: AppHandle) -> Result<(), String> {
    REVIEW_WINDOW_READY.store(true, Ordering::SeqCst);
    REVIEW_BOOT_WATCHDOG_RETRIES.store(0, Ordering::SeqCst);
    log::info!("review_window_ready received");

    if !REVIEW_WINDOW_ACTIVE.load(Ordering::SeqCst) {
        return Ok(());
    }

    // Replay pending multi-candidate payload (takes priority if set)
    let mc_payload = PENDING_MULTI_CANDIDATE_PAYLOAD.lock().unwrap().take();
    if let Some(payload) = mc_payload {
        log::info!(
            "review_window_ready replaying multi-candidate payload: history_id={:?}, {} candidates",
            payload.history_id,
            payload.candidates.len()
        );
        if let Some(review_window) = app.get_webview_window("review_window") {
            let _ = review_window.emit("review-window-multi-candidate", &payload);
            // Multi-candidate mode shows window immediately (no content_ready handshake)
            let focus_token = REVIEW_WINDOW_FOCUS_TOKEN.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = review_window.show();
            let _ = review_window.set_focus();
            schedule_focus_review_window(app.clone(), focus_token);
        }
        return Ok(());
    }

    // Replay pending single-model payload
    let payload = PENDING_REVIEW_PAYLOAD.lock().unwrap().take();
    if let Some(payload) = payload {
        log::info!(
            "review_window_ready replaying payload: history_id={:?}, change_percent={}",
            payload.history_id,
            payload.change_percent
        );
        let _ = emit_review_payload(&app, payload);
    }

    Ok(())
}

/// Called by frontend when content is rendered and ready to be shown
#[tauri::command]
pub fn review_window_content_ready(app: AppHandle) -> Result<(), String> {
    log::info!("review_window_content_ready received");

    // Ack the show watchdog: any content_ready proves the JS event loop is
    // alive (and the handler below shows the window when a payload is active).
    REVIEW_CONTENT_ACK_SEQ.store(
        REVIEW_SHOW_ATTEMPT_SEQ.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );

    if !REVIEW_WINDOW_ACTIVE.load(Ordering::SeqCst) {
        log::info!("review_window_content_ready ignored (no active payload)");
        return Ok(());
    }

    if let Some(review_window) = app.get_webview_window("review_window") {
        let focus_token = REVIEW_WINDOW_FOCUS_TOKEN.fetch_add(1, Ordering::SeqCst) + 1;

        let show_result = review_window.show();
        debug!("review_window.show() result: {:?}", show_result);
        let focus_result = review_window.set_focus();
        debug!("review_window.set_focus() result: {:?}", focus_result);
        schedule_focus_review_window(app.clone(), focus_token);
    }

    Ok(())
}

/// Called by frontend to resize the review window after measuring actual DOM content.
/// When `reposition` is true, the window is re-centered on the cursor's monitor (initial show).
/// When false, only size changes (e.g. after prompt switch).
#[tauri::command]
pub fn resize_review_window(
    app: AppHandle,
    width: f64,
    height: f64,
    reposition: bool,
) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("review_window") {
        // Dynamic max height tracks the current monitor so the window can grow
        // with content (e.g. long polish + English preview) instead of clipping
        // at a fixed 920px. We still leave ~80px for menu bar / dock.
        let (_screen_w, screen_h) = get_screen_logical_size(&window);
        let dynamic_max_h = (screen_h - 80.0).max(REVIEW_WINDOW_MIN_HEIGHT);

        let w = width.clamp(REVIEW_WINDOW_MIN_WIDTH, REVIEW_WINDOW_MAX_WIDTH);
        let h = height.clamp(REVIEW_WINDOW_MIN_HEIGHT, dynamic_max_h);
        let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize {
            width: w,
            height: h,
        }));
        if reposition {
            position_window_near_cursor(&window, w, h);
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub fn get_last_review_history_id() -> Option<i64> {
    let last_id = LAST_REVIEW_HISTORY_ID.lock().unwrap();
    *last_id
}

pub fn set_last_active_window(info: Option<ActiveWindowInfo>) {
    let mut last_window = LAST_ACTIVE_WINDOW.lock().unwrap();
    *last_window = info;
}

pub fn get_last_active_window() -> Option<ActiveWindowInfo> {
    let last_window = LAST_ACTIVE_WINDOW.lock().unwrap();
    last_window.clone()
}

pub fn is_review_window_active() -> bool {
    REVIEW_WINDOW_ACTIVE.load(Ordering::SeqCst)
}

pub fn set_review_rewrite_in_progress(active: bool) {
    REVIEW_REWRITE_IN_PROGRESS.store(active, Ordering::SeqCst);
}

pub fn is_review_rewrite_in_progress() -> bool {
    REVIEW_REWRITE_IN_PROGRESS.load(Ordering::SeqCst)
}

pub fn set_review_editor_active(active: bool) {
    if let Ok(mut guard) = REVIEW_EDITOR_ACTIVE.lock() {
        *guard = active;
    }
}

pub fn is_review_editor_active() -> bool {
    REVIEW_EDITOR_ACTIVE
        .lock()
        .map(|guard| *guard)
        .unwrap_or(false)
}

pub fn set_review_editor_content(text: String) {
    if let Ok(mut guard) = REVIEW_EDITOR_CONTENT.lock() {
        *guard = text;
    }
}

pub fn current_review_editor_content() -> Option<String> {
    REVIEW_EDITOR_CONTENT
        .lock()
        .ok()
        .map(|guard| guard.trim().to_string())
        .filter(|text| !text.is_empty())
}

pub fn freeze_review_editor_content_snapshot() {
    let snapshot = current_review_editor_content();
    debug!(
        "[ReviewWindow] freeze_snapshot: content_len={:?}",
        snapshot.as_ref().map(|s| s.len())
    );
    if let Ok(mut guard) = FROZEN_REVIEW_EDITOR_CONTENT.lock() {
        *guard = snapshot;
    }
}

pub fn take_frozen_review_editor_content() -> Option<String> {
    FROZEN_REVIEW_EDITOR_CONTENT
        .lock()
        .ok()
        .and_then(|mut guard| guard.take())
}

/// Get the current rewrite session ID.
pub fn current_rewrite_session_id() -> u64 {
    REWRITE_SESSION_COUNTER.load(Ordering::SeqCst)
}

/// Get conversation history, only if the session ID matches.
pub fn get_rewrite_conversation(session_id: u64) -> Option<Vec<RewriteMessage>> {
    REWRITE_CONVERSATION.lock().ok().and_then(|guard| {
        if guard.session_id == session_id {
            Some(guard.messages.clone())
        } else {
            None
        }
    })
}

/// Append a message to the conversation history, only if the session ID matches.
pub fn append_rewrite_message(session_id: u64, role: RewriteRole, content: String) {
    if let Ok(mut guard) = REWRITE_CONVERSATION.lock() {
        if guard.session_id == session_id {
            guard.messages.push(RewriteMessage { role, content });
        }
    }
}

/// Clear conversation history and start a new session.
fn reset_rewrite_conversation() {
    let new_id = REWRITE_SESSION_COUNTER.fetch_add(1, Ordering::SeqCst) + 1;
    if let Ok(mut guard) = REWRITE_CONVERSATION.lock() {
        guard.session_id = new_id;
        guard.messages.clear();
    }
}

/// Increment the rewrite count for the current review session and return the new value.
pub fn increment_rewrite_count() -> u32 {
    REWRITE_COUNT.fetch_add(1, Ordering::SeqCst) + 1
}

/// Reset the rewrite count (called when a new review session starts).
fn reset_rewrite_count() {
    REWRITE_COUNT.store(0, Ordering::SeqCst);
}

/// Shows the review window with multiple model candidates for selection
pub fn show_review_window_with_candidates(
    app_handle: &AppHandle,
    source_text: String,
    candidates: Vec<MultiModelCandidate>,
    history_id: Option<i64>,
    output_mode: PromptOutputMode,
    skill_name: Option<String>,
    prompt_id: Option<String>,
    auto_selected_id: Option<String>,
) {
    reset_rewrite_conversation();
    reset_rewrite_count();
    let had_visible_windows = record_hidden_windows(app_handle);
    #[cfg(target_os = "macos")]
    ensure_app_active_for_review(app_handle, had_visible_windows);

    debug!(
        "show_review_window_with_candidates called with {} candidates",
        candidates.len()
    );

    {
        let mut last_id = LAST_REVIEW_HISTORY_ID.lock().unwrap();
        *last_id = history_id;
    }

    let candidate_count = candidates.len() as f64;
    let char_count = source_text.chars().count();

    let payload = ReviewWindowMultiCandidatePayload {
        source_text,
        candidates,
        history_id,
        output_mode,
        skill_name,
        prompt_id,
        auto_selected_id,
    };

    // Store payload and mark active BEFORE creating window
    {
        let mut mc = PENDING_MULTI_CANDIDATE_PAYLOAD.lock().unwrap();
        *mc = Some(payload.clone());
    }
    {
        // Clear any pending single-model payload
        let mut pending = PENDING_REVIEW_PAYLOAD.lock().unwrap();
        *pending = None;
    }
    {
        let mut last_payload = LAST_REVIEW_PAYLOAD.lock().unwrap();
        *last_payload = None;
    }
    REVIEW_WINDOW_ACTIVE.store(true, Ordering::SeqCst);

    if !ensure_review_window(app_handle) {
        return;
    }

    if let Some(review_window) = app_handle.get_webview_window("review_window") {
        debug!("Found review_window, emitting multi-candidate event...");
        let (screen_w, screen_h) = get_screen_logical_size(&review_window);

        // Width: use 65% of screen width for multi-candidate, generous space for text
        let width = (screen_w * 0.65).clamp(680.0, REVIEW_WINDOW_MAX_WIDTH);

        // Height: (n+1) * per_item
        // Base 110px per slot; add 20px per 100 chars of source text for longer content
        let extra = (char_count / 100) as f64 * 20.0;
        let per_item = 110.0 + extra;
        let desired_height = (candidate_count + 1.0) * per_item;
        let height = desired_height.clamp(REVIEW_WINDOW_MIN_HEIGHT, screen_h * 0.85);

        let _ = review_window.set_size(tauri::Size::Logical(tauri::LogicalSize { width, height }));
        position_window_near_cursor(&review_window, width, height);

        // If JS is already loaded, emit and show immediately; otherwise review_window_ready() will replay
        if REVIEW_WINDOW_READY.load(Ordering::SeqCst) {
            let emit_result = review_window.emit("review-window-multi-candidate", &payload);
            debug!(
                "review_window.emit() multi-candidate result: {:?}",
                emit_result
            );

            let focus_token = REVIEW_WINDOW_FOCUS_TOKEN.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = review_window.show();
            let _ = review_window.set_focus();
            schedule_focus_review_window(app_handle.clone(), focus_token);
        } else {
            spawn_ready_watchdog(app_handle.clone());
        }

        schedule_hide_windows(app_handle.clone());
    }
}
