use crate::input;
use crate::settings;
use crate::settings::OverlayPosition;
use tauri::{AppHandle, Emitter, Manager};

#[cfg(not(target_os = "macos"))]
use log::debug;

#[cfg(not(target_os = "macos"))]
use tauri::WebviewWindowBuilder;

#[cfg(target_os = "macos")]
use tauri::WebviewUrl;

#[cfg(target_os = "macos")]
use tauri_nspanel::{tauri_panel, CollectionBehavior, PanelBuilder, PanelLevel};

#[cfg(target_os = "macos")]
tauri_panel! {
    panel!(RecordingOverlayPanel {
        config: {
            // Allow becoming key window so we can receive keyboard input for skill confirmation
            can_become_key_window: true,
            is_floating_panel: true
        }
    })
}

// This is the *window* size; the overlay UI inside can be smaller and is centered.
// We keep this large enough to support realtime transcription with internal scrolling.
const OVERLAY_WIDTH: f64 = 540.0;
const OVERLAY_HEIGHT: f64 = 300.0;
const CURSOR_VERTICAL_OFFSET: f64 = 18.0;

/// Monotonically-increasing generation bumped by every `show_*`/`hide_*`
/// overlay transition. Used so a delayed "success" → hide can bail out if a
/// new overlay state has taken over (e.g. user started a fresh recording
/// while the success checkmark was still being displayed).
static OVERLAY_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn bump_overlay_generation() -> u64 {
    OVERLAY_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
}

pub fn current_overlay_generation() -> u64 {
    OVERLAY_GENERATION.load(std::sync::atomic::Ordering::SeqCst)
}

/// Session-scoped paste-target app. Snapshotted at recording start (so the
/// overlay can show the target app's icon all the way from "录音中" through
/// "后处理中" to the eventual "翻译成功 ✓"). Refreshed by the translation
/// insert path via `show_translation_overlay_fast(..., Some(app))`.
static OVERLAY_TARGET_APP: once_cell::sync::Lazy<std::sync::Mutex<Option<String>>> =
    once_cell::sync::Lazy::new(|| std::sync::Mutex::new(None));

fn set_overlay_target_app(name: Option<String>) {
    if let Ok(mut slot) = OVERLAY_TARGET_APP.lock() {
        *slot = name;
    }
}

fn current_overlay_target_app() -> Option<String> {
    OVERLAY_TARGET_APP.lock().ok().and_then(|slot| slot.clone())
}

fn clear_overlay_target_app() {
    set_overlay_target_app(None);
}

#[cfg(target_os = "macos")]
const OVERLAY_TOP_OFFSET: f64 = 46.0;
#[cfg(any(target_os = "windows", target_os = "linux"))]
const OVERLAY_TOP_OFFSET: f64 = 4.0;

#[cfg(target_os = "macos")]
const OVERLAY_BOTTOM_OFFSET: f64 = 15.0;

#[cfg(any(target_os = "windows", target_os = "linux"))]
const OVERLAY_BOTTOM_OFFSET: f64 = 40.0;

/// Forces a window to be topmost using Win32 API (Windows only)
/// This is more reliable than Tauri's set_always_on_top which can be overridden
#[cfg(target_os = "windows")]
fn force_overlay_topmost(overlay_window: &tauri::webview::WebviewWindow) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    };

    // Clone because run_on_main_thread takes 'static
    let overlay_clone = overlay_window.clone();

    // Make sure the Win32 call happens on the UI thread
    let _ = overlay_clone.clone().run_on_main_thread(move || {
        if let Ok(hwnd) = overlay_clone.hwnd() {
            unsafe {
                // Force Z-order: make this window topmost without changing size/pos or stealing focus
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
        }
    });
}

