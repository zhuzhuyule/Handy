# 历史详情页快捷插入到上一个外部应用 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在历史详情卡片的编辑按钮左侧加一个 Insert 按钮，让用户能把该条历史的文本一键插入到上一次切换到 Votype 之前所在的外部 app，并自动隐藏 settings 窗口。

**Architecture:** 新增后端 `foreground_tracker` 模块（500ms 轮询 `fetch_active_window`，按 process_id 过滤掉 Votype 自身），暴露 `get_quick_insert_target` 和 `quick_insert_to_target` 两个 Tauri command（复用现有的 `focus_app_by_pid` + `paste` 链路）；前端用 1Hz hook 读取 slot，DashboardEntryCard 在两处编辑按钮位置左侧加 Insert IconButton。

**Tech Stack:** Rust + Tauri 2.x；`once_cell::sync::Lazy + std::sync::Mutex`、`tauri::async_runtime::spawn + tokio::time::sleep`；React + Radix UI + Tabler Icons + sonner + i18next（项目已有）。

**Spec:** `docs/specs/2026-05-20-quick-insert-from-history-detail.spec.md`

---

## File Structure

| 文件                                                       | 操作 | 责任                                                                                |
| ---------------------------------------------------------- | ---- | ----------------------------------------------------------------------------------- |
| `src-tauri/src/foreground_tracker.rs`                      | 新建 | 模块级 slot + 轮询 task + 两个公开 helper（get/start）+ 单元测试                    |
| `src-tauri/src/lib.rs`                                     | 修改 | `mod foreground_tracker;`；setup 中启动 tracker；invoke_handler 注册 2 个新 command |
| `src-tauri/src/commands/mod.rs`                            | 修改 | `QuickInsertTarget` struct + `get_quick_insert_target` + `quick_insert_to_target`   |
| `src/hooks/useQuickInsertTarget.ts`                        | 新建 | 1Hz polling hook，挂载时启动、卸载时清理                                            |
| `src/components/settings/dashboard/DashboardEntryCard.tsx` | 修改 | 导入 hook + IconSend + toast；两处编辑按钮位置左侧加 Insert IconButton              |
| `src/i18n/locales/en/translation.json`                     | 修改 | 加 5 个新 key（dashboard.actions 下）                                               |
| `src/i18n/locales/zh/translation.json`                     | 修改 | 同上中文翻译                                                                        |

不动其它 13 个 locale 文件——i18next 默认 fallback 到 en。

---

### Task 1: 新建 `foreground_tracker` 模块（TDD）

**Files:**

- Create: `src-tauri/src/foreground_tracker.rs`

**说明：** 把"是否更新 slot"做成纯函数 `next_slot_value`，方便单测；轮询循环只是薄包装。

- [ ] **Step 1.1: 先写 5 个失败测试**

创建 `src-tauri/src/foreground_tracker.rs`，内容如下（只有测试，主体留空——TDD red 阶段）：

```rust
use crate::active_window::ActiveWindowInfo;
use once_cell::sync::Lazy;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tauri::AppHandle;

// Placeholder so the test imports compile; bodies land in Step 1.3.
pub(crate) fn next_slot_value(
    _fetched: Result<ActiveWindowInfo, String>,
    _self_pid: u64,
) -> Option<ActiveWindowInfo> {
    unimplemented!()
}

pub(crate) fn set_last_external_frontmost(_info: ActiveWindowInfo) {
    unimplemented!()
}

pub fn get_last_external_frontmost() -> Option<ActiveWindowInfo> {
    unimplemented!()
}

#[allow(dead_code)] // removed in Step 1.3 when start() is wired up
const POLL_INTERVAL: Duration = Duration::from_millis(500);

#[allow(dead_code)] // removed in Task 2 when lib.rs setup calls start()
pub fn start(_app_handle: &AppHandle) {
    unimplemented!()
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
        let _guard = SLOT_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set_last_external_frontmost(snap(77, "Chrome"));
        let got = get_last_external_frontmost().expect("slot should hold value");
        assert_eq!(got.process_id, 77);
        assert_eq!(got.app_name, "Chrome");
    }

    #[test]
    fn later_set_overwrites_earlier_value() {
        let _guard = SLOT_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        set_last_external_frontmost(snap(77, "Chrome"));
        set_last_external_frontmost(snap(88, "Slack"));
        let got = get_last_external_frontmost().expect("slot should hold value");
        assert_eq!(got.process_id, 88);
        assert_eq!(got.app_name, "Slack");
    }
}
```

