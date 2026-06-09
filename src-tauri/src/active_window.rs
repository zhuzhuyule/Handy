use active_win_pos_rs::{get_active_window, WindowPosition as RawWindowPosition};
use enigo::{Enigo, Mouse, Settings};
use serde::Serialize;

/// Lightweight serializable representation of the active window's bounds.
#[derive(Debug, Serialize, Clone)]
pub struct WindowPosition {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<RawWindowPosition> for WindowPosition {
    fn from(value: RawWindowPosition) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

/// Snapshot of the currently focused application's main window.
#[derive(Debug, Serialize, Clone)]
pub struct ActiveWindowInfo {
    pub title: String,
    pub app_name: String,
    pub window_id: String,
    pub process_id: u64,
    pub process_path: String,
    pub position: WindowPosition,
}

#[derive(Debug, Serialize)]
pub struct CursorPosition {
    pub x: i32,
    pub y: i32,
}

pub fn fetch_active_window() -> Result<ActiveWindowInfo, String> {
    let active_window =
        get_active_window().map_err(|_| "无法获取当前活动窗口，可能缺少辅助权限".to_string())?;

    let mut used_accessibility = false;
    let title = if active_window.title.is_empty() {
        #[cfg(target_os = "macos")]
        {
            match get_window_title_via_accessibility(active_window.process_id) {
                Ok(t) => {
                    used_accessibility = true;
                    t
                }
                Err(_) => active_window.title.clone(),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            active_window.title.clone()
        }
    } else {
        active_window.title.clone()
    };

    log::debug!(
        "[ActiveWindow] {} | app: {:?} | title: {:?} (fallback: {})",
        if used_accessibility { "FIXED" } else { "RAW" },
        active_window.app_name,
        title,
        used_accessibility
    );

    Ok(ActiveWindowInfo {
        title,
        app_name: active_window.app_name,
        window_id: active_window.window_id,
        process_id: active_window.process_id,
        process_path: active_window.process_path.to_string_lossy().to_string(),
        position: active_window.position.into(),
    })
}

/// Get window title using macOS Accessibility API (AXUIElement)
/// This is more reliable than active_win_pos_rs for some applications
#[cfg(target_os = "macos")]
fn get_window_title_via_accessibility(pid: u64) -> Result<String, String> {
    use core_foundation::base::{CFRelease, TCFType};
    use core_foundation::string::{CFString, CFStringRef};
    use std::ffi::c_void;
    use std::ptr;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
        fn AXUIElementCopyAttributeValue(
            element: *mut c_void,
            attribute: CFStringRef,
            value: *mut *mut c_void,
        ) -> i32;
    }

    unsafe {
        // Create AXUIElement for the application
        let app_element = AXUIElementCreateApplication(pid as i32);
        if app_element.is_null() {
            return Err("Failed to create AXUIElement for app".to_string());
        }

        // Get focused window
        let focused_window_attr = CFString::new("AXFocusedWindow");
        let mut focused_window: *mut c_void = ptr::null_mut();
        let result = AXUIElementCopyAttributeValue(
            app_element,
            focused_window_attr.as_concrete_TypeRef(),
            &mut focused_window,
        );

        if result != 0 || focused_window.is_null() {
            CFRelease(app_element);
            return Err("Failed to get focused window".to_string());
        }

        // Get window title
        let title_attr = CFString::new("AXTitle");
        let mut title_value: *mut c_void = ptr::null_mut();
        let result = AXUIElementCopyAttributeValue(
            focused_window,
            title_attr.as_concrete_TypeRef(),
            &mut title_value,
        );

        CFRelease(focused_window);
        CFRelease(app_element);

        if result != 0 || title_value.is_null() {
            return Err("Failed to get window title".to_string());
        }

        // Convert CFString to Rust String
        let cf_title = CFString::wrap_under_create_rule(title_value as CFStringRef);
        let title = cf_title.to_string();

        Ok(title)
    }
}

/// macOS-only safe variant for the 500ms foreground poller. Calls NSWorkspace
/// directly via objc2 with explicit nil checks so it never aborts on a transient
/// nil from `frontmostApplication` (which happens during focus transitions —
/// the auto-generated `appkit-nsworkspace-bindings` deref the raw pointer and
/// trigger a non-unwinding panic that catch_unwind cannot recover from).
///
/// Returned info lacks the window-position / title / window_id fields because
/// NSWorkspace alone cannot supply them; the poller's downstream consumers
/// (quick_insert_to_target, paste_text_to_active_window) only need pid + app_name.
#[cfg(target_os = "macos")]
pub fn safe_fetch_frontmost_app_macos() -> Result<ActiveWindowInfo, String> {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, msg_send_id};
    use std::ffi::CStr;
    use std::os::raw::c_char;