fn get_monitor_with_cursor(app_handle: &AppHandle) -> Option<(tauri::Monitor, (f64, f64))> {
    let cursor = input::get_cursor_position(app_handle)?;
    let cursor_x = cursor.0 as f64;
    let cursor_y = cursor.1 as f64;

    if let Ok(monitors) = app_handle.available_monitors() {
        for monitor in monitors {
            let scale = monitor.scale_factor();
            let min_x = monitor.position().x as f64 / scale;
            let min_y = monitor.position().y as f64 / scale;
            let max_x = min_x + monitor.size().width as f64 / scale;
            let max_y = min_y + monitor.size().height as f64 / scale;

            if cursor_x >= min_x && cursor_x < max_x && cursor_y >= min_y && cursor_y < max_y {
                return Some((monitor, (cursor_x, cursor_y)));
            }
        }
    }

    None
}

fn calculate_overlay_position(app_handle: &AppHandle) -> Option<(f64, f64)> {
    let settings = settings::get_settings(app_handle);

    if settings.overlay_position == OverlayPosition::FollowCursor {
        if let Some((monitor, cursor_pos)) = get_monitor_with_cursor(app_handle) {
            let scale = monitor.scale_factor();
            let work_area = monitor.work_area();
            let work_area_x = work_area.position.x as f64 / scale;
            let work_area_y = work_area.position.y as f64 / scale;
            let work_area_width = work_area.size.width as f64 / scale;
            let work_area_height = work_area.size.height as f64 / scale;

            let mut x = cursor_pos.0 - OVERLAY_WIDTH / 2.0;
            let mut y = cursor_pos.1 + CURSOR_VERTICAL_OFFSET;
            if y + OVERLAY_HEIGHT > work_area_y + work_area_height {
                y = cursor_pos.1 - OVERLAY_HEIGHT - CURSOR_VERTICAL_OFFSET;
            }

            x = x.clamp(work_area_x, work_area_x + work_area_width - OVERLAY_WIDTH);
            y = y.clamp(work_area_y, work_area_y + work_area_height - OVERLAY_HEIGHT);

            return Some((x, y));
        }
    } else if settings.overlay_position != OverlayPosition::None {
        if let Some((monitor, _)) = get_monitor_with_cursor(app_handle) {
            return Some(position_on_monitor(&monitor, settings.overlay_position));
        }
    }

    if let Ok(Some(monitor)) = app_handle.primary_monitor() {
        if settings.overlay_position == OverlayPosition::None {
            return None;
        }
        return Some(position_on_monitor(&monitor, settings.overlay_position));
    }

    None
}

fn position_on_monitor(monitor: &tauri::Monitor, overlay_position: OverlayPosition) -> (f64, f64) {
    let work_area = monitor.work_area();
    let scale = monitor.scale_factor();
    let work_area_width = work_area.size.width as f64 / scale;
    let work_area_height = work_area.size.height as f64 / scale;
    let work_area_x = work_area.position.x as f64 / scale;
    let work_area_y = work_area.position.y as f64 / scale;

    let x = work_area_x + (work_area_width - OVERLAY_WIDTH) / 2.0;
    let y = match overlay_position {
        OverlayPosition::Top => work_area_y + OVERLAY_TOP_OFFSET,
        OverlayPosition::Bottom => {
            work_area_y + work_area_height - OVERLAY_HEIGHT - OVERLAY_BOTTOM_OFFSET
        }
        OverlayPosition::FollowCursor => {
            work_area_y + work_area_height - OVERLAY_HEIGHT - OVERLAY_BOTTOM_OFFSET
        }
        OverlayPosition::None => {
            work_area_y + work_area_height - OVERLAY_HEIGHT - OVERLAY_BOTTOM_OFFSET
        }
    };

    (x, y)
}

// macOS doesn't call into focus restoration: the panel-based overlay never
// steals focus there (see comments in the show_* functions), so the helpers
// below would be dead code on that target.
#[cfg(not(target_os = "macos"))]
fn focused_votype_window_label(app_handle: &AppHandle) -> Option<&'static str> {
    for label in ["review_window", "main"] {
        if app_handle
            .get_webview_window(label)
            .and_then(|window| window.is_focused().ok())
            .unwrap_or(false)
        {
            return Some(label);
        }
    }

    None
}

