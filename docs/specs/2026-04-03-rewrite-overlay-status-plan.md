# Rewrite Overlay Status Enhancement — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show rewrite count badge and dynamic status transitions (转录中→处理中→已改写) in the recording overlay during voice rewrite flow.

**Architecture:** Extend the existing overlay event system. Send `rewrite_count` with the recording-phase payload so the badge appears immediately. Reuse existing overlay states ("transcribing", "llm") for the post-recording phases. Add a new `rewrite-operation-complete` event for the operation flash. Delay overlay hide by 800ms in the ReviewRewrite path.

**Tech Stack:** React/TypeScript (overlay frontend), Rust/Tauri (backend events), CSS

---

### Task 1: Fix main.tsx payload parsing bug

**Files:**

- Modify: `src/overlay/main.tsx:18-31`

This is the root cause of the rewrite overlay black screen. The `show-overlay` listener casts the entire payload as `OverlayState`, but rewrite payloads are objects `{ state, rewrite_count }`.

- [ ] **Step 1: Fix the payload parser in main.tsx**

Replace the `show-overlay` listener in `OverlayApp` (the fix from earlier in this session is already applied):

```tsx
const unlistenShow = listen("show-overlay", (event) => {
  // Payload can be a plain string ("recording", "transcribing", "llm")
  // or an object { state: "rewrite", rewrite_count: N }.
  let state: OverlayState;
  if (
    event.payload !== null &&
    typeof event.payload === "object" &&
    "state" in (event.payload as object)
  ) {
    state = (event.payload as { state: OverlayState }).state;
  } else {
    state = event.payload as OverlayState;
  }
  setInitialState(state);
});
```

- [ ] **Step 2: Verify TypeScript compiles**

Run: `npx tsc --noEmit`
Expected: No errors

- [ ] **Step 3: Commit**

```bash
git add src/overlay/main.tsx
git commit -m "Fix overlay payload parsing for object payloads

show-overlay listener was casting the entire object payload as OverlayState
string, causing rewrite state to render with undefined status text (black
screen). Now correctly extracts .state from object payloads."
```

---

### Task 2: Add i18n translations for rewrite overlay states

**Files:**

- Modify: `src/i18n/locales/zh/translation.json`
- Modify: `src/i18n/locales/en/translation.json`

- [ ] **Step 1: Add zh translations**

In the `overlay.status` section of `src/i18n/locales/zh/translation.json`, change the `rewrite` key and add `rewriteOperation` entries:

```json
"status": {
  "llm": "后处理中…",
  "recording": "录音中…",
  "transcribing": "转录中…",
  "review": "审阅中…",
  "rewrite": "处理中…"
},
"rewriteOperation": {
  "rewrite": "已改写",
  "expand": "已扩充",
  "format": "已格式化",
  "translate": "已翻译",
  "polish": "已润色",
  "append": "已追加",
  "unknown": "已完成"
}
```

Note: `rewrite` status text changes from `"第{{count}}次修改"` to `"处理中…"`. The count is now shown via the badge, not in the text.

- [ ] **Step 2: Add en translations**

In the `overlay.status` section of `src/i18n/locales/en/translation.json`, update similarly. Find the existing `rewrite` key and update it, then add `rewriteOperation`:

```json
"status": {
  "llm": "Processing…",
  "recording": "Recording…",
  "transcribing": "Transcribing…",
  "review": "Reviewing…",
  "rewrite": "Processing…"
},
"rewriteOperation": {
  "rewrite": "Rewritten",
  "expand": "Expanded",
  "format": "Formatted",
  "translate": "Translated",
  "polish": "Polished",
  "append": "Appended",
  "unknown": "Done"
}
```

- [ ] **Step 3: Commit**

```bash
git add src/i18n/locales/zh/translation.json src/i18n/locales/en/translation.json
git commit -m "Add rewrite overlay operation translations (zh, en)

Change rewrite status from count text to '处理中…'. Add operation
completion texts: rewrite/expand/format/translate/polish/append."
```