- [ ] **Step 1.2: 在 lib.rs 中暂时声明该模块，运行测试确认失败**

打开 `src-tauri/src/lib.rs`，在模块声明区（约 line 1-30 紧挨其他 `mod xxx;` 声明的地方）添加：

```rust
mod foreground_tracker;
```

然后运行：

```bash
cd src-tauri && cargo test --lib foreground_tracker 2>&1 | tail -20
```

Expected: 5 个测试全部 PANIC（`unimplemented!()`），编译通过。

- [ ] **Step 1.3: 实现主体，替换 placeholder**

把 Step 1.1 中所有 `unimplemented!()` 占位、`#[allow(dead_code)]` 与对应函数主体替换为：

```rust
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
```

> 删除上面 4 处 `#[allow(dead_code)]` 注释。`start` 仍然没人调用，会触发 `dead_code` 警告——Task 2 即将接入，**在 Task 2 提交之前不要单独提交本步**，避免引入瞬间警告。

- [ ] **Step 1.4: 验证 5 个测试通过**

```bash
cd src-tauri && cargo test --lib foreground_tracker 2>&1 | tail -10
```

Expected: 5 passed, 0 failed.

- [ ] **Step 1.5: 编译检查（暂留一个 dead_code 警告，下个 Task 接入）**

```bash
cd src-tauri && cargo check --lib 2>&1 | tail -10
```

Expected: 编译通过；可能出现 `function 'start' is never used` 的 warning——预期，Task 2 会消除。**本 Task 暂不提交**，与 Task 2 合并为一个 commit。

---

### Task 2: 在 `lib.rs` setup 中启动 tracker

**Files:**

- Modify: `src-tauri/src/lib.rs`（Step 1.2 已加的 `mod` 行 + setup 函数中的启动调用）

- [ ] **Step 2.1: 在 setup 中启动 tracker**

定位 `src-tauri/src/lib.rs:400-406`（`utils::create_recording_overlay(app_handle);` 所在的 setup 函数尾部）：

```rust
    // Create the recording overlay window (hidden by default)
    utils::create_recording_overlay(app_handle);

    openai_api_server::start_openai_api_server(app_handle);

    // Review window is created lazily on first use (saves ~80MB idle memory)
}
```

在 `utils::create_recording_overlay(app_handle);` 之后、`openai_api_server::start_openai_api_server(app_handle);` 之前插入：

```rust
    // 后台 500ms 轮询，保存最近一次"非 Votype 自身"的 frontmost app。
    // 详情卡片的 Insert 按钮以此为目标。
    // 见 docs/specs/2026-05-20-quick-insert-from-history-detail.spec.md
    foreground_tracker::start(app_handle);
```

- [ ] **Step 2.2: 编译 + 测试**

```bash
cd src-tauri && cargo check --lib 2>&1 | tail -10
cd src-tauri && cargo test --lib foreground_tracker 2>&1 | tail -10
```

Expected: 编译 0 errors 0 warnings；5 tests pass。

- [ ] **Step 2.3: 合并提交 Task 1 + 2**

```bash
cd /Users/zac/code/github/asr/Handy
git add src-tauri/src/foreground_tracker.rs src-tauri/src/lib.rs
git commit -m "Add foreground tracker module polling last external frontmost"
```

---

### Task 3: 后端 Tauri command — `get_quick_insert_target` + `quick_insert_to_target`

**Files:**

- Modify: `src-tauri/src/commands/mod.rs`（新增 struct + 2 个 command）
- Modify: `src-tauri/src/lib.rs`（invoke_handler 注册）

- [ ] **Step 3.1: 在 commands/mod.rs 顶部检查 use 块**

打开 `src-tauri/src/commands/mod.rs` 查看现有 use 块。确认包含 `use tauri::{AppHandle, Manager};`——`Manager` trait 提供 `get_webview_window`。如缺，添加。（既有 `paste_to_previous_window` 应该已经 import 了 AppHandle，但可能没 import Manager。）

- [ ] **Step 3.2: 在 `paste_to_previous_window` 之后追加新结构体 + 两个 command**

定位 `src-tauri/src/commands/mod.rs:198`（`paste_to_previous_window` 函数结束的 `}` 之后），追加：