#[cfg(not(target_os = "macos"))]
fn restore_votype_focus_after_overlay_show(app_handle: &AppHandle, label: Option<&'static str>) {
    let Some(label) = label else {
        return;
    };

    if let Some(window) = app_handle.get_webview_window(label) {
        let _ = window.set_focus();
        let _ = window.emit("votype-refocus-active-input", ());
    }

    let app_handle = app_handle.clone();
    std::thread::spawn(move || {
        for delay_ms in [40_u64, 120_u64] {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            if let Some(window) = app_handle.get_webview_window(label) {
                let _ = window.set_focus();
                let _ = window.emit("votype-refocus-active-input", ());
            }
        }
    });
}

pub fn set_overlay_interactive(app_handle: &AppHandle, interactive: bool) {
    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        let _ = overlay_window.set_ignore_cursor_events(!interactive);
    }
}

/// Creates the recording overlay window and keeps it hidden by default
#[cfg(not(target_os = "macos"))]
pub fn create_recording_overlay(app_handle: &AppHandle) {
    if let Some((x, y)) = calculate_overlay_position(app_handle) {
        match WebviewWindowBuilder::new(
            app_handle,
            "recording_overlay",
            tauri::WebviewUrl::App("src/overlay/index.html".into()),
        )
        .title("Recording")
        .position(x, y)
        .resizable(false)
        .inner_size(OVERLAY_WIDTH, OVERLAY_HEIGHT)
        .shadow(false)
        .maximizable(false)
        .minimizable(false)
        .closable(false)
        .accept_first_mouse(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .transparent(true)
        .focused(false)
        .visible(true)
        .build()
        {
            Ok(_window) => {
                set_overlay_interactive(app_handle, false);
                debug!("Recording overlay window created (OS-visible, CSS-hidden)");
            }
            Err(e) => {
                debug!("Failed to create recording overlay window: {}", e);
            }
        }
    }
}

/// Creates the recording overlay panel and keeps it hidden by default (macOS)
#[cfg(target_os = "macos")]
pub fn create_recording_overlay(app_handle: &AppHandle) {
    if let Some((x, y)) = calculate_overlay_position(app_handle) {
        // PanelBuilder creates a Tauri window then converts it to NSPanel.
        // The window remains registered, so get_webview_window() still works.
        match PanelBuilder::<_, RecordingOverlayPanel>::new(app_handle, "recording_overlay")
            .url(WebviewUrl::App("src/overlay/index.html".into()))
            .title("Recording")
            .position(tauri::Position::Logical(tauri::LogicalPosition { x, y }))
            .level(PanelLevel::Status)
            .size(tauri::Size::Logical(tauri::LogicalSize {
                width: OVERLAY_WIDTH,
                height: OVERLAY_HEIGHT,
            }))
            .has_shadow(false)
            .transparent(true)
            .no_activate(true)
            .corner_radius(0.0)
            .with_window(|w| w.decorations(false).transparent(true))
            .collection_behavior(
                CollectionBehavior::new()
                    .can_join_all_spaces()
                    .full_screen_auxiliary(),
            )
            .build()
        {
            Ok(panel) => {
                set_overlay_interactive(app_handle, false);
                // Leave the panel OS-visible from creation onward; the React
                // side controls actual visibility via CSS (.fade-in class).
                // This avoids macOS NSPanel show()/hide() scheduling latency
                // on the hot path.
                let _ = panel.show();
            }
            Err(e) => {
                log::error!("Failed to create recording overlay panel: {}", e);
            }
        }
    }
}

/// Shows the recording overlay window with fade-in animation
pub fn show_recording_overlay(app_handle: &AppHandle) {
    bump_overlay_generation();
    // Snapshot the target app the moment recording starts; keeps the icon
    // consistent for the whole session (recording → transcribing → llm →
    // translation → success) instead of only the final translation insert.
    let target = crate::active_window::fetch_active_window()
        .ok()
        .map(|info| info.app_name);
    set_overlay_target_app(target);
    // Check if overlay should be shown based on position setting
    let settings = settings::get_settings(app_handle);
    if settings.overlay_position == OverlayPosition::None {
        return;
    }

    // macOS: panel is OS-visible from app startup (see create_recording_overlay)
    // and React owns visibility via CSS opacity. Calling .show() and refocusing
    // here would re-assert panel ordering through `[NSWindow makeKeyAndOrderFront:]`,
    // which activates the whole Votype app and pulls every visible Votype window
    // (e.g. the settings window) above the user's foreground app.
    #[cfg(not(target_os = "macos"))]
    let refocus_target = focused_votype_window_label(app_handle);

    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        // Update position before showing to prevent flicker from position changes
        if let Some((x, y)) = calculate_overlay_position(app_handle) {
            let _ = overlay_window
                .set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
        }

        let _ = overlay_window.set_ignore_cursor_events(true);
        #[cfg(not(target_os = "macos"))]
        let _ = overlay_window.show();

        // On Windows, aggressively re-assert "topmost" in the native Z-order after showing
        #[cfg(target_os = "windows")]
        force_overlay_topmost(&overlay_window);

        emit_overlay_state_with_retry(overlay_window, "recording");
        #[cfg(not(target_os = "macos"))]
        restore_votype_focus_after_overlay_show(app_handle, refocus_target);
    }
}

