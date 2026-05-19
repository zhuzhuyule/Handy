# 录音开始时即捕获活动应用 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把活动窗口快照从 `stop()` 时即时获取，改为 `start()` 时按 `transcription_id` 暂存、`stop()` 时按 id 取回，让转录结果始终被输入到按键瞬间的那个应用。

**Architecture:** 在 `src-tauri/src/actions/transcribe.rs` 引入模块级 `Lazy<Mutex<Option<(u64, ActiveWindowInfo)>>>` slot；新增三个 `pub(super)` helper（`set_start_snapshot` / `take_start_snapshot` / `clear_start_snapshot_if_matches`）；`start()` 同步调用 `fetch_active_window` 并写 slot；`stop()` 改用 `take_start_snapshot(current_id)` 并 fallback 到原 `fetch_active_window()`；`FinishGuard::drop` 兜底清理。

**Tech Stack:** Rust + Tauri 2.x；`once_cell::sync::Lazy`、`std::sync::Mutex`（项目已用模式，参考 `src-tauri/src/review_window.rs:108`）；测试位于 `transcribe.rs` 的 `#[cfg(test)] mod tests` 块。

**Spec:** `docs/specs/2026-05-19-capture-active-window-at-start.spec.md`

---

## File Structure

| 文件                                  | 改动                                                              | 责任             |
| ------------------------------------- | ----------------------------------------------------------------- | ---------------- |
| `src-tauri/src/actions/transcribe.rs` | 新增 import / 静态 slot / 3 个 helper / 3 处调用点 / 3 个单元测试 | 全部改动集中于此 |

不新建文件。不修改其他文件（边界严格遵循 spec）。

---

### Task 1: 添加 slot + 三个 helper（TDD）

**Files:**

- Modify: `src-tauri/src/actions/transcribe.rs`（imports 段 + 文件末尾 helper 段 + tests 模块）

**说明：** 这是状态层的纯函数测试，使用唯一 `transcription_id` 避免测试间干扰。helper 全部 `pub(super)` 以便测试与同模块的 `start/stop` 调用。

- [ ] **Step 1.1: 在 tests 模块写 3 个失败的测试**

打开 `src-tauri/src/actions/transcribe.rs`，定位 `#[cfg(test)] mod tests` 块（约 line 3210）。修改 `use super::{...}` 增加三个新 helper 与 slot id 类型；在文件末尾追加三个新测试。

将 `mod tests` 块开头的 `use super::{...}` 改为：

```rust
    use super::{
        classify_recording_samples, clear_start_snapshot_if_matches, direct_paste_target_pid,
        effective_recording_duration_ms, resolve_post_process_outcome,
        set_start_snapshot, should_show_post_process_review_window, take_start_snapshot,
        PostProcessOutcome, RecordingDisposition, EFFECTIVE_RECORDING_TOO_SHORT_MS,
        WHISPER_SAMPLE_RATE,
    };
```

在该测试模块最末尾（`direct_paste_target_pid_uses_snapshot_pid` 之后，`}` 闭合 mod 之前）追加：

```rust
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
        let _guard = SNAPSHOT_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let id: u64 = 0xA001_0001;
        // 先确保 slot 干净（防御之前 panic 测试残留）
        clear_start_snapshot_if_matches(id);

        set_start_snapshot(id, Some(make_test_snapshot(101, "AppA")));

        let taken = take_start_snapshot(id).expect("should return snapshot for matching id");
        assert_eq!(taken.process_id, 101);
        assert_eq!(taken.app_name, "AppA");

        // 第二次 take 同 id 应得到 None（已被清掉）
        assert!(take_start_snapshot(id).is_none());
    }

    #[test]
    fn start_snapshot_take_with_mismatching_id_returns_none_and_keeps_slot() {
        let _guard = SNAPSHOT_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let id_owner: u64 = 0xA001_0002;
        let id_other: u64 = 0xA001_0003;
        clear_start_snapshot_if_matches(id_owner);
        clear_start_snapshot_if_matches(id_other);

        set_start_snapshot(id_owner, Some(make_test_snapshot(202, "AppB")));

        // 不匹配的 id 不能拿走快照
        assert!(take_start_snapshot(id_other).is_none());

        // 真正的 owner 仍能拿到
        let taken = take_start_snapshot(id_owner).expect("owner should still take its snapshot");
        assert_eq!(taken.app_name, "AppB");
    }

    #[test]
    fn clear_start_snapshot_if_matches_only_clears_owned_id() {
        let _guard = SNAPSHOT_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let id_owner: u64 = 0xA001_0004;
        let id_other: u64 = 0xA001_0005;
        clear_start_snapshot_if_matches(id_owner);
        clear_start_snapshot_if_matches(id_other);

        set_start_snapshot(id_owner, Some(make_test_snapshot(303, "AppC")));

        // 用别的 id 调用清理是 no-op
        clear_start_snapshot_if_matches(id_other);
        assert!(
            take_start_snapshot(id_owner).is_some(),
            "non-matching clear must not drop owner's snapshot"
        );

        // 重新放入，用本人的 id 清理
        set_start_snapshot(id_owner, Some(make_test_snapshot(303, "AppC")));
        clear_start_snapshot_if_matches(id_owner);
        assert!(take_start_snapshot(id_owner).is_none());
    }
```