```rust
/// 暴露给前端的最小 target 描述。
/// 前端用 `app_name` 渲染 tooltip；`pid` 仅用于日志/调试，命令端读 backend slot 自己解析。
#[derive(serde::Serialize)]
pub struct QuickInsertTarget {
    pub app_name: String,
    pub pid: u64,
}

/// 返回 backend 当前缓存的"最近一次非 Votype frontmost"。前端 Dashboard 详情按钮以此决定 enabled/disabled。
#[tauri::command]
pub fn get_quick_insert_target() -> Option<QuickInsertTarget> {
    crate::foreground_tracker::get_last_external_frontmost().map(|info| QuickInsertTarget {
        app_name: info.app_name,
        pid: info.process_id,
    })
}

/// 把 `text` 插入到 backend 缓存的目标 app。
///
/// 失败时 Err 字符串采用三类前缀，前端按前缀映射 toast：
/// - `no_target` — slot 空（理论上前端 disabled 已阻止，仅作 defensive）
/// - `focus_failed: <details>` — focus_app_by_pid 失败，目标 app 可能已关闭
/// - `paste_failed: <details>` — paste 失败，常因辅助权限缺失
///
/// 成功后隐藏 main 窗口；失败时 main 保持打开，前端 toast 显示错误。
#[tauri::command]
pub fn quick_insert_to_target(app: AppHandle, text: String) -> Result<(), String> {
    use std::time::Duration;

    let target = crate::foreground_tracker::get_last_external_frontmost()
        .ok_or_else(|| "no_target".to_string())?;

    crate::active_window::focus_app_by_pid(target.process_id)
        .map_err(|e| format!("focus_failed: {e}"))?;

    std::thread::sleep(Duration::from_millis(120));

    crate::clipboard::paste(text, app.clone()).map_err(|e| format!("paste_failed: {e}"))?;

    if let Some(main) = app.get_webview_window("main") {
        let _ = main.hide();
    }

    Ok(())
}
```

- [ ] **Step 3.3: 在 lib.rs 的 invoke_handler 中注册两条**

定位 `src-tauri/src/lib.rs:767`（`.invoke_handler(tauri::generate_handler![` 起始）。找到现有的 `commands::paste_to_previous_window` 一行（搜 `paste_to_previous_window` 定位）；在它之后追加两行：

```rust
            commands::paste_to_previous_window,
            commands::get_quick_insert_target,
            commands::quick_insert_to_target,
```

> 注意：缩进与上下文一致（4 个空格 + 后续是 tab/缩进，按文件实际为准）。如果 `commands::paste_to_previous_window` 行不在 invoke_handler 列表里（旧版本可能用别的命名空间），改在最末尾任意位置追加。

- [ ] **Step 3.4: 编译 + 测试**

```bash
cd src-tauri && cargo check --lib 2>&1 | tail -10
cd src-tauri && cargo test --lib foreground_tracker 2>&1 | tail -10
```

Expected: 0 errors, 0 warnings；tracker 5 tests 仍 pass。

- [ ] **Step 3.5: 提交**

```bash
cd /Users/zac/code/github/asr/Handy
git add src-tauri/src/commands/mod.rs src-tauri/src/lib.rs
git commit -m "Add Tauri commands get_quick_insert_target and quick_insert_to_target"
```

---

### Task 4: 前端 hook `useQuickInsertTarget`

**Files:**

- Create: `src/hooks/useQuickInsertTarget.ts`

- [ ] **Step 4.1: 创建 hook**

新建 `src/hooks/useQuickInsertTarget.ts`，内容：

```typescript
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

export interface QuickInsertTarget {
  app_name: string;
  pid: number;
}

const POLL_INTERVAL_MS = 1000;

/**
 * Polls the backend every 1s for the most recent non-Votype frontmost app.
 * Polling only runs while the hook is mounted; cleanup clears the interval.
 *
 * Returns `null` when the backend has no target yet (app just started,
 * Wayland session, or accessibility permission missing).
 */
export function useQuickInsertTarget(): QuickInsertTarget | null {
  const [target, setTarget] = useState<QuickInsertTarget | null>(null);

  useEffect(() => {
    let cancelled = false;

    const tick = async () => {
      try {
        const next = await invoke<QuickInsertTarget | null>(
          "get_quick_insert_target",
        );
        if (!cancelled) setTarget(next);
      } catch {
        // Backend error → fall back to "no target" rather than crashing UI.
        if (!cancelled) setTarget(null);
      }
    };

    tick(); // immediate first read so first paint is accurate
    const id = setInterval(tick, POLL_INTERVAL_MS);

    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, []);

  return target;
}
```