---

### Task 3: Add rewrite count badge and operation-complete state to overlay frontend

**Files:**

- Modify: `src/overlay/RecordingOverlay.css`
- Modify: `src/overlay/RecordingOverlay.tsx:15,84-85,131-146,319-348,561-577,590-597`

- [ ] **Step 1: Add badge CSS**

Append to `src/overlay/RecordingOverlay.css`:

```css
/* Rewrite count badge — replaces icon in rewrite mode */
.rewrite-badge {
  width: 22px;
  height: 22px;
  border-radius: 50%;
  background: rgba(255, 255, 255, 0.18);
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

.rewrite-badge-number {
  font-size: 12px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.82);
  font-family:
    -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
  line-height: 1;
  font-variant-numeric: tabular-nums;
}
```

- [ ] **Step 2: Change rewriteCount initial value and manage it separately from reset**

In `RecordingOverlay.tsx`, change the initial value of `rewriteCount` from `1` to `0` (around line 85):

```tsx
const [rewriteCount, setRewriteCount] = useState<number>(0);
const [operationText, setOperationText] = useState<string>("");
```

In `resetOverlayRecordingState` (around line 131), add `operationText` reset but do NOT add `rewriteCount` reset — count is managed only by the `show-overlay` handler to avoid race conditions with `useEffect([initialState])`:

```tsx
setOperationText("");
```

In `setupEventListeners` (after the existing `show-overlay` listener block, around line 354), add:

```tsx
const unlistenRewriteOperation = await listen<{ operation: string }>(
  "rewrite-operation-complete",
  (event) => {
    const op = event.payload.operation || "unknown";
    setOperationText(
      t(
        `overlay.rewriteOperation.${op}`,
        t("overlay.rewriteOperation.unknown"),
      ),
    );
  },
);
if (disposed) {
  unlistenRewriteOperation();
  return;
}
unlisteners.push(unlistenRewriteOperation);
```

- [ ] **Step 3: Extract rewrite_count from all object payloads (not just "rewrite" state)**

In the `show-overlay` listener in `RecordingOverlay.tsx` (around line 319-348), change the rewrite_count extraction to apply to ALL object payloads — move it outside the `overlayState === "rewrite"` check:

Current code:

```tsx
if (overlayState === "rewrite" && richPayload.rewrite_count !== undefined) {
  setRewriteCount(richPayload.rewrite_count);
}
```

Change to (handle rewrite_count for ALL states, and reset to 0 for non-rewrite recordings):

```tsx
if (richPayload.rewrite_count !== undefined) {
  setRewriteCount(richPayload.rewrite_count);
} else if (overlayState === "recording") {
  setRewriteCount(0);
}
```

This allows the "recording" state payload to carry rewrite_count (for rewrite mode), and explicitly resets the count when a normal recording starts (plain string "recording" payload has no rewrite_count). The count is NOT reset in `resetOverlayRecordingState` to avoid the `useEffect([initialState])` race that would clear it after the direct handler sets it.

- [ ] **Step 4: Update getIcon to show badge in rewrite mode**

Replace the `getIcon` function (around line 561):

```tsx
const isRewriteMode = rewriteCount > 0;

const getIcon = () => {
  if (Boolean(errorText) && state !== "recording") {
    return <CancelIcon color="var(--ruby-9)" />;
  }
  if (isRewriteMode) {
    return (
      <Box className="rewrite-badge">
        <Text className="rewrite-badge-number">{rewriteCount}</Text>
      </Box>
    );
  }
  if (state === "recording") {
    return <MicrophoneIcon color={accentColor} />;
  } else {
    return <TranscriptionIcon color={accentColor} />;
  }
};
```

- [ ] **Step 5: Update statusTextMap — remove count interpolation from rewrite**