> **测试 id 设计**：每个测试用独立高位前缀（`0xA001_00xx`）；同时通过 `SNAPSHOT_TEST_LOCK` 串行化执行，避免并行测试覆盖共享的全局 slot。

- [ ] **Step 1.2: 运行测试，确认失败**

```bash
cd src-tauri && cargo test --lib actions::transcribe::tests::start_snapshot 2>&1 | tail -30
```

Expected: 编译错误 — `cannot find function set_start_snapshot in this scope` / `cannot find function take_start_snapshot in this scope` / `cannot find function clear_start_snapshot_if_matches in this scope`。

- [ ] **Step 1.3: 添加 import 与 slot 定义**

在 `src-tauri/src/actions/transcribe.rs` 第 19 行 `use log::{debug, error, info};` 之后插入：

```rust
use once_cell::sync::Lazy;
use std::sync::Mutex as StdMutex;
```

> 用 `StdMutex` 别名是为了避免与 tokio/parking_lot 的 Mutex 在同一文件里混用（本文件已经 `use std::sync::Arc;`，但没有显式 import `std::sync::Mutex`）。如果将来需要 tokio Mutex 不会冲突。

- [ ] **Step 1.4: 在 `impl TranscribeAction {` 之前（约 line 269）添加 slot 与 3 个 helper**

定位 `impl TranscribeAction {`（line 269）。在该 `impl` 块上方插入：

```rust
/// 在 `start()` 按键瞬间捕获到的活动窗口快照，配对 `transcription_id`。
/// `stop()` 取回时必须校验 id，避免被后续录音覆盖时仍把旧快照交给已 race 的旧 pipeline。
static START_SNAPSHOT: Lazy<StdMutex<Option<(u64, crate::active_window::ActiveWindowInfo)>>> =
    Lazy::new(|| StdMutex::new(None));

/// 写入 start 时的活动窗口快照。`info = None`（fetch 失败）时不占用 slot，
/// stop 取回时会按既有兜底再次 `fetch_active_window()`。
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
pub(super) fn clear_start_snapshot_if_matches(transcription_id: u64) {
    let mut slot = START_SNAPSHOT.lock().expect("START_SNAPSHOT poisoned");
    if matches!(slot.as_ref(), Some((stored_id, _)) if *stored_id == transcription_id) {
        *slot = None;
    }
}
```

- [ ] **Step 1.5: 再次运行测试，确认通过**

```bash
cd src-tauri && cargo test --lib actions::transcribe::tests::start_snapshot 2>&1 | tail -20
cd src-tauri && cargo test --lib actions::transcribe::tests::clear_start_snapshot 2>&1 | tail -20
```

Expected: 三个测试均 PASS。

- [ ] **Step 1.6: 提交**

```bash
cd /Users/zac/code/github/asr/Handy
git add src-tauri/src/actions/transcribe.rs
git commit -m "Introduce start-time active-window snapshot slot keyed by transcription id"
```

---

### Task 2: `start()` 中捕获快照

**Files:**

- Modify: `src-tauri/src/actions/transcribe.rs`（line 800 附近，`if recording_started {` 块内）

- [ ] **Step 2.1: 修改 `start()` — 在 register_cancel_shortcut 后捕获快照**

定位 `src-tauri/src/actions/transcribe.rs:800-825`：

```rust
        if recording_started {
            shortcut::register_cancel_shortcut(app);

            if enable_realtime {
                let tm_realtime = app.state::<Arc<TranscriptionManager>>().inner().clone();
                ...
```

在 `shortcut::register_cancel_shortcut(app);` 之后、`if enable_realtime {` 之前插入：

```rust
            // 按键瞬间记录目标应用：stop() 时即使用户已经切换了焦点，
            // 我们仍能把转录结果送回最初的那个窗口。
            // 见 docs/specs/2026-05-19-capture-active-window-at-start.spec.md
            let start_snapshot = active_window::fetch_active_window().ok();
            match &start_snapshot {
                Some(info) => debug!(
                    "[StartSnapshot] captured id={} app='{}' title='{}' pid={}",
                    new_id, info.app_name, info.title, info.process_id
                ),
                None => log::warn!(
                    "[StartSnapshot] fetch_active_window failed for id={}, stop() will fallback",
                    new_id
                ),
            }
            set_start_snapshot(new_id, start_snapshot);
```