- [ ] **Step 4.2: tsc 验证（确保类型正确）**

```bash
bun tsc --noEmit 2>&1 | tail -10
```

Expected: 0 errors。

- [ ] **Step 4.3: 提交**

```bash
cd /Users/zac/code/github/asr/Handy
git add src/hooks/useQuickInsertTarget.ts
git commit -m "Add useQuickInsertTarget polling hook"
```

---

### Task 5: 添加 i18n 翻译 key

**Files:**

- Modify: `src/i18n/locales/en/translation.json`
- Modify: `src/i18n/locales/zh/translation.json`

**说明：** 只改 en + zh；其它 13 个 locale 文件不动，i18next 默认会 fallback 到 en（项目既有行为）。

- [ ] **Step 5.1: 在 en/translation.json 的 dashboard.actions 段加 5 个 key**

打开 `src/i18n/locales/en/translation.json`。定位 `dashboard.actions` 段（约 line 82-93）：

```json
    "actions": {
      "edit": "Edit",
      "editDescription": "Modify the transcribed text for this history entry",
      "editPlaceholder": "Enter the modified text...",
      "editTitle": "Edit Recognition Result",
      "openRecordings": "Recordings",
      "reprocessFailed": "AI reprocessing failed",
      "retranscribe": "Re-transcribe using audio",
      "rejectPolish": "Reject polish result",
      "retranscribeFailed": "Re-transcription failed",
      "retranscribing": "Retranscribing..."
    },
```

替换为（按字母序插入 5 个新 key）：

```json
    "actions": {
      "edit": "Edit",
      "editDescription": "Modify the transcribed text for this history entry",
      "editPlaceholder": "Enter the modified text...",
      "editTitle": "Edit Recognition Result",
      "openRecordings": "Recordings",
      "quickInsert": "Insert into {{app}}",
      "quickInsertEmpty": "No recent external app detected",
      "quickInsertErrorFocus": "Failed to activate target window",
      "quickInsertErrorNoTarget": "No target app to insert into",
      "quickInsertErrorPaste": "Paste failed",
      "reprocessFailed": "AI reprocessing failed",
      "retranscribe": "Re-transcribe using audio",
      "rejectPolish": "Reject polish result",
      "retranscribeFailed": "Re-transcription failed",
      "retranscribing": "Retranscribing..."
    },
```

- [ ] **Step 5.2: 在 zh/translation.json 加同样 5 个 key**

打开 `src/i18n/locales/zh/translation.json`，定位对应的 `dashboard.actions` 段，按相同顺序（字母序）插入：

```json
      "quickInsert": "插入到 {{app}}",
      "quickInsertEmpty": "未检测到最近的外部应用",
      "quickInsertErrorFocus": "无法激活目标窗口",
      "quickInsertErrorNoTarget": "没有可插入的目标应用",
      "quickInsertErrorPaste": "粘贴失败",
```

> 中文 key 顺序与 en 保持一致（字母序），便于 diff 与协作。

- [ ] **Step 5.3: JSON 语法校验**

```bash
node -e "JSON.parse(require('fs').readFileSync('src/i18n/locales/en/translation.json','utf8'))" && echo "en OK"
node -e "JSON.parse(require('fs').readFileSync('src/i18n/locales/zh/translation.json','utf8'))" && echo "zh OK"
```

Expected: 两行都打印 `... OK`。

- [ ] **Step 5.4: 提交**

```bash
cd /Users/zac/code/github/asr/Handy
git add src/i18n/locales/en/translation.json src/i18n/locales/zh/translation.json
git commit -m "Add i18n keys for quick insert button and error toasts"
```

---

### Task 6: DashboardEntryCard 加 Insert 按钮

**Files:**

- Modify: `src/components/settings/dashboard/DashboardEntryCard.tsx`

**说明：** 两处编辑按钮位置（Tabs 模式 line 597-608、非 Tabs 模式 line 622-639）。Insert 按钮放在 Edit 按钮**左侧**——用 `Flex gap="1"` 把两个按钮并排放在原来的 `Box absolute top-2 right-2` 容器里。

- [ ] **Step 6.1: 补充 imports**