    unsafe {
        let workspace_cls = class!(NSWorkspace);
        let workspace: Retained<AnyObject> = msg_send_id![workspace_cls, sharedWorkspace];

        let frontmost: Option<Retained<AnyObject>> =
            msg_send_id![&*workspace, frontmostApplication];
        let app = frontmost.ok_or_else(|| "frontmostApplication returned nil".to_string())?;

        let pid_i32: i32 = msg_send![&*app, processIdentifier];
        if pid_i32 <= 0 {
            return Err(format!("invalid pid {} from frontmostApplication", pid_i32));
        }
        let pid = pid_i32 as u64;

        // localizedName / bundleIdentifier can each return nil; treat as Unknown.
        let read_ns_string = |obj: Option<Retained<AnyObject>>| -> Option<String> {
            let ns = obj?;
            let utf8: *const c_char = msg_send![&*ns, UTF8String];
            if utf8.is_null() {
                return None;
            }
            Some(CStr::from_ptr(utf8).to_string_lossy().into_owned())
        };

        let name_obj: Option<Retained<AnyObject>> = msg_send_id![&*app, localizedName];
        let app_name = read_ns_string(name_obj).unwrap_or_else(|| "Unknown".to_string());

        let bundle_obj: Option<Retained<AnyObject>> = msg_send_id![&*app, bundleIdentifier];
        let bundle_id = read_ns_string(bundle_obj).unwrap_or_default();

        Ok(ActiveWindowInfo {
            title: String::new(),
            app_name,
            window_id: bundle_id,
            process_id: pid,
            process_path: String::new(),
            position: WindowPosition {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            },
        })
    }
}

pub fn fetch_cursor_position() -> Result<CursorPosition, String> {
    let enigo =
        Enigo::new(&Settings::default()).map_err(|e| format!("创建 Enigo 实例失败: {}", e))?;
    let location = enigo
        .location()
        .map_err(|e| format!("无法获取鼠标位置: {}", e))?;
    Ok(CursorPosition {
        x: location.0,
        y: location.1,
    })
}

#[cfg(target_os = "macos")]
pub fn focus_app_by_pid(pid: u64) -> Result<(), String> {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, msg_send_id};

    let pid_i32 = pid as i32;

    // NSRunningApplication is documented as thread-safe for many operations.
    // For now, we perform the activation here. If crashes persist during focus restoration,
    // this should also be wrapped in run_on_main_thread by callers with AppHandle.

    unsafe {
        let cls = class!(NSRunningApplication);
        let app: Option<Retained<AnyObject>> =
            msg_send_id![cls, runningApplicationWithProcessIdentifier: pid_i32];

        match app {
            Some(app) => {
                let options: usize = 2; // NSApplicationActivateIgnoringOtherApps
                let success: bool = msg_send![&*app, activateWithOptions: options];
                if success {
                    Ok(())
                } else {
                    Err("Failed to activate app".to_string())
                }
            }
            None => Err(format!("No running application found for pid {}", pid)),
        }
    }
}

