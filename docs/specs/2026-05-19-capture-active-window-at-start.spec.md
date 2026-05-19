---
name: "录音开始时即捕获活动应用"
tags: [shortcut, transcribe, focus, ux]
depends_on: []
estimate: "0.5 day"
---

## 意图

"按下录音键或指令键时立即记录当前激活的应用程序；结束录音时，无论焦点是否切换，转录结果都被输入到按键时记录的那个应用。"

当前实现仅在 `stop()`（第二次按键）时调用 `fetch_active_window()`，因此用户在录音过程中切换到别的窗口（例如查阅资料、切换浏览器标签），最终输入文本会被发送到错误的应用。把捕获点提前到 `start()`，让"按键瞬间的焦点"作为输入目标的真源。

## 约束

- 仅修改活动窗口快照的捕获时机；`selected_text` 与 `cursor_context` 仍在 `stop()` 读取（用户主动决策）。
- 必须按 `transcription_id` 配对快照，避免多次按键 / 并发触发时的脏数据。
- 必须保留 `stop()` 端的 fallback：start 时未能捕获到快照（异常或新路径未接入）时退回原来的"实时 fetch"行为，以免回归。
- `review-window-local`（voice rewrite）流程的目标是 review 窗口本身，新行为不能破坏既有的 `votype_mode = ReviewRewrite` 与重写注入逻辑。
- `always_on_microphone` 模式下 `start()` 仍被调用，行为一致。
- 遵守 CLAUDE.md 中的运行时规则（不在协调线程中 `block_on`、不在非 async 上下文中 `tokio::spawn`）。捕获操作在 start 同步执行，无 async 调用。
- 清理责任：早返回（录音过短）、用户取消、异常 panic 时不得残留过期快照。
- 不引入新依赖；继续使用项目现有的 `once_cell::sync::Lazy + std::sync::Mutex` 模式（参考 `review_window.rs:108` 的 `LAST_ACTIVE_WINDOW`）。

## 已定决策

- **存储位置：** 在 `src-tauri/src/actions/transcribe.rs` 模块内新增一个静态 `Lazy<Mutex<Option<(u64, ActiveWindowInfo)>>>`，以 `transcription_id` 为 key。
  - 不选 HashMap：实际上同一时刻只有一个录音，单 slot 足够，且配 id 校验后即可隔离串扰。
  - 不放进 `AudioRecordingManager`：避免在 manager 层增加 UI/平台耦合；transcribe action 是天然的归属点。
- **捕获时机：** `start()` 中拿到 `new_id = rm.increment_transcription_id()`（已存在，line 752）且 `recording_started == true`（line 800 之后）时同步调用 `fetch_active_window().ok()` 并写入 slot。失败（如缺权限）记 warn，slot 写入 `None`。
- **消费时机：** `stop()` 异步管线中替换 `transcribe.rs:944` 的 `fetch_active_window()`，改为先按 `current_transcription_id` `.take()` 取回；取不到则 fallback 到 `fetch_active_window()` 并记 warn。
- **清理时机：**
  - `take_start_snapshot()` 用 `.take()` 在 stop 路径自然清空匹配 id 的值。
  - `FinishGuard::drop` 兜底：若 slot 仍持有 `current_transcription_id` 对应的快照（例如 stop 在 line 944 之前 panic），无条件清掉自己的那一份。
  - id 不匹配时不动 slot，避免误清掉后启动的新录音。
- **行为分离：** 不动 `selected_text` / `cursor_context`；不动 `review-window-local` 的 `votype_mode` 判定；不动后续 `focus_app_by_pid` 调用链。
- **日志：** 捕获时 debug 一行（`app/title/pid`），用 `[StartSnapshot]` 前缀，方便排障。

## 边界

### 允许修改

- `src-tauri/src/actions/transcribe.rs`
  - 新增模块级静态 + 两个私有 helper（`set_start_snapshot`、`take_start_snapshot`、`clear_start_snapshot_if_matches`）
  - `TranscribeAction::start()` 内增加捕获调用
  - `TranscribeAction::stop()` 内替换 line 944 的捕获、`FinishGuard::drop` 内加兜底清理

### 禁止

- 修改 `src-tauri/src/active_window.rs`（不动 `fetch_active_window` / `focus_app_by_pid` 实现）。
- 修改 `src-tauri/src/clipboard.rs`、`get_selected_text` / `get_cursor_context` 行为。
- 修改 `src-tauri/src/review_window.rs`、`votype_mode` 推导、`PromptBuilder` 等下游消费者。
- 修改 `src-tauri/src/managers/audio_recording.rs` 等 manager 层（保持 transcription_id 单一来源）。
- 不允许在 `start()` 中执行任何 async 操作或剪贴板/选区读取——只读窗口元数据。

## 排除范围

- 不在 start 时捕获 `selected_text` / `cursor_context`（已与用户确认）。
- 不调整 always_on 模式的语音活动检测/边界。
- 不引入新的 UX 提示（如"已锁定到 X 应用"的浮层）。
- 不修改 `commands/mod.rs` 中独立的 `get_active_window_info` Tauri 命令（review 窗口用）。
- 不修改 `pipeline.rs:1753` 那条独立的 `fetch_active_window()` 调用（属于另一条路径）。

## 验收场景

### 1. happy_path_switch_app_during_recording

- **Given**: 用户在 Slack 输入框中获得焦点，按下录音键启动录音
- **When**: 录音过程中切换到 Chrome 浏览器查资料，然后再按录音键结束
- **Then**:
  - 应用焦点自动切回 Slack（由 `focus_app_by_pid` 完成）
  - 转录结果输入到 Slack 输入框
  - 日志中 `[StartSnapshot]` 行 app=Slack，stop 路径未再次调用 `fetch_active_window()`