打开 `src/components/settings/dashboard/DashboardEntryCard.tsx`，定位顶部 import 段（line 1-29）。

把第 10-19 行的 Tabler import 改为（按字母序插入 `IconSend`）：

```typescript
import {
  IconCopy,
  IconMicrophone,
  IconPencil,
  IconPlayerPlay,
  IconSend,
  IconStar,
  IconThumbDown,
  IconTrash,
  IconWand,
} from "@tabler/icons-react";
```

在 import 段末尾（line 29 `import type { HistoryEntry, PostProcessStep } from "./dashboardTypes";` 与 line 30 `import { EditHistoryDialog, ...` 之间）插入：

```typescript
import { toast } from "sonner";
import { useQuickInsertTarget } from "../../../hooks/useQuickInsertTarget";
```

- [ ] **Step 6.2: 在组件函数体内拿到 hook 值并写一个共享的点击 handler**

定位组件函数体开始的位置（约 line 60，`(...) => {` 之后）。在 `const { t } = useTranslation();` 等 hook 调用附近（视实际为准，搜 `useTranslation` 定位）加入：

```typescript
const quickInsertTarget = useQuickInsertTarget();

const handleQuickInsert = useCallback(
  async (text: string) => {
    try {
      await invoke("quick_insert_to_target", { text });
    } catch (err) {
      const msg = typeof err === "string" ? err : String(err);
      if (msg.startsWith("focus_failed")) {
        toast.error(t("dashboard.actions.quickInsertErrorFocus"));
      } else if (msg.startsWith("paste_failed")) {
        toast.error(t("dashboard.actions.quickInsertErrorPaste"));
      } else {
        toast.error(t("dashboard.actions.quickInsertErrorNoTarget"));
      }
    }
  },
  [t],
);
```

> `invoke` 与 `useCallback` 都已经 import；`t` 已通过 `useTranslation()` 拿到（见现有 line 75 附近，搜 `useTranslation` 确认）。

- [ ] **Step 6.3: 修改 Tabs 模式的编辑按钮（line 596-608 附近）**

定位现有代码（约 line 596-608）：

```tsx
{
  /* Unified Edit Button for Tabs */
}
<Box className="absolute top-2 right-2 opacity-0 group-hover:opacity-100 transition-all z-20">
  <Tooltip content={t("dashboard.actions.edit")}>
    <IconButton
      variant="ghost"
      size="1"
      onClick={handleGlobalEdit}
      className="text-logo-primary hover:bg-logo-primary/10 cursor-pointer"
    >
      <IconPencil size={14} />
    </IconButton>
  </Tooltip>
</Box>;
```

替换为：

```tsx
{
  /* Quick Insert + Edit Buttons for Tabs */
}
<Box className="absolute top-2 right-2 opacity-0 group-hover:opacity-100 transition-all z-20">
  <Flex gap="1">
    <Tooltip
      content={
        quickInsertTarget
          ? t("dashboard.actions.quickInsert", {
              app: quickInsertTarget.app_name,
            })
          : t("dashboard.actions.quickInsertEmpty")
      }
    >
      <IconButton
        variant="ghost"
        size="1"
        disabled={!quickInsertTarget}
        onClick={() => handleQuickInsert(entry.transcription_text)}
        className="text-logo-primary hover:bg-logo-primary/10 cursor-pointer"
      >
        <IconSend size={14} />
      </IconButton>
    </Tooltip>
    <Tooltip content={t("dashboard.actions.edit")}>
      <IconButton
        variant="ghost"
        size="1"
        onClick={handleGlobalEdit}
        className="text-logo-primary hover:bg-logo-primary/10 cursor-pointer"
      >
        <IconPencil size={14} />
      </IconButton>
    </Tooltip>
  </Flex>
</Box>;
```

> `Flex` 已经在文件顶部 `import { Box, ... Flex, ... } from "@radix-ui/themes"`（line 1-9）。

- [ ] **Step 6.4: 修改非 Tabs 模式的编辑按钮（line 622-639 附近）**

定位现有代码（约 line 622-639）：

```tsx
{
  !isCancelled && (
    <Box className="absolute top-2 right-2 opacity-0 group-hover:opacity-100 transition-all z-20">
      <Tooltip content={t("dashboard.actions.edit")}>
        <IconButton
          variant="ghost"
          size="1"
          onClick={() =>
            openEditDialog("transcription_text", entry.transcription_text)
          }
          className="text-logo-primary hover:bg-logo-primary/10 cursor-pointer"
        >
          <IconPencil size={14} />
        </IconButton>
      </Tooltip>
    </Box>
  );
}
```