Change the `statusTextMap` (around line 572):

```tsx
const statusTextMap: Record<OverlayState, string> = {
  recording: t("overlay.status.recording"),
  transcribing: t("overlay.status.transcribing"),
  llm: t("overlay.status.llm"),
  rewrite: t("overlay.status.rewrite"),
};
```

Remove `{ count: rewriteCount }` since rewrite text is now "处理中…" (no interpolation).

- [ ] **Step 6: Show operationText when available (completion flash)**

In the render section, modify the non-recording status text block (around line 647-658). Replace:

```tsx
{
  !showRealtimeText && state !== "recording" && !skillConfirmation && (
    <Flex direction="column" className="status-text" align="center">
      {!showErrorText && (
        <Text>{chainedPromptName || statusTextMap[state]}</Text>
      )}
      {showErrorText && (
        <Text style={{ color: "var(--ruby-9)", fontWeight: "bold" }}>
          {errorText}
        </Text>
      )}
    </Flex>
  );
}
```

With:

```tsx
{
  !showRealtimeText && state !== "recording" && !skillConfirmation && (
    <Flex direction="column" className="status-text" align="center">
      {!showErrorText && (
        <Text>
          {operationText || chainedPromptName || statusTextMap[state]}
        </Text>
      )}
      {showErrorText && (
        <Text style={{ color: "var(--ruby-9)", fontWeight: "bold" }}>
          {errorText}
        </Text>
      )}
    </Flex>
  );
}
```

`operationText` takes priority when set (the brief completion flash).

- [ ] **Step 7: Verify TypeScript compiles**

Run: `npx tsc --noEmit`
Expected: No errors

- [ ] **Step 8: Commit**

```bash
git add src/overlay/RecordingOverlay.tsx src/overlay/RecordingOverlay.css
git commit -m "Add rewrite count badge and operation-complete display to overlay

- Show gray circular badge with rewrite count number instead of icon
- Listen for rewrite-operation-complete event to flash operation text
- Extract rewrite_count from any object payload (not just rewrite state)
- operationText takes display priority for the completion flash"
```

---

### Task 4: Backend — send rewrite_count during recording and use transcribing state in stop

**Files:**

- Modify: `src-tauri/src/overlay.rs:387-396,398-417`
- Modify: `src-tauri/src/actions/transcribe.rs:200,236-238,344-350`

- [ ] **Step 1: Generalize the rewrite overlay emit function in overlay.rs**

Rename `emit_rewrite_overlay_state_with_retry` to be more general, and update `RewriteOverlayPayload` to allow any state. Replace the existing `emit_rewrite_overlay_state_with_retry` and `RewriteOverlayPayload` (around line 398-417):

```rust
#[derive(Clone, serde::Serialize)]
struct OverlayStateWithCount {
    state: &'static str,
    rewrite_count: u32,
}

fn emit_overlay_state_with_count_and_retry(
    overlay_window: tauri::WebviewWindow,
    state: &'static str,
    rewrite_count: u32,
) {
    let payload = OverlayStateWithCount {
        state,
        rewrite_count,
    };
    let _ = overlay_window.emit("show-overlay", payload.clone());

    std::thread::spawn(move || {
        for delay_ms in [40_u64, 120_u64] {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            let _ = overlay_window.emit("show-overlay", payload.clone());
        }
    });
}
```

Update `show_rewrite_overlay` to use the new function (around line 339-363):

```rust
pub fn show_rewrite_overlay(app_handle: &AppHandle, rewrite_count: u32) {
    let settings = settings::get_settings(app_handle);
    if settings.overlay_position == OverlayPosition::None {
        return;
    }

    let refocus_target = focused_votype_window_label(app_handle);

    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        if !overlay_window.is_visible().unwrap_or(false) {
            update_overlay_position(app_handle);
        }

        let _ = overlay_window.set_ignore_cursor_events(true);
        let _ = overlay_window.show();

        #[cfg(target_os = "windows")]
        force_overlay_topmost(&overlay_window);

        emit_overlay_state_with_count_and_retry(overlay_window, "rewrite", rewrite_count);
        restore_votype_focus_after_overlay_show(app_handle, refocus_target);
    }
}
```