> **位置选择原因：** 必须在 `recording_started == true` 且 `new_id` 已分配之后（new_id 在 line 752 拿到）。放在 `register_cancel_shortcut` 后，与"录音正式生效"的其它副作用并列。

- [ ] **Step 2.2: 编译检查**

```bash
cd src-tauri && cargo check 2>&1 | tail -20
```

Expected: 0 errors, 0 warnings。

- [ ] **Step 2.3: 跑现有测试，确认未回归**

```bash
cd src-tauri && cargo test --lib actions::transcribe 2>&1 | tail -10
```

Expected: 全部 PASS。

- [ ] **Step 2.4: 提交**

```bash
cd /Users/zac/code/github/asr/Handy
git add src-tauri/src/actions/transcribe.rs
git commit -m "Capture active window snapshot at start of recording"
```

---

### Task 3: `stop()` 中改用按 id 取回的快照

**Files:**

- Modify: `src-tauri/src/actions/transcribe.rs:944`（`let active_window_snapshot = ...`）

- [ ] **Step 3.1: 替换 stop 中的活动窗口捕获**

定位 `src-tauri/src/actions/transcribe.rs:944`：

```rust
                let active_window_snapshot = active_window::fetch_active_window().ok();
                if let Some(info) = &active_window_snapshot {
                    debug!(
                        "Active window (snapshot): app='{}' title='{}' pid={} window_id={}",
                        info.app_name, info.title, info.process_id, info.window_id
                    );
                }
```

替换为：

```rust
                // 优先使用 start() 按键瞬间捕获的快照；取不到（fetch 失败 / 路径未接入 / 已被新录音覆盖）
                // 时退回原行为。Fallback 保证不出现回归。
                // 见 docs/specs/2026-05-19-capture-active-window-at-start.spec.md
                let (active_window_snapshot, snapshot_source) =
                    match take_start_snapshot(current_transcription_id) {
                        Some(info) => (Some(info), "start"),
                        None => (active_window::fetch_active_window().ok(), "stop-fallback"),
                    };
                if let Some(info) = &active_window_snapshot {
                    debug!(
                        "Active window ({}): app='{}' title='{}' pid={} window_id={}",
                        snapshot_source, info.app_name, info.title, info.process_id, info.window_id
                    );
                } else {
                    debug!(
                        "Active window (none, source={}): id={}",
                        snapshot_source, current_transcription_id
                    );
                }
```

- [ ] **Step 3.2: 编译检查**

```bash
cd src-tauri && cargo check 2>&1 | tail -20
```

Expected: 0 errors, 0 warnings。

- [ ] **Step 3.3: 跑现有测试**

```bash
cd src-tauri && cargo test --lib actions::transcribe 2>&1 | tail -10
```

Expected: 全部 PASS。

- [ ] **Step 3.4: 提交**

```bash
cd /Users/zac/code/github/asr/Handy
git add src-tauri/src/actions/transcribe.rs
git commit -m "Consume start-time active window snapshot in stop with fetch fallback"
```

---

### Task 4: `FinishGuard::drop` 兜底清理

**Files:**

- Modify: `src-tauri/src/actions/transcribe.rs`（`FinishGuard::drop` 实现，约 line 868）

- [ ] **Step 4.1: 在 `FinishGuard::drop` 最前面加上清理**

定位 `src-tauri/src/actions/transcribe.rs:868-903`：

```rust
            impl Drop for FinishGuard {
                fn drop(&mut self) {
                    let rm = self.app.state::<Arc<AudioRecordingManager>>();
                    if rm.get_current_transcription_id() == self.transcription_id {
                        shortcut::unregister_cancel_shortcut(&self.app);
                        ...
```

将 `fn drop(&mut self) {` 之后第一行（`let rm = ...`）之前插入：

```rust
                    // 兜底清空 start() 时记录的活动窗口快照：
                    // - happy path：stop() 的 take_start_snapshot 已经拿走，本次为 no-op。
                    // - 录音过短 / panic / 异常早退：take 没有发生，这里防止 slot 长期残留。
                    // 若 slot 已被后续录音覆盖（id 不匹配），保持不动，让新 owner 自己处理。
                    clear_start_snapshot_if_matches(self.transcription_id);
```

- [ ] **Step 4.2: 编译 + 测试**

```bash
cd src-tauri && cargo check 2>&1 | tail -20
cd src-tauri && cargo test --lib actions::transcribe 2>&1 | tail -10
```

Expected: 0 errors, 0 warnings；所有测试 PASS。

- [ ] **Step 4.3: 提交**

```bash
cd /Users/zac/code/github/asr/Handy
git add src-tauri/src/actions/transcribe.rs
git commit -m "Clear start-time snapshot in FinishGuard drop"
```

---