替换为：

```tsx
{
  !isCancelled && (
    <Box className="absolute top-2 right-2 opacity-0 group-hover:opacity-100 transition-all z-20">
      <Flex gap="1">
        <Tooltip
          content={
            quickInsertTarget
              ? t("dashboard.actions.quickInsert", {
                  app: quickInsertTarget.app_name,
                })
              : t("dashboard.actions.quickInsertEmpty")
          }
        >
          <IconButton
            variant="ghost"
            size="1"
            disabled={!quickInsertTarget}
            onClick={() => handleQuickInsert(entry.transcription_text)}
            className="text-logo-primary hover:bg-logo-primary/10 cursor-pointer"
          >
            <IconSend size={14} />
          </IconButton>
        </Tooltip>
        <Tooltip content={t("dashboard.actions.edit")}>
          <IconButton
            variant="ghost"
            size="1"
            onClick={() =>
              openEditDialog("transcription_text", entry.transcription_text)
            }
            className="text-logo-primary hover:bg-logo-primary/10 cursor-pointer"
          >
            <IconPencil size={14} />
          </IconButton>
        </Tooltip>
      </Flex>
    </Box>
  );
}
```

- [ ] **Step 6.5: tsc + lint 验证**

```bash
bun tsc --noEmit 2>&1 | tail -10
bun lint 2>&1 | tail -15
```

Expected: 0 errors。若 `bun lint` 报对当前文件 line 30 附近 import 顺序的告警（biome / eslint 可能要求 type import 与 value import 分组），按现有项目约定调整顺序——不要禁用 lint。

- [ ] **Step 6.6: 提交**

```bash
cd /Users/zac/code/github/asr/Handy
git add src/components/settings/dashboard/DashboardEntryCard.tsx
git commit -m "Add Insert button to history detail card for quick paste into previous app"
```

---

### Task 7: 全量校验 + 手动 smoke + 偏差回填

**Files:** （仅校验，无代码改动）

- [ ] **Step 7.1: 后端 clippy + fmt**

```bash
cd src-tauri && cargo clippy --all-targets -- -D warnings 2>&1 | tail -25
cd src-tauri && cargo fmt --check 2>&1 | tail -10
```

**重要 context：** 项目 `src-tauri/src/actions/transcribe.rs` 有 pre-existing clippy 错误（line 399、line 1530）与 transcribe 之外的若干 pre-existing 错误。

- 如果 clippy 仅报 pre-existing 违规（不在 `foreground_tracker.rs`、`commands/mod.rs` 新增段、`lib.rs` 新增行），记录为 pre-existing 并跳过修复。
- 如果 clippy 报本次 commit 引入的新违规，立即修复并补一个 commit。

若 `cargo fmt --check` 报 diff 且 diff 限定在本 task 涉及的文件中：

```bash
cd src-tauri && cargo fmt
cd /Users/zac/code/github/asr/Handy && git diff --stat
```

确认 diff 范围后：

```bash
cd /Users/zac/code/github/asr/Handy
git add -u src-tauri/
git commit -m "Apply cargo fmt"
```

若 diff 包含本 task 没碰过的文件，STOP，不要盲格式化。

- [ ] **Step 7.2: 后端全量测试**

```bash
cd src-tauri && cargo test --lib 2>&1 | tail -20
```

Expected: 新增 5 个 tracker 测试 + 既有测试全部 pass。已知 2 个 pre-existing failure（`test_has_repetition_pattern` 与 `ensure_post_process_defaults_restores_required_builtin_provider`）与本次无关，不算回归。

- [ ] **Step 7.3: 前端类型与 lint**

```bash
bun tsc --noEmit 2>&1 | tail -10
bun lint 2>&1 | tail -15
```

Expected: 0 errors（新文件 + 修改文件均通过）。

- [ ] **Step 7.4: 手动 smoke 测试（由 parent agent 与用户协调，subagent 不直接跑）**

跑 `bun tauri dev`，按 spec 验收场景验证：

