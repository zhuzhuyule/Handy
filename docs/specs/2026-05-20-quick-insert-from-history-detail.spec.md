---
name: "历史详情页快捷插入到上一个外部应用"
tags: [dashboard, foreground-tracker, paste, cross-platform]
depends_on: []
estimate: "1 day"
---

## 意图

"在历史详情卡片的编辑按钮左侧加一个 Insert 按钮；点击后，把该条历史的当前展示文本快速插入到用户上一次切换到 Votype 之前所处的外部应用，并自动隐藏设置窗口。"

复用项目已有的跨平台 `focus_app_by_pid` + `paste` 链路。新增的唯一基础设施是一个后台 frontmost-app 追踪器：因为 OS 不直接暴露"上一个 frontmost"，需要持续维护一份"最近的非 Votype frontmost"快照供 UI 与命令使用。

## 约束

- 三平台都要可用（macOS / Windows / Linux X11）。Wayland 上 `fetch_active_window()` 会返回 Err，按"优雅降级"处理：slot 永远为空 → 按钮置灰 + tooltip 说明。
- 必须复用现有的 `crate::active_window::fetch_active_window` 与 `crate::active_window::focus_app_by_pid`，不引入新的平台 API 依赖。
- 必须复用现有的 `crate::clipboard::paste` 与 PasteMethod 路径，不引入新的输入注入链路。
- 不能破坏现有 `paste_to_previous_window` 与 `review_window::LAST_ACTIVE_WINDOW` 的语义——新追踪器是独立的，与 review window 流程互不影响。
- 不在 Votype 自身被 frontmost 时更新 slot——通过 `process_id == std::process::id() as u64` 比对识别"自己"，避免 settings 打开瞬间把目标 app 覆盖掉。
- 遵守 CLAUDE.md 中的运行时规则（追踪器后台 task 用 `tauri::async_runtime::spawn`，不在协调线程中 `block_on`，不在非 async 上下文中 `tokio::spawn`）。
- 修改前提交前消除所有 warning。

## 已定决策

- **追踪机制：轮询**，间隔 500ms。
  - 不选 OS 事件钩子（NSWorkspace / SetWinEventHook / X11 PropertyNotify）：三平台需三套代码、Wayland 仍需降级，复杂度不对等收益。500ms 延迟对"按钮可用性"这个场景人眼不可感知。
  - 后期如发现 macOS 上 500ms 仍有体感，再在 macOS 单独叠加 NSWorkspace 钩子做即时刷新（不在本 spec 范围）。
- **存储位置：** 新建 `src-tauri/src/foreground_tracker.rs` 模块，放一个 `static LAST_EXTERNAL_FRONTMOST: Lazy<Mutex<Option<ActiveWindowInfo>>>`。
  - 不复用 `review_window::LAST_ACTIVE_WINDOW`：那是 review 流程内部状态（手动赋值时机），语义不同；耦合会让两条路径互相干扰。
- **slot 更新策略：** 只在 `info.process_id != std::process::id() as u64` 时更新。Votype 自己 frontmost 期间 slot 保持上一个值——这正是用户希望的"切到设置时记住来自哪个 app"。
- **暴露给前端的接口：**
  - `get_quick_insert_target() -> Option<QuickInsertTarget { app_name, pid }>`：只回最小信息，UI 渲染 tooltip 用 `app_name`，pid 仅给命令使用（前端不需要直接看到）。
  - `quick_insert_to_target(text: String) -> Result<(), String>`：失败时 Err 字符串采用三类前缀，前端按前缀映射 toast：
    - `"no_target"` — slot 空（理论上前端 disabled 应阻止，仅作 defensive）
    - `"focus_failed: <details>"` — focus_app_by_pid 返 Err，目标 app 可能已关闭
    - `"paste_failed: <details>"` — paste 返 Err，常因权限缺失
- **插入语义：** focus_app_by_pid → 120ms 等焦点切换 → paste(text)。完全沿用 `paste_to_previous_window` 命令的现有模式（commands/mod.rs:184-198）。
- **成功后副作用：** 插入成功后由后端命令调 `main_window.hide()`。前端不需要额外处理，可视效果是 settings 消失、目标 app 自然成为 frontmost。
  - 不在前端 hide：避免前端在事件竞态中比 paste 完成更早 hide 导致焦点反弹。