### Task 5: 全量检查 + 手动 smoke 测试指引

**Files:** （仅校验，不改动）

- [ ] **Step 5.1: Clippy + fmt 检查**

```bash
cd src-tauri && cargo clippy --all-targets -- -D warnings 2>&1 | tail -20
cd src-tauri && cargo fmt --check 2>&1 | tail -10
```

Expected: clippy 无 warning；fmt 无 diff。若 `cargo fmt --check` 报 diff，运行 `cargo fmt` 然后用 `git add -p` 看 diff 后单独提交一次 `style: cargo fmt`。

- [ ] **Step 5.2: 全量测试**

```bash
cd src-tauri && cargo test --lib 2>&1 | tail -20
```

Expected: 全部 PASS。

- [ ] **Step 5.3: 手动 smoke 测试**

以下场景对应 spec 验收场景 1、2、3、6。请在本机跑 `bun tauri dev`，按顺序验证（前端用任意已配置的快捷键）：

1. **happy_path_switch_app_during_recording**
   - 在 Slack/iMessage 输入框获得焦点 → 按录音键
   - 录音中切到 Chrome
   - 按录音键停止
   - 期望：焦点自动切回 Slack，文本输入到 Slack
   - 在日志中能看到 `[StartSnapshot] captured id=... app='Slack'` 和 `Active window (start): app='Slack' ...`

2. **happy_path_no_switch**
   - 在 Notion 中按录音键 → 不切换 → 停止
   - 期望：内容输入到 Notion，日志看到 `Active window (start): app='Notion' ...`

3. **edge_case_too_short_recording**
   - 按录音键后立刻松开（< 500ms）
   - 期望：不发生输入；日志中能看到 `ignored/too_short`；下一次正常录音的 start 日志显示新 id，没有混入旧的快照

4. **edge_case_review_window_voice_rewrite**
   - 打开 review 窗口（任意完成一次普通转录使其出现），在 review 窗口按 voice rewrite 快捷键
   - 期望：rewrite 结果回写到 review 窗口（事件路径不变），与改动前一致

> **Scenario 4（overlapping_sessions）和 5（accessibility_denied）** 在工程环境难以稳定复现，靠代码审查 + Task 1 的单元测试保证：
>
> - Scenario 4 由 `take_start_snapshot` 的 id 校验保证（已被 Task 1.1 测试覆盖）
> - Scenario 5 由 `set_start_snapshot` 对 `None` 的处理 + stop 端 fallback 共同保证（代码路径已覆盖）

- [ ] **Step 5.4: 回填 spec 的实施偏差表**

打开 `docs/specs/2026-05-19-capture-active-window-at-start.spec.md`，在 `## 实施偏差` 段下方更新表格。若实现完全按计划执行，把表格替换为：

```markdown
| 原计划 | 实际实现 | 原因 |
| ------ | -------- | ---- |
| 无偏差 | —        | —    |
```

否则按实际差异填写（例如：发现 fmt 改了相邻行 → 记录；发现某个 helper 需要更名 → 记录）。

- [ ] **Step 5.5: 最终提交**

```bash
cd /Users/zac/code/github/asr/Handy
git add docs/specs/2026-05-19-capture-active-window-at-start.spec.md
git commit -m "Record spec deviations for active-window snapshot capture timing"
```

---

## Self-Review

**Spec coverage：**

| Spec 要求                                                          | 对应任务                                     |
| ------------------------------------------------------------------ | -------------------------------------------- |
| 存储位置：模块级 `Lazy<Mutex<Option<(u64, ActiveWindowInfo)>>>`    | Task 1.4                                     |
| 三个 helper：set / take / clear_if_matches                         | Task 1.4 + Task 1 测试                       |
| start() 中同步捕获，记 debug 日志，fetch 失败记 warn               | Task 2.1                                     |
| stop() 中按 id 取回 + fallback                                     | Task 3.1                                     |
| FinishGuard::drop 兜底清理                                         | Task 4.1                                     |
| 不改 active_window.rs / clipboard.rs / review_window.rs / managers | 所有 Task 的 Files 段                        |
| 验收场景 1 / 2 / 3 / 6（手动）                                     | Task 5.3                                     |
| 验收场景 4 / 5（代码）                                             | Task 1.1 测试 + Task 2.1 + Task 3.1 fallback |
| 实施偏差表回填                                                     | Task 5.4                                     |

**Placeholder scan：** 无 TBD / TODO / "implement later" / "appropriate error handling"。所有代码块完整。

**Type consistency：** 三个 helper 名称 `set_start_snapshot` / `take_start_snapshot` / `clear_start_snapshot_if_matches` 在 Task 1.1 测试、Task 1.4 定义、Task 2.1 / 3.1 / 4.1 调用点保持一致。`StdMutex` 别名仅在 Task 1.3、1.4 出现。