#[cfg(target_os = "windows")]
pub fn focus_app_by_pid(pid: u64) -> Result<(), String> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow, ShowWindow,
        SW_RESTORE,
    };

    unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let target = &mut *(lparam.0 as *mut WindowSearch);
        if !target.hwnd.is_invalid() {
            return BOOL(0);
        }

        let mut window_pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut window_pid));
        if window_pid as u64 == target.pid && IsWindowVisible(hwnd).as_bool() {
            target.hwnd = hwnd;
            return BOOL(0);
        }

        BOOL(1)
    }

    #[derive(Default)]
    struct WindowSearch {
        pid: u64,
        hwnd: HWND,
    }

    let mut search = WindowSearch {
        pid,
        hwnd: HWND::default(),
    };

    unsafe {
        let lparam = LPARAM(&mut search as *mut WindowSearch as isize);
        EnumWindows(Some(enum_windows_proc), lparam);
    }

    if search.hwnd.is_invalid() {
        return Err("No window found for target pid".to_string());
    }

    unsafe {
        ShowWindow(search.hwnd, SW_RESTORE);
        if SetForegroundWindow(search.hwnd).as_bool() {
            Ok(())
        } else {
            Err("Failed to bring window to foreground".to_string())
        }
    }
}

#[cfg(target_os = "linux")]
pub fn focus_app_by_pid(pid: u64) -> Result<(), String> {
    use crate::utils::is_wayland;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        AtomEnum, ClientMessageData, ClientMessageEvent, ConnectionExt, EventMask, PropMode, Window,
    };

    if is_wayland() {
        return focus_previous_app_wayland();
    }
    if std::env::var("DISPLAY").is_err() {
        return Err("DISPLAY is not set; likely running under Wayland".to_string());
    }

    let (conn, screen_num) = x11rb::connect(None).map_err(|e| e.to_string())?;
    let root = conn.setup().roots[screen_num].root;

    let atom_pid = conn
        .intern_atom(false, b"_NET_WM_PID")
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?
        .atom;
    let atom_active = conn
        .intern_atom(false, b"_NET_ACTIVE_WINDOW")
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?
        .atom;

    let tree = conn
        .query_tree(root)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;

    let mut target_window: Option<Window> = None;
    for win in tree.children {
        let prop = conn
            .get_property(false, win, atom_pid, AtomEnum::CARDINAL, 0, 1)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        if let Some(value) = prop.value32().and_then(|mut v| v.next()) {
            if value as u64 == pid {
                target_window = Some(win);
                break;
            }
        }
    }

    let target = target_window.ok_or_else(|| "No X11 window found for pid".to_string())?;

    let event = ClientMessageEvent {
        response_type: 33,
        format: 32,
        sequence: 0,
        window: target,
        type_: atom_active,
        data: ClientMessageData::from([1, 0, 0, 0, 0]),
    };

    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    )
    .map_err(|e| e.to_string())?;
    conn.flush().map_err(|e| e.to_string())?;

    Ok(())
}

#[cfg(target_os = "linux")]
fn focus_previous_app_wayland() -> Result<(), String> {
    use std::process::Command;

    if is_wtype_available() {
        let output = Command::new("wtype")
            .args(["-M", "alt", "-k", "Tab", "-m", "alt"])
            .output()
            .map_err(|e| format!("Failed to execute wtype: {}", e))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("wtype failed: {}", stderr));
    }

    if is_dotool_available() {
        let output = Command::new("sh")
            .arg("-c")
            .arg("echo key alt+tab | dotool")
            .output()
            .map_err(|e| format!("Failed to execute dotool: {}", e))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("dotool failed: {}", stderr));
    }

    Err("Wayland focus requires wtype or dotool".to_string())
}

#[cfg(target_os = "linux")]
fn is_wtype_available() -> bool {
    std::process::Command::new("which")
        .arg("wtype")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn is_dotool_available() -> bool {
    std::process::Command::new("which")
        .arg("dotool")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(all(
    not(target_os = "macos"),
    not(target_os = "windows"),
    not(target_os = "linux")
))]
pub fn focus_app_by_pid(_pid: u64) -> Result<(), String> {
    Err("Focus by pid is not supported on this platform".to_string())
}