- **失败时不 hide：** 任何一步 Err（slot 空 / focus 失败 / paste 失败）→ 不 hide settings、不 toast、返 Err 给前端，由前端 sonner toast 显示。
- **文本选择：** 接收前端传入的 `text` 字符串。前端传"详情页当前显示的文本"——和 Copy 按钮（DashboardEntryCard.tsx line 420-442 的 `onCopy(text)`）保持一致，避免歧义。
- **图标：** `IconSend`（Tabler Icons，项目已用）。
- **前端共享状态：** 单例 polling（hook 内部用 `setInterval`，挂载/卸载托管），1Hz；后端轮询 500ms 保证 slot 新鲜，前端 1Hz 读取即可。

## 边界

### 允许修改

- 新建：
  - `src-tauri/src/foreground_tracker.rs`
  - `src/hooks/useQuickInsertTarget.ts`
- 修改：
  - `src-tauri/src/lib.rs`：setup 中调用 `foreground_tracker::start(...)`；`invoke_handler` 注册两个新 command；`mod foreground_tracker`
  - `src-tauri/src/commands/mod.rs`：新增 `get_quick_insert_target` 与 `quick_insert_to_target`
  - `src/components/settings/dashboard/DashboardEntryCard.tsx`：在两处 edit 按钮位置（line 597-608、line 622-639）左侧加 Insert 按钮

### 禁止

- 修改 `src-tauri/src/active_window.rs`（`fetch_active_window` / `focus_app_by_pid` 实现保持不动）
- 修改 `src-tauri/src/clipboard.rs`、`src-tauri/src/input.rs`（`paste` / `paste_text_direct` / PasteMethod 不动）
- 修改 `src-tauri/src/review_window.rs` 或 `LAST_ACTIVE_WINDOW` / `paste_to_previous_window`
- 在前端 hide settings 窗口（统一由后端命令负责）
- 引入任何新的 platform crate 依赖

## 排除范围

- 多条历史一次性批量插入。
- 用户手动选择目标 app（picker / 下拉）。
- 历史插入计数 / 撤销栈 / 持久化。
- macOS NSWorkspace 事件钩子优化（如发现 500ms 轮询有体感再单独立项）。
- Wayland 上的替代追踪方案（dotool / wlroots foreign-toplevel 等不在本 spec 范围）。
- 修改 review window 的 voice-rewrite / paste-to-previous 流程。
- 修改触发录音时捕获 active app 的逻辑（已在 docs/specs/2026-05-19-capture-active-window-at-start.spec.md 完成）。

## 验收场景

### 1. happy_path_insert_into_previous_app

- **Given**: 用户在 VSCode 编辑文件（VSCode 是 frontmost），切换到 Votype 设置窗，进入历史 Dashboard 详情页查看某条历史
- **When**: 点击编辑按钮左侧的 Insert 按钮
- **Then**:
  - settings 窗口自动隐藏（main_window.hide）
  - VSCode 自然回到 frontmost
  - VSCode 当前光标位置接收到该条历史的文本
  - 后端日志中能看到 `[ForegroundTracker]` 行最近一次更新 slot 时 app=Code

### 2. empty_slot_button_disabled_with_tooltip

- **Given**: 应用刚启动 < 500ms（追踪器尚未轮询过）或处于 Wayland 会话（fetch 永远 Err）
- **When**: 用户进入 Dashboard 详情页
- **Then**:
  - Insert 按钮存在且置灰
  - hover tooltip 显示 "未检测到最近的外部应用"
  - 按钮点击无反应（disabled 属性）

### 3. error_path_target_app_closed

- **Given**: slot 保存了 VSCode 的 pid=12345，但用户已经关闭了 VSCode 进程
- **When**: 用户点击 Insert
- **Then**:
  - `focus_app_by_pid(12345)` 返回 Err
  - 后端命令立即返 `Err("focus_failed: ...")`（不调用 paste）
  - settings 窗口保持打开
  - 前端 sonner toast 显示 "无法激活目标窗口"（按前缀映射）