1. **happy_path_insert_into_previous_app**：
   - 在 VSCode 编辑代码 → 切到 Votype 设置 → Dashboard 详情页找一条历史 → 鼠标 hover 出现两个按钮 → 点 Insert
   - 期望：settings 自动隐藏 + VSCode 拿到文本
   - 日志：能看到 `get_quick_insert_target` 在 hover 前已被前端 polling 多次调用，`quick_insert_to_target` 成功后 `[main.hide]` 路径触发
2. **empty_slot**：刚启动应用 < 500ms 内进入 Dashboard → 按钮置灰 + tooltip "未检测到最近的外部应用"
3. **rapid_switch**：VSCode → Chrome → Votype 各停留 > 500ms → 详情页 hover 看 tooltip = "插入到 Chrome"
4. **self_does_not_overwrite**：在 settings 停留 5 秒 → tooltip 仍显示之前的外部 app（不变成 Votype）
5. **target_closed**：关闭 VSCode → 立刻点 Insert（slot 仍可能短暂保有 VSCode 信息）→ toast "无法激活目标窗口"

> **error_path_paste_failure**（spec scenario 6）需在缺辅助权限环境下复现，比较难，靠代码 review 保证。
> **edge_case_dashboard_unmount_stops_polling**（spec scenario 7）需在 Dashboard ↔ 其他 settings 子页切换后通过 Network/devtools 观察 `get_quick_insert_target` 请求是否停止，可选。

- [ ] **Step 7.5: 回填 spec 偏差表**

打开 `docs/specs/2026-05-20-quick-insert-from-history-detail.spec.md`，定位 `## 实施偏差` 段。若实现完全按计划，把表替换为：

```markdown
| 原计划 | 实际实现 | 原因 |
| ------ | -------- | ---- |
| 无偏差 | —        | —    |
```

否则按实际差异填写（例如：发现某个 invoke_handler 位置不同、某个 import 顺序被 lint 调整、i18n 文件中已有同名 key 等）。

- [ ] **Step 7.6: 最终提交**

```bash
cd /Users/zac/code/github/asr/Handy
git add docs/specs/2026-05-20-quick-insert-from-history-detail.spec.md
git commit -m "Record implementation deviations for quick-insert from history detail"
```

---

## Self-Review

**1. Spec coverage：**

| Spec 要求                                              | 对应任务                                          |
| ------------------------------------------------------ | ------------------------------------------------- |
| 后端追踪器 + 500ms 轮询 + slot 过滤 self_pid           | Task 1（含 next_slot_value 纯函数 + 3 单测）      |
| Tauri command get/insert 接口契约（含 3 类 err 前缀）  | Task 3（commands/mod.rs）                         |
| 前端 1Hz hook，挂载/卸载托管                           | Task 4（useQuickInsertTarget）                    |
| 详情页 Insert 按钮（Tabs + 非 Tabs 两处）              | Task 6（DashboardEntryCard）                      |
| i18n key（en + zh，其他 fallback）                     | Task 5                                            |
| 成功后 main_window.hide 由后端负责                     | Task 3 Step 3.2                                   |
| 失败时 settings 保持打开 + 前端 toast                  | Task 3 + Task 6 Step 6.2                          |
| 验收场景 1-7                                           | Task 1 单测覆盖部分；Task 7 Step 7.4 手动覆盖其余 |
| 不动 active_window / clipboard / input / review_window | 所有 task 的 Files 段均无以上文件                 |
| 不引入新平台 crate 依赖                                | 仅使用 once_cell / tokio / serde（既有）          |

**2. Placeholder scan：** 无 TBD/TODO/"implement later"/"add appropriate error handling"。所有代码块完整且可直接复制。Step 1.1 中 `unimplemented!()` 是 TDD red 阶段的合理占位，Step 1.3 明确移除。

**3. Type consistency：**

- `QuickInsertTarget { app_name, pid }` 在 Task 3（Rust struct）与 Task 4（TS interface）字段名一致（snake_case，因为 serde 默认）。
- `next_slot_value` / `set_last_external_frontmost` / `get_last_external_frontmost` 在 Task 1.1 测试、Task 1.3 实现、Task 3.2 调用方使用完全一致的命名。
- i18n key 命名（quickInsert / quickInsertEmpty / quickInsertErrorFocus / quickInsertErrorNoTarget / quickInsertErrorPaste）在 Task 5 写入与 Task 6 引用一致。
- err 前缀 `no_target` / `focus_failed:` / `paste_failed:` 在 spec 决策段、Task 3 Step 3.2 后端实现、Task 6 Step 6.2 前端映射均一致。