/// Shows the recording overlay with rewrite count badge
pub fn show_recording_overlay_rewrite(app_handle: &AppHandle, rewrite_count: u32) {
    bump_overlay_generation();
    let target = crate::active_window::fetch_active_window()
        .ok()
        .map(|info| info.app_name);
    set_overlay_target_app(target);
    let settings = settings::get_settings(app_handle);
    if settings.overlay_position == OverlayPosition::None {
        return;
    }

    // macOS: see comment in show_recording_overlay above.
    #[cfg(not(target_os = "macos"))]
    let refocus_target = focused_votype_window_label(app_handle);

    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        if let Some((x, y)) = calculate_overlay_position(app_handle) {
            let _ = overlay_window
                .set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
        }

        let _ = overlay_window.set_ignore_cursor_events(true);
        #[cfg(not(target_os = "macos"))]
        let _ = overlay_window.show();

        #[cfg(target_os = "windows")]
        force_overlay_topmost(&overlay_window);

        emit_overlay_state_with_count_and_retry(overlay_window, "recording", rewrite_count);
        #[cfg(not(target_os = "macos"))]
        restore_votype_focus_after_overlay_show(app_handle, refocus_target);
    }
}

/// Shows the transcribing overlay window
pub fn show_transcribing_overlay(app_handle: &AppHandle) {
    // Check if overlay should be shown based on position setting
    let settings = settings::get_settings(app_handle);
    if settings.overlay_position == OverlayPosition::None {
        return;
    }

    // macOS: see comment in show_recording_overlay above.
    #[cfg(not(target_os = "macos"))]
    let refocus_target = focused_votype_window_label(app_handle);

    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        // Only update position if the overlay is not currently visible
        if !overlay_window.is_visible().unwrap_or(false) {
            update_overlay_position(app_handle);
        }

        let _ = overlay_window.set_ignore_cursor_events(true);
        #[cfg(not(target_os = "macos"))]
        let _ = overlay_window.show();

        // On Windows, aggressively re-assert "topmost" in the native Z-order after showing
        #[cfg(target_os = "windows")]
        force_overlay_topmost(&overlay_window);

        emit_overlay_state_with_retry(overlay_window, "transcribing");
        #[cfg(not(target_os = "macos"))]
        restore_votype_focus_after_overlay_show(app_handle, refocus_target);
    }
}

/// Shows the LLM processing overlay window
pub fn show_llm_processing_overlay(app_handle: &AppHandle) {
    let settings = settings::get_settings(app_handle);
    if settings.overlay_position == OverlayPosition::None {
        return;
    }

    // macOS: see comment in show_recording_overlay above.
    #[cfg(not(target_os = "macos"))]
    let refocus_target = focused_votype_window_label(app_handle);

    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        // Only update position if the overlay is not currently visible
        if !overlay_window.is_visible().unwrap_or(false) {
            update_overlay_position(app_handle);
        }

        let _ = overlay_window.set_ignore_cursor_events(true);
        #[cfg(not(target_os = "macos"))]
        let _ = overlay_window.show();
        emit_overlay_state_with_retry(overlay_window, "llm");
        #[cfg(not(target_os = "macos"))]
        restore_votype_focus_after_overlay_show(app_handle, refocus_target);
    }
}