### 2. happy_path_no_switch

- **Given**: 用户在 Notion 中按下录音键并保持焦点不动
- **When**: 录音结束
- **Then**:
  - 行为与改动前一致：内容输入到 Notion
  - start 时已捕获到 Notion，stop 时复用，无回归

### 3. edge_case_too_short_recording

- **Given**: 用户按下录音键后立即又按一次（< 500ms）
- **When**: `classify_recording_samples` 判定 `IgnoredTooShort` 提前 return
- **Then**:
  - `FinishGuard::drop` 触发，清空当前 `transcription_id` 对应的 slot
  - 后续新录音不会读到这条过期快照

### 4. edge_case_overlapping_sessions

- **Given**: 旧录音 pipeline 仍在异步阶段（FinishGuard 未 drop），用户立刻按下新一次录音
- **When**: 新 `start()` 调用 `increment_transcription_id()` 拿到更大的 id，覆盖 slot
- **Then**:
  - 旧 pipeline 的 stop 路径用旧 id `.take()` 取不到自己的快照（已被新录音覆盖），fallback 到 `fetch_active_window()` 实时读取——此时旧录音已被新录音 race 抢占，无论结果如何均是既有 race 行为，不构成回归
  - 新录音正常按新快照走

### 5. error_path_accessibility_denied

- **Given**: macOS 上未授予辅助权限，`get_active_window()` 返回 Err
- **When**: 用户按下录音键
- **Then**:
  - `fetch_active_window().ok()` 返回 `None`，slot **不写入**（保持 None 或保留更旧的清理过状态），记 warn
  - stop 路径调用 `take_start_snapshot(current_id)` 取不到匹配项，fallback 到 `fetch_active_window()`，仍返回 `None`
  - 下游 `focus_app_by_pid` 跳过（既有逻辑），整体降级到"输入到当前焦点"——与改动前权限缺失场景行为一致

### 6. edge_case_review_window_voice_rewrite

- **Given**: 用户在 review 窗口按下 voice rewrite 快捷键（`shortcut_str == "review-window-local"`）
- **When**: 录音 → 结束
- **Then**:
  - start 时仍捕获快照（review 窗口或其父窗口），但 `votype_mode = ReviewRewrite` 路径不依赖该快照决定写入位置
  - 重写结果通过 `review-window-rewrite-apply` 事件回写 review 窗口，行为无回归

## 实施偏差

| 原计划                                                                                    | 实际实现                                                                                                                                                                                               | 原因                                                                                                                                                                                                                                 |
| ----------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Task 1 仅引入 slot + helpers + 测试                                                       | 新 helpers 临时加上 `#[allow(dead_code)]`，Task 2/3/4 接线后逐步移除；末态无残留                                                                                                                       | CLAUDE.md 强制要求提交前消除所有 warning，未引用的新符号会触发 `dead_code` lint                                                                                                                                                      |
| 测试代码完全复刻计划示例                                                                  | 省略两条中文行内注释（`// 先确保 slot 干净`、`// 第二次 take 同 id 应得到 None`）                                                                                                                      | 风格简化，不影响断言与行为                                                                                                                                                                                                           |
| Task 1 仅添加 3 个测试和 helper                                                           | 新增 `SNAPSHOT_TEST_LOCK: std::sync::Mutex<()>` 与 `make_test_snapshot()` 测试工具                                                                                                                     | 三测试共享同一个全局 slot，需要 test-side Mutex 串行化以防 cargo 并行运行时互相覆盖                                                                                                                                                  |
| Task 3 仅替换 `stop()` 中 line 944 的 `fetch_active_window()` 调用                        | 同时将 `Active window (snapshot): ...` 日志扩展为带 `source` 标签的 Some/None 双分支（`Active window (start/stop-fallback): ...` 与 `Active window (none, source=...): id=...`）                       | 增加可观测性：让运维能区分快照命中、fallback 与权限缺失三种路径，并把 id 写到 None 分支便于与 `[StartSnapshot]` 行配对                                                                                                               |
| 取消（ESC）录音时也应清理 slot                                                            | 当前 `cancel_current_operation`（`utils.rs:33`）不调用 `FinishGuard::drop`，也不 bump transcription id，导致 cancel-mid-recording 后 slot 残留旧 `(id, info)` 直到下次成功的 `set_start_snapshot` 覆盖 | 残留无功能影响（后续 `take_start_snapshot` 通过 id 校验自动跳过）、无安全风险，仅占用一份 `ActiveWindowInfo`（几百字节）。修复需要在 utils.rs 调用 transcribe 模块的 `pub(super)` helper，会扩大文件边界。建议作为后续小改单独处理。 |
| Reviewer 建议保留 `fetch_active_window()` 的原始 Err 字符串到 warn 日志                   | 未采纳，仍按计划用 `.ok()` 丢弃                                                                                                                                                                        | 计划字面要求 `.ok()`；现有 warn 已能定位失败发生在 start 路径，错误细节属可选增强                                                                                                                                                    |
| Reviewer 建议把 source 标签改为更直白的 `live`/`fallback`，并在 Some 分支日志中也带 `id=` | 未采纳，保持 `"start"`/`"stop-fallback"` 与现有字段集                                                                                                                                                  | 当前命名在 grep 时已无歧义；id 仅在 None 分支保留以避免 happy-path 日志冗长                                                                                                                                                          |