- [ ] **Step 2: Add show_recording_overlay_rewrite function**

Add a new public function in `overlay.rs` right after `show_recording_overlay` (after line 308):

```rust
/// Shows the recording overlay with rewrite count badge
pub fn show_recording_overlay_rewrite(app_handle: &AppHandle, rewrite_count: u32) {
    let settings = settings::get_settings(app_handle);
    if settings.overlay_position == OverlayPosition::None {
        return;
    }

    let refocus_target = focused_votype_window_label(app_handle);

    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        if let Some((x, y)) = calculate_overlay_position(app_handle) {
            let _ = overlay_window
                .set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
        }

        let _ = overlay_window.set_ignore_cursor_events(true);
        let _ = overlay_window.show();

        #[cfg(target_os = "windows")]
        force_overlay_topmost(&overlay_window);

        emit_overlay_state_with_count_and_retry(overlay_window, "recording", rewrite_count);
        restore_votype_focus_after_overlay_show(app_handle, refocus_target);
    }
}
```

- [ ] **Step 3: Move rewrite_count increment to start() in transcribe.rs**

In `start()` (around line 200), rename `_shortcut_str` to `shortcut_str` and add the import:

```rust
fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
```

Replace the `show_recording_overlay(app)` call (around line 238) with:

```rust
if shortcut_str == "review-window-local" {
    let rewrite_count = crate::review_window::increment_rewrite_count();
    show_recording_overlay_rewrite(app, rewrite_count);
} else {
    show_recording_overlay(app);
}
```

Add the import at the top of the file (around line 9):

```rust
use crate::overlay::{show_recording_overlay, show_recording_overlay_rewrite, show_rewrite_overlay, show_transcribing_overlay};
```

- [ ] **Step 4: Change stop() to use show_transcribing_overlay for rewrite mode**

In `stop()` (around line 344-350), replace:

```rust
if shortcut_str == "review-window-local" {
    let rewrite_count = crate::review_window::increment_rewrite_count();
    show_rewrite_overlay(app, rewrite_count);
} else {
    show_transcribing_overlay(app);
}
```

With:

```rust
// Rewrite count was already incremented in start()
show_transcribing_overlay(app);
```

Now both rewrite and normal flows show "转录中…" after recording stops. The badge persists because the frontend's `rewriteCount` state was set during recording and isn't reset by "transcribing" state.

- [ ] **Step 5: Make increment_rewrite_count idempotent for the stop path**

Since `increment_rewrite_count` is no longer called from `stop()`, verify there are no other callers that depend on the old behavior. Search for all usages:

Run: `grep -rn "increment_rewrite_count" src-tauri/src/`

Confirm it's only called from `start()` now. If there are other callers, evaluate if they need adjustment.

- [ ] **Step 6: Verify Rust compiles**

Run: `cargo check --manifest-path src-tauri/Cargo.toml 2>&1 | tail -5`
Expected: `Finished` with no new errors

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/overlay.rs src-tauri/src/actions/transcribe.rs
git commit -m "Send rewrite_count during recording phase, show transcribing after stop