/// Shows the translation overlay without restoring Votype window focus.
/// Used when the caller is about to hide the currently-focused Votype window
/// anyway (e.g. `confirm_reviewed_transcription` hiding the review window);
/// running the focus restore thread would otherwise race with the hide and
/// make the overlay appear to lag.
pub fn show_translation_overlay_fast(app_handle: &AppHandle, target_app_name: Option<String>) {
    bump_overlay_generation();
    // Refresh the session target (the insert path may be triggered outside
    // the original recording session, e.g. from a review window that was
    // opened minutes ago).
    if target_app_name.is_some() {
        set_overlay_target_app(target_app_name);
    }
    let settings = settings::get_settings(app_handle);
    if settings.overlay_position == OverlayPosition::None {
        return;
    }

    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        if !overlay_window.is_visible().unwrap_or(false) {
            update_overlay_position(app_handle);
        }
        let _ = overlay_window.set_ignore_cursor_events(true);
        // macOS: see comment in show_recording_overlay above.
        #[cfg(not(target_os = "macos"))]
        let _ = overlay_window.show();
        emit_overlay_state_with_retry(overlay_window, "translation");
    }
}

/// Shows the translation overlay window
pub fn show_translation_overlay(app_handle: &AppHandle) {
    bump_overlay_generation();
    let settings = settings::get_settings(app_handle);
    if settings.overlay_position == OverlayPosition::None {
        return;
    }

    // macOS: see comment in show_recording_overlay above.
    #[cfg(not(target_os = "macos"))]
    let refocus_target = focused_votype_window_label(app_handle);

    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        if !overlay_window.is_visible().unwrap_or(false) {
            update_overlay_position(app_handle);
        }

        let _ = overlay_window.set_ignore_cursor_events(true);
        #[cfg(not(target_os = "macos"))]
        let _ = overlay_window.show();
        emit_overlay_state_with_retry(overlay_window, "translation");
        #[cfg(not(target_os = "macos"))]
        restore_votype_focus_after_overlay_show(app_handle, refocus_target);
    }
}

/// Shows a brief success checkmark state on the overlay window. Returns the
/// overlay generation at the moment of the show so the caller can schedule a
/// cancellable delayed hide (see `confirm_reviewed_transcription`).
///
/// `variant = "translation_success"` labels the state as "翻译成功" instead of
/// the generic "已插入" used for non-translation inserts.
pub fn show_success_overlay(
    app_handle: &AppHandle,
    variant: &'static str,
    target_app_name: Option<String>,
) -> u64 {
    let gen = bump_overlay_generation();
    if target_app_name.is_some() {
        set_overlay_target_app(target_app_name);
    }
    let settings = settings::get_settings(app_handle);
    if settings.overlay_position == OverlayPosition::None {
        return gen;
    }

    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        if !overlay_window.is_visible().unwrap_or(false) {
            update_overlay_position(app_handle);
            // macOS: see comment in show_recording_overlay above.
            #[cfg(not(target_os = "macos"))]
            let _ = overlay_window.show();
        }
        let _ = overlay_window.set_ignore_cursor_events(true);
        emit_overlay_state_with_retry(overlay_window, variant);
    }
    gen
}

