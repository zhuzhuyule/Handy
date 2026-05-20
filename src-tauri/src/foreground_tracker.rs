use crate::active_window::{fetch_active_window, ActiveWindowInfo};
use once_cell::sync::Lazy;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tauri::AppHandle;

/// 后台轮询保存的"最近一次非 Votype 自身的 frontmost app"。
/// `quick_insert_to_target` 命令以此为目标做 focus + paste。
static LAST_EXTERNAL_FRONTMOST: Lazy<StdMutex<Option<ActiveWindowInfo>>> =
    Lazy::new(|| StdMutex::new(None));

const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// 决定一次轮询的 fetch 结果是否应该写入 slot：
/// - Ok(info) 且 pid != self → 返回 Some(info)（覆盖 slot）
/// - Ok(info) 且 pid == self → None（保留旧 slot）
/// - Err → None（保留旧 slot）
///
/// 纯函数，供轮询循环与单测共用。
pub(crate) fn next_slot_value(
    fetched: Result<ActiveWindowInfo, String>,
    self_pid: u64,
) -> Option<ActiveWindowInfo> {
    match fetched {
        Ok(info) if info.process_id != self_pid => Some(info),
        _ => None,
    }
}

pub(crate) fn set_last_external_frontmost(info: ActiveWindowInfo) {
    let mut slot = LAST_EXTERNAL_FRONTMOST
        .lock()
        .expect("LAST_EXTERNAL_FRONTMOST poisoned");
    *slot = Some(info);
}

/// 命令侧使用：取一份 slot 的克隆，None 表示还没追踪到外部 app（应用刚启动或 Wayland 抓不到）。
#[allow(dead_code)] // removed in Task 3 when quick_insert_to_target command consumes the slot
pub fn get_last_external_frontmost() -> Option<ActiveWindowInfo> {
    let slot = LAST_EXTERNAL_FRONTMOST
        .lock()
        .expect("LAST_EXTERNAL_FRONTMOST poisoned");
    slot.clone()
}

/// 启动后台轮询。在 `lib.rs` 的 setup 中调用一次即可——任务永久存活到进程退出。
/// 取自身 pid 后传入闭包，避免每次循环都 syscall。
pub fn start(_app_handle: &AppHandle) {
    let self_pid = std::process::id() as u64;
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            if let Some(info) = next_slot_value(fetch_active_window(), self_pid) {
                set_last_external_frontmost(info);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::active_window::WindowPosition;

    fn snap(pid: u64, app: &str) -> ActiveWindowInfo {
        ActiveWindowInfo {
            title: format!("{app}-title"),
            app_name: app.to_string(),
            window_id: format!("w-{pid}"),
            process_id: pid,
            process_path: format!("/Applications/{app}.app"),
            position: WindowPosition {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
        }
    }

    // Slot tests touch a shared global; serialize them so cargo's parallel
    // runner doesn't make them flake.
    static SLOT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn next_slot_value_returns_info_for_external_pid() {
        let result = next_slot_value(Ok(snap(99, "VSCode")), 12345);
        assert!(
            matches!(result, Some(ref info) if info.process_id == 99 && info.app_name == "VSCode"),
            "expected Some(info) with VSCode, got {:?}",
            result
        );
    }

    #[test]
    fn next_slot_value_skips_self_pid() {
        let result = next_slot_value(Ok(snap(12345, "Votype")), 12345);
        assert!(result.is_none(), "self pid must not write to slot");
    }

    #[test]
    fn next_slot_value_skips_fetch_error() {
        let result = next_slot_value(Err("permission denied".to_string()), 12345);
        assert!(result.is_none(), "fetch error must leave slot untouched");
    }

    #[test]
    fn set_then_get_round_trip() {
        let _guard = SLOT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_last_external_frontmost(snap(77, "Chrome"));
        let got = get_last_external_frontmost().expect("slot should hold value");
        assert_eq!(got.process_id, 77);
        assert_eq!(got.app_name, "Chrome");
    }

    #[test]
    fn later_set_overwrites_earlier_value() {
        let _guard = SLOT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_last_external_frontmost(snap(77, "Chrome"));
        set_last_external_frontmost(snap(88, "Slack"));
        let got = get_last_external_frontmost().expect("slot should hold value");
        assert_eq!(got.process_id, 88);
        assert_eq!(got.app_name, "Slack");
    }
}