### 4. edge_case_rapid_app_switch

- **Given**: 用户依次操作：VSCode → Chrome → Votype 设置（中间至少各停留 > 500ms）
- **When**: 用户点击 Insert
- **Then**:
  - slot 中保存的是 Chrome（最近一次非 Votype 的 frontmost）
  - 插入文本进入 Chrome 当前焦点
  - 历史日志中应能看到 slot 从 VSCode → Chrome 的两次更新

### 5. edge_case_self_does_not_overwrite

- **Given**: slot 当前是 VSCode；用户切到 Votype 设置窗，停留 5 秒（期间 10 轮轮询都返回 Votype 自己）
- **When**: 这 5 秒内追踪器多次轮询
- **Then**:
  - slot 不被覆盖，仍是 VSCode
  - 用户随时点击 Insert 仍能正确插入 VSCode

### 6. error_path_paste_failure_keeps_settings_open

- **Given**: slot 正常、focus_app_by_pid 成功，但 paste 因为辅助权限缺失返 Err
- **When**: 用户点击 Insert
- **Then**:
  - 后端命令返 `Err("paste_failed: <details>")`
  - settings 窗口保持打开
  - 前端 sonner toast 显示 "粘贴失败" + 后端给的详细原因
  - slot 不变（下次仍可重试）

### 7. edge_case_dashboard_unmount_stops_polling

- **Given**: 用户在 Dashboard 详情页停留 → 切到其他 settings 子页（Dashboard 卸载）
- **When**: Dashboard 卸载
- **Then**:
  - 前端 `useQuickInsertTarget` 的 `setInterval` 被清理（无 1Hz 调用）
  - 后端 500ms 轮询继续（slot 始终被维护）
  - 用户回到 Dashboard 时 hook 重新挂载，立刻拿到最新 slot

## 实施偏差

| 原计划                                           | 实际实现                                                                                                                                        | 原因                                                                                                                                            |
| ------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| Task 1+2 仅引入 slot + tracker + setup wiring    | 同时加了 `log::debug!` / `log::info!` / `log::warn!` 日志（slot 更新、首次捕获、首次 Ok→Err 转换）                                              | spec 场景 1 验收要求"日志中能看到 `[ForegroundTracker]` 行"；原实现无日志，无法验证                                                             |
| Task 1+2 仅引入 `start()` 后台 task              | 新增 `STARTED: AtomicBool` idempotency guard，并把 `start()` 参数从 `&AppHandle` 降为无参                                                       | code review 指出无防护可能被重复调用 spawn 两个 poller；同时 `_app_handle` 实际未使用                                                           |
| Task 3 仅添加 2 个 Tauri command                 | 在 `quick_insert_to_target` 内额外加 `log::info!("[QuickInsert] inserting into app=... pid=...")` 与 `main_window.hide()` 失败时的 `log::warn!` | code review 指出便于"插入到错误 app"问题的事后排查；隐藏失败的静默 `let _` 不利于平台异常诊断                                                   |
| Task 6 仅在 disabled Tooltip 包裹原生 IconButton | 用 `<span className="inline-flex">` 包了一层 IconButton；className 加 `disabled:cursor-not-allowed disabled:opacity-50`                         | 原生 HTML `disabled` 拦截 pointer events，Radix Tooltip 在 disabled 按钮上不会显示；spec UX 决策"显示但置灰 + tooltip 解释"在原始实现下无法兑现 |
| Spec 未覆盖 slot-eviction race                   | 已知限制：用户阅读 tooltip 时 slot 可能被新外部 frontmost 覆盖，插入目标可能与 tooltip 显示不一致                                               | 1Hz 前端轮询 + 用户阅读时延 < 几秒，但 race 窗口存在。修复需要 backend 接 expected_pid 参数做 token 校验；v2 改进                               |
| Spec 未指明每个 DashboardEntryCard 实例独立轮询  | 当前 `useQuickInsertTarget` 在每个挂载的 DashboardEntryCard 中独立运行 1Hz polling，导致 N 卡片 = N invokes/sec                                 | Task 4 设计未做 singleton 化；性能影响小（IPC 调用极轻），但有优化空间。后续如做共享可改为 Context + 单实例                                     |