/// Shows the inserting overlay window
pub fn show_inserting_overlay(app_handle: &AppHandle) {
    bump_overlay_generation();
    let settings = settings::get_settings(app_handle);
    if settings.overlay_position == OverlayPosition::None {
        return;
    }

    // macOS: see comment in show_recording_overlay above.
    #[cfg(not(target_os = "macos"))]
    let refocus_target = focused_votype_window_label(app_handle);

    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        if !overlay_window.is_visible().unwrap_or(false) {
            update_overlay_position(app_handle);
        }

        let _ = overlay_window.set_ignore_cursor_events(true);
        #[cfg(not(target_os = "macos"))]
        let _ = overlay_window.show();
        emit_overlay_state_with_retry(overlay_window, "inserting");
        #[cfg(not(target_os = "macos"))]
        restore_votype_focus_after_overlay_show(app_handle, refocus_target);
    }
}

/// Monotonic sequence number for each "show-overlay" emit. Used by the retry
/// thread so that a later state change (e.g. translation → success) cancels
/// the still-pending 40 ms / 120 ms retries of the earlier state instead of
/// letting them overwrite the current one.
static OVERLAY_EMIT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn emit_overlay_state_with_retry(overlay_window: tauri::WebviewWindow, state: &'static str) {
    // Always carry the current session's target app so the overlay can
    // render its icon consistently across recording → transcribing → llm →
    // translation → success. Callers (re-)set this via
    // `set_overlay_target_app` at session start / at translation insert.
    let app_name = current_overlay_target_app();
    let seq = OVERLAY_EMIT_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let payload = OverlayStateWithApp {
        state,
        app_name: app_name.clone(),
    };
    let _ = overlay_window.emit("show-overlay", payload.clone());

    std::thread::spawn(move || {
        for delay_ms in [40_u64, 120_u64] {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            if OVERLAY_EMIT_SEQ.load(std::sync::atomic::Ordering::SeqCst) != seq {
                return; // a newer emit superseded us
            }
            let _ = overlay_window.emit("show-overlay", payload.clone());
        }
    });
}

#[derive(Clone, serde::Serialize)]
struct OverlayStateWithCount {
    state: &'static str,
    rewrite_count: u32,
}

/// Rich payload: state + optional target app name. The overlay uses this to
/// render a small icon of the paste-target app alongside the status label.
#[derive(Clone, serde::Serialize)]
struct OverlayStateWithApp {
    state: &'static str,
    app_name: Option<String>,
}

fn emit_overlay_state_with_count_and_retry(
    overlay_window: tauri::WebviewWindow,
    state: &'static str,
    rewrite_count: u32,
) {
    let seq = OVERLAY_EMIT_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let payload = OverlayStateWithCount {
        state,
        rewrite_count,
    };
    let _ = overlay_window.emit("show-overlay", payload.clone());

    std::thread::spawn(move || {
        for delay_ms in [40_u64, 120_u64] {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            if OVERLAY_EMIT_SEQ.load(std::sync::atomic::Ordering::SeqCst) != seq {
                return;
            }
            let _ = overlay_window.emit("show-overlay", payload.clone());
        }
    });
}

/// Updates the overlay window position based on current settings
pub fn update_overlay_position(app_handle: &AppHandle) {
    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        if let Some((x, y)) = calculate_overlay_position(app_handle) {
            let _ = overlay_window
                .set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
        }
    }
}

/// Fades the overlay out. The Tauri window itself stays OS-visible so the
/// next show is instantaneous (CSS transition only, ~80 ms) instead of
/// paying Cocoa's window show/hide scheduling cost each time.
pub fn hide_recording_overlay(app_handle: &AppHandle) {
    let gen = bump_overlay_generation();
    // Bump the emit seq too so any still-pending show-overlay retries (40 ms
    // / 120 ms after the previous state emit) abort instead of re-showing
    // the overlay right after we asked it to hide.
    OVERLAY_EMIT_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    log::debug!("[overlay-trace] hide_recording_overlay gen={}", gen);
    clear_overlay_target_app();
    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        let _ = overlay_window.set_ignore_cursor_events(true);
        // React side toggles its `.fade-in` class off and runs a CSS opacity
        // transition to 0. No window.hide() — the window remains an invisible
        // transparent overlay ready for the next emit.
        let _ = overlay_window.emit("hide-overlay", ());
    }
}