- Add show_recording_overlay_rewrite() that sends count with recording state
- Move increment_rewrite_count from stop() to start() so badge shows immediately
- stop() now uses show_transcribing_overlay for both rewrite and normal flows
- Generalize overlay state+count emit function for reuse"
```

---

### Task 5: Backend — emit LLM processing state and operation-complete from rewrite pipeline

**Files:**

- Modify: `src-tauri/src/actions/post_process/pipeline.rs:819-966`
- Modify: `src-tauri/src/actions/transcribe.rs:2176-2197`

- [ ] **Step 1: Emit "rewrite" state and operation-complete from execute_votype_rewrite_prompt**

In `pipeline.rs`, inside `execute_votype_rewrite_prompt` (around line 898), add overlay state transition BEFORE the LLM call. Use a direct emit of `"rewrite"` state (not `show_llm_processing_overlay` which emits `"llm"` → "后处理中…"). The `"rewrite"` state now maps to "处理中…" in i18n:

```rust
// Switch overlay to "处理中…" state for rewrite LLM processing
if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
    let _ = overlay_window.emit("show-overlay", "rewrite");
}

let rewrite_start = std::time::Instant::now();
```

After the successful parse of the operation (around line 932-965), emit the operation-complete event. Insert right before the `return` inside the `if let Some(parsed)` block (around line 953):

```rust
// Emit operation completion for overlay flash
if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
    #[derive(Clone, serde::Serialize)]
    struct OperationComplete {
        operation: String,
    }
    let _ = overlay_window.emit(
        "rewrite-operation-complete",
        OperationComplete {
            operation: parsed.operation.clone(),
        },
    );
}
```

Also emit for the fallback path (around line 969-980, the non-parsed return), emit with "unknown":

```rust
// Emit fallback operation completion
if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
    #[derive(Clone, serde::Serialize)]
    struct OperationComplete {
        operation: String,
    }
    let _ = overlay_window.emit(
        "rewrite-operation-complete",
        OperationComplete {
            operation: "unknown".to_string(),
        },
    );
}
```

- [ ] **Step 2: Delay overlay hide in ReviewRewrite path of transcribe.rs**

In `transcribe.rs`, modify the `ReviewRewrite` handler (around line 2176-2197). Replace:

```rust
if matches!(votype_mode, VotypeInputMode::ReviewRewrite) {
    utils::hide_recording_overlay(&ah_clone);
    change_tray_icon(&ah_clone, TrayIconState::Idle);
```

With:

```rust
if matches!(votype_mode, VotypeInputMode::ReviewRewrite) {
    change_tray_icon(&ah_clone, TrayIconState::Idle);
```

And replace the early `return;` at the end of the block with a delayed hide:

```rust
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
```

The review editor content is updated immediately, but the overlay stays visible for 800ms showing the operation text.

- [ ] **Step 3: Verify Rust compiles**

Run: `cargo check --manifest-path src-tauri/Cargo.toml 2>&1 | tail -5`
Expected: `Finished` with no new errors

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/actions/post_process/pipeline.rs src-tauri/src/actions/transcribe.rs
git commit -m "Emit rewrite processing state and operation-complete from pipeline

- Emit 'rewrite' overlay state before LLM call in execute_votype_rewrite_prompt
- Emit rewrite-operation-complete event with operation type after LLM returns
- Delay overlay hide by 800ms in ReviewRewrite path for operation flash
- Review editor content updates immediately, overlay persists briefly"
```

---

### Task 6: Verify complete flow end-to-end

- [ ] **Step 1: Build the full app**

Run: `cargo check --manifest-path src-tauri/Cargo.toml && npx tsc --noEmit`
Expected: Both pass

- [ ] **Step 2: Manual verification checklist**

Test against spec acceptance scenarios:

1. **Normal recording (non-review):** Press record → verify mic icon shows (no badge) → stop → verify transcribing/LLM text → verify overlay hides normally
2. **First rewrite:** Open review window → press record → verify badge shows "1" during recording → stop → verify "转录中…" with badge → verify "处理中…" with badge → verify operation flash (e.g. "已改写") → overlay hides after ~800ms
3. **Second rewrite:** Press record again in review window → verify badge shows "2" → same flow
4. **Close and reopen review window:** Verify count resets to 1 on next rewrite

- [ ] **Step 3: Commit any fixes from testing**

If any adjustments are needed from manual testing, commit them separately.