/// Cached "overlay is enabled" flag (`overlay_position != None`), kept in sync
/// with settings. Read on every audio level callback (~24-30 Hz), so the audio
/// hot path never touches the settings store. Defaults to false so nothing is
/// emitted until `setup` / `write_settings` populates it from real settings.
///
/// This is the core of the WebKit memory-leak fix (upstream Handy #1279/#1447):
/// the `recording_overlay` webview is created at boot regardless of
/// `overlay_position`, and every `mic-level` event its WebKit subprocess
/// processes drives an unbounded C++ allocation. Skipping emission when the
/// overlay is disabled removes the leak driver for `overlay_position: none`
/// users; targeting only the overlay window (below) halves the per-callback
/// WebKit dispatch for everyone else.
static OVERLAY_ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Refresh the cached overlay-enabled flag from settings. Called once at
/// startup and from `write_settings` on every persisted settings change, so
/// every path that can alter `overlay_position` keeps the cache in sync.
pub fn refresh_overlay_enabled_cache(settings: &settings::AppSettings) {
    OVERLAY_ENABLED.store(
        settings.overlay_position != OverlayPosition::None,
        std::sync::atomic::Ordering::Relaxed,
    );
}

pub fn emit_levels(app_handle: &AppHandle, levels: &[f32]) {
    // Skip entirely when the overlay is disabled: no window listens for
    // `mic-level`, yet a hidden overlay's WebKit subprocess would still leak
    // on each event it processes.
    if !OVERLAY_ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    use std::sync::atomic::{AtomicU64, Ordering};
    static EMIT_COUNT: AtomicU64 = AtomicU64::new(0);
    let count = EMIT_COUNT.fetch_add(1, Ordering::Relaxed);
    // Log every ~2 seconds (assuming ~30 emits/sec)
    if count.is_multiple_of(60) {
        let max = levels.iter().cloned().fold(0.0f32, f32::max);
        log::debug!("[waveform-emit] #{} max_level={:.3}", count, max);
    }

    // Target only the overlay window. In Tauri 2 both `AppHandle::emit` and
    // `WebviewWindow::emit` broadcast to every webview; `mic-level` is consumed
    // solely by the overlay (RecordingOverlay.tsx), so one targeted emit is
    // correct and produces a single eval_script per callback instead of the old
    // broadcast + per-window pair.
    let _ = app_handle.emit_to("recording_overlay", "mic-level", levels);
}

/// Makes the overlay window key window so it can receive keyboard input
/// Used when showing skill confirmation dialog
/// On macOS, we need to activate the app first, then make the panel key window
#[cfg(target_os = "macos")]
pub fn focus_recording_overlay(app_handle: &AppHandle) {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, msg_send_id};
    use tauri_nspanel::ManagerExt;

    let app_handle_inner = app_handle.clone();
    let _ = app_handle.run_on_main_thread(move || {
        set_overlay_interactive(&app_handle_inner, true);
        // First, activate the current app to force macOS to give us focus
        unsafe {
            // Get current process ID
            let pid = std::process::id() as i32;

            // Get NSRunningApplication class
            let cls = class!(NSRunningApplication);

            // Call +[NSRunningApplication runningApplicationWithProcessIdentifier:]
            let app: Option<Retained<AnyObject>> =
                msg_send_id![cls, runningApplicationWithProcessIdentifier: pid];

            if let Some(app) = app {
                // NSApplicationActivateIgnoringOtherApps = 1 << 1 = 2
                let options: usize = 2;
                let _: bool = msg_send![&*app, activateWithOptions: options];
            }
        }

        // Small delay to ensure activation completes (using sleep on main thread is slightly risky
        // but 20ms is short enough for this specific UX requirement)
        std::thread::sleep(std::time::Duration::from_millis(20));

        // Now make the panel key window
        if let Ok(panel) = app_handle_inner.get_webview_panel("recording_overlay") {
            panel.make_key_window();
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub fn focus_recording_overlay(app_handle: &AppHandle) {
    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        let _ = overlay_window.set_ignore_cursor_events(false);
        let _ = overlay_window.set_focus();
    }
}
