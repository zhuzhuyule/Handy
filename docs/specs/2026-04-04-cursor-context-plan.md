# Cursor Context Injection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Capture text surrounding the cursor in the active editor via macOS Accessibility API and inject it as `[cursor-context]` into the post-processing prompt, improving LLM disambiguation accuracy.

**Architecture:** New `CursorContext` struct + `get_cursor_context()` function in `clipboard.rs` using AXValue/AXSelectedTextRange. Passed through `transcribe.rs` → `pipeline.rs` → `PromptBuilder` where it renders as a `[cursor-context]` section in the user message. Mutually exclusive with `selected_text`.

**Tech Stack:** Rust, macOS Accessibility API (Core Foundation), Tauri

---

### Task 1: CursorContext struct and boundary truncation helpers

**Files:**

- Modify: `src-tauri/src/clipboard.rs`

- [ ] **Step 1: Write the CursorContext struct and truncation helper tests**

Add at the bottom of `clipboard.rs` inside the existing `#[cfg(test)] mod tests` block:

```rust
#[test]
fn test_truncate_before_at_sentence_boundary() {
    let text = "First sentence. Second sentence. Third sentence here.";
    let result = truncate_before(text, 30);
    // Should cut at the period after "Second sentence"
    assert_eq!(result, "Second sentence. Third sentence here.");
}

#[test]
fn test_truncate_before_no_boundary() {
    let text = "abcdefghijklmnopqrstuvwxyz";
    let result = truncate_before(text, 10);
    assert_eq!(result.chars().count(), 10);
}

#[test]
fn test_truncate_after_at_sentence_boundary() {
    let text = "First sentence. Second sentence. Third.";
    let result = truncate_after(text, 20);
    assert_eq!(result, "First sentence.");
}

#[test]
fn test_truncate_after_no_boundary() {
    let text = "abcdefghijklmnopqrstuvwxyz";
    let result = truncate_after(text, 10);
    assert_eq!(result.chars().count(), 10);
}

#[test]
fn test_truncate_before_empty() {
    assert_eq!(truncate_before("", 300), "");
}

#[test]
fn test_truncate_after_empty() {
    assert_eq!(truncate_after("", 100), "");
}

#[test]
fn test_truncate_before_paragraph_boundary() {
    let text = "Line one.\nLine two.\nLine three.";
    let result = truncate_before(text, 15);
    // Should find the newline boundary
    assert!(result.starts_with("Line two.") || result.starts_with("Line three."));
}

#[test]
fn test_truncate_before_shorter_than_limit() {
    let text = "Short text.";
    let result = truncate_before(text, 300);
    assert_eq!(result, "Short text.");
}

#[test]
fn test_truncate_after_shorter_than_limit() {
    let text = "Short text.";
    let result = truncate_after(text, 100);
    assert_eq!(result, "Short text.");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src-tauri && cargo test --lib clipboard::tests::test_truncate -- 2>&1 | head -30`
Expected: compilation errors — `truncate_before` and `truncate_after` not defined.

- [ ] **Step 3: Implement CursorContext and truncation helpers**

Add the `CursorContext` struct and helpers above the existing `#[cfg(test)]` block in `clipboard.rs`:

```rust
/// Text surrounding the cursor position in the active editor.
#[derive(Debug, Clone)]
pub struct CursorContext {
    /// Text before the cursor (up to 300 chars, truncated at sentence/paragraph boundary).
    pub before: String,
    /// Text after the cursor (up to 100 chars, truncated at sentence/paragraph boundary).
    pub after: String,
}

const CURSOR_CONTEXT_BEFORE_LIMIT: usize = 300;
const CURSOR_CONTEXT_AFTER_LIMIT: usize = 100;
const AX_VALUE_MAX_LENGTH: usize = 100_000;

/// Truncate text to at most `limit` characters from the END, preferring to cut
/// at sentence/paragraph boundaries (newline, period, exclamation, question mark).
fn truncate_before(text: &str, limit: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return text.to_string();
    }

    let start = chars.len() - limit;
    let slice = &chars[start..];

    // Look for a boundary in the first 30% of the slice to avoid cutting too much
    let search_range = (limit * 3) / 10;
    let boundary_chars = ['\n', '。', '！', '？', '.', '!', '?'];

    for i in 0..search_range.min(slice.len()) {
        if boundary_chars.contains(&slice[i]) {
            // Start after the boundary character (skip whitespace too)
            let mut j = i + 1;
            while j < slice.len() && slice[j].is_whitespace() {
                j += 1;
            }
            if j < slice.len() {
                return slice[j..].iter().collect();
            }
        }
    }

    // No sentence boundary found — try space/CJK boundary
    for i in 0..search_range.min(slice.len()) {
        if slice[i].is_whitespace() {
            let mut j = i + 1;
            while j < slice.len() && slice[j].is_whitespace() {
                j += 1;
            }
            if j < slice.len() {
                return slice[j..].iter().collect();
            }
        }
    }

    // Hard truncate
    slice.iter().collect()
}

/// Truncate text to at most `limit` characters from the START, preferring to cut
/// at sentence/paragraph boundaries.
fn truncate_after(text: &str, limit: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return text.to_string();
    }

    let slice = &chars[..limit];
    let boundary_chars = ['\n', '。', '！', '？', '.', '!', '?'];

    // Search from the end of the slice backwards, within last 30%
    let search_start = limit - (limit * 3) / 10;
    for i in (search_start..slice.len()).rev() {
        if boundary_chars.contains(&slice[i]) {
            return slice[..=i].iter().collect();
        }
    }

    // No sentence boundary — try space
    for i in (search_start..slice.len()).rev() {
        if slice[i].is_whitespace() {
            return slice[..i].iter().collect();
        }
    }

    // Hard truncate
    slice.iter().collect()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd src-tauri && cargo test --lib clipboard::tests::test_truncate`
Expected: all truncation tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/clipboard.rs
git commit -m "Add CursorContext struct and boundary truncation helpers"
```

---

### Task 2: macOS Accessibility API cursor context retrieval

**Files:**

- Modify: `src-tauri/src/clipboard.rs`

- [ ] **Step 1: Implement get_cursor_context_via_accessibility for macOS**

Add after the existing `get_selected_text_via_accessibility()` function in `clipboard.rs` (after line 380):

```rust
/// Get text surrounding the cursor in the active text field using macOS Accessibility API.
/// Returns CursorContext with before/after text, truncated at sentence boundaries.
/// Fails silently if the focused element doesn't support AXValue or AXSelectedTextRange.
#[cfg(target_os = "macos")]
fn get_cursor_context_via_accessibility() -> Result<CursorContext, String> {
    use core_foundation::base::{CFRelease, TCFType};
    use core_foundation::number::CFNumber;
    use core_foundation::string::{CFString, CFStringRef};
    use std::ffi::c_void;
    use std::ptr;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateSystemWide() -> *mut c_void;
        fn AXUIElementCopyAttributeValue(
            element: *mut c_void,
            attribute: CFStringRef,
            value: *mut *mut c_void,
        ) -> i32;
    }

    // AXValueRef helpers for extracting CFRange
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct CFRange {
        location: i64,
        length: i64,
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXValueGetValue(
            value: *mut c_void,
            value_type: u32,
            value_ptr: *mut c_void,
        ) -> bool;
    }

    // AXValueType::CGRange = 4 (kAXValueCFRangeType)
    const K_AX_VALUE_CF_RANGE_TYPE: u32 = 4;

    unsafe {
        let system_element = AXUIElementCreateSystemWide();
        if system_element.is_null() {
            return Err("Failed to create system-wide AXUIElement".to_string());
        }

        // Get the focused UI element
        let focused_attr = CFString::new("AXFocusedUIElement");
        let mut focused_element: *mut c_void = ptr::null_mut();
        let result = AXUIElementCopyAttributeValue(
            system_element,
            focused_attr.as_concrete_TypeRef(),
            &mut focused_element,
        );

        if result != 0 || focused_element.is_null() {
            CFRelease(system_element);
            return Err("Failed to get focused UI element".to_string());
        }

        // Get AXValue (full text content)
        let value_attr = CFString::new("AXValue");
        let mut value_ref: *mut c_void = ptr::null_mut();
        let result = AXUIElementCopyAttributeValue(
            focused_element,
            value_attr.as_concrete_TypeRef(),
            &mut value_ref,
        );

        if result != 0 || value_ref.is_null() {
            CFRelease(focused_element);
            CFRelease(system_element);
            return Err("Failed to get AXValue from focused element".to_string());
        }

        let cf_value = CFString::wrap_under_create_rule(value_ref as CFStringRef);
        let full_text = cf_value.to_string();

        // Length guard
        if full_text.len() > AX_VALUE_MAX_LENGTH {
            CFRelease(focused_element);
            CFRelease(system_element);
            return Err(format!(
                "AXValue too large ({} chars, limit {})",
                full_text.len(),
                AX_VALUE_MAX_LENGTH
            ));
        }

        // Get AXSelectedTextRange → CFRange { location, length }
        let range_attr = CFString::new("AXSelectedTextRange");
        let mut range_ref: *mut c_void = ptr::null_mut();
        let result = AXUIElementCopyAttributeValue(
            focused_element,
            range_attr.as_concrete_TypeRef(),
            &mut range_ref,
        );

        CFRelease(focused_element);
        CFRelease(system_element);

        if result != 0 || range_ref.is_null() {
            return Err("Failed to get AXSelectedTextRange".to_string());
        }

        let mut range = CFRange {
            location: 0,
            length: 0,
        };
        let ok = AXValueGetValue(
            range_ref,
            K_AX_VALUE_CF_RANGE_TYPE,
            &mut range as *mut CFRange as *mut c_void,
        );
        CFRelease(range_ref);

        if !ok || range.location < 0 {
            return Err("Failed to extract CFRange from AXSelectedTextRange".to_string());
        }

        let cursor_pos = range.location as usize;
        let chars: Vec<char> = full_text.chars().collect();

        if cursor_pos > chars.len() {
            return Err(format!(
                "Cursor position {} exceeds text length {}",
                cursor_pos,
                chars.len()
            ));
        }

        let before_text: String = chars[..cursor_pos].iter().collect();
        let after_text: String = chars[cursor_pos..].iter().collect();

        let before = truncate_before(&before_text, CURSOR_CONTEXT_BEFORE_LIMIT);
        let after = truncate_after(&after_text, CURSOR_CONTEXT_AFTER_LIMIT);

        if before.is_empty() && after.is_empty() {
            return Err("Cursor context is empty".to_string());
        }

        Ok(CursorContext { before, after })
    }
}
```

- [ ] **Step 2: Add the public get_cursor_context function**

Add after the `get_cursor_context_via_accessibility` function:

```rust
/// Get cursor context from the active application.
/// On macOS, uses Accessibility API. On other platforms, returns Err (not supported).
pub fn get_cursor_context(_app_handle: &AppHandle) -> Result<CursorContext, String> {
    #[cfg(target_os = "macos")]
    {
        match get_cursor_context_via_accessibility() {
            Ok(ctx) => {
                info!(
                    "Cursor context acquired: before={}chars, after={}chars",
                    ctx.before.chars().count(),
                    ctx.after.chars().count()
                );
                Ok(ctx)
            }
            Err(e) => {
                log::debug!("Failed to get cursor context via accessibility: {}", e);
                Err(e)
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Cursor context not supported on this platform".to_string())
    }
}
```

- [ ] **Step 3: Verify compilation**

Run: `cd src-tauri && cargo check 2>&1 | tail -5`
Expected: no errors (we can't unit-test AX API calls without a running UI).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/clipboard.rs
git commit -m "Add macOS Accessibility API cursor context retrieval"
```

---

### Task 3: Capture cursor context at transcription time

**Files:**

- Modify: `src-tauri/src/actions/transcribe.rs`

- [ ] **Step 1: Add cursor context capture after selected_text**

In `transcribe.rs`, find the line (approximately line 443):

```rust
let selected_text = crate::clipboard::get_selected_text(&ah).ok();
```

Add immediately after it:

```rust
// Cursor context: mutually exclusive with selected_text
let cursor_context = if selected_text.is_some() {
    None
} else {
    crate::clipboard::get_cursor_context(&ah).ok()
};
```

- [ ] **Step 2: Pass cursor_context into unified_post_process**

Find the call to `unified_post_process` (approximately line 1744). The current call ends with:

```rust
                                    selected_text.clone(),
                                    review_document_text.clone(),
                                )
```

Change to:

```rust
                                    selected_text.clone(),
                                    review_document_text.clone(),
                                    cursor_context.clone(),
                                )
```

- [ ] **Step 3: Verify compilation fails with expected error**

Run: `cd src-tauri && cargo check 2>&1 | head -20`
Expected: Error about `unified_post_process` argument count mismatch — this is correct because we haven't updated the signature yet.

- [ ] **Step 4: Commit (WIP — will compile after Task 4)**

```bash
git add src-tauri/src/actions/transcribe.rs
git commit -m "WIP: Capture cursor context at transcription time"
```

---

### Task 4: Thread cursor_context through pipeline.rs

**Files:**

- Modify: `src-tauri/src/actions/post_process/pipeline.rs`

- [ ] **Step 1: Add cursor_context parameter to unified_post_process**

In `pipeline.rs`, update the `unified_post_process` function signature. After the existing parameter:

```rust
    review_document_text: Option<String>,
```

Add:

```rust
    cursor_context: Option<crate::clipboard::CursorContext>,
```

- [ ] **Step 2: Pass cursor_context to PromptBuilder in the LitePolish path**

In the LitePolish section (around line 347), find:

```rust
        let built = super::prompt_builder::PromptBuilder::new(&lite_prompt, transcription)
            .app_name(app_name.as_deref())
            .window_title(window_title.as_deref())
            .hotword_injection(hotword_injection)
            .session_context(session_ctx)
            .app_language(&lite_settings.app_language)
            .injection_policy(super::prompt_builder::InjectionPolicy::for_post_process(
                &lite_settings,
            ))
            .build();
```

Change to:

```rust
        let built = super::prompt_builder::PromptBuilder::new(&lite_prompt, transcription)
            .app_name(app_name.as_deref())
            .window_title(window_title.as_deref())
            .hotword_injection(hotword_injection)
            .cursor_context(cursor_context.as_ref())
            .session_context(session_ctx)
            .app_language(&lite_settings.app_language)
            .injection_policy(super::prompt_builder::InjectionPolicy::for_post_process(
                &lite_settings,
            ))
            .build();
```

- [ ] **Step 3: Pass cursor_context to PromptBuilder in the FullPolish path (maybe_post_process_transcription)**

In the `maybe_post_process_transcription` function, find the PromptBuilder construction (around line 2042):

```rust
        let mut builder = super::prompt_builder::PromptBuilder::new(&prompt, transcription_content)
            .streaming_transcription(streaming_transcription)
            .selected_text(selected_text.as_deref())
            .app_name(app_name.as_deref())
```

This function doesn't currently receive `cursor_context`. Add it as a parameter:

After `review_document_text: Option<String>,` in the `maybe_post_process_transcription` signature (line 1083), add:

```rust
    cursor_context: Option<crate::clipboard::CursorContext>,
```

Then add `.cursor_context(cursor_context.as_ref())` to the builder chain (around line 2042):

```rust
        let mut builder = super::prompt_builder::PromptBuilder::new(&prompt, transcription_content)
            .streaming_transcription(streaming_transcription)
            .selected_text(selected_text.as_deref())
            .cursor_context(cursor_context.as_ref())
            .app_name(app_name.as_deref())
```

- [ ] **Step 4: Update the call to maybe_post_process_transcription in unified_post_process**

Find the call to `maybe_post_process_transcription` in `unified_post_process` (around line 663). After `review_document_text,` add `cursor_context,`:

```rust
    ) = maybe_post_process_transcription(
        app_handle,
        settings_ref,
        transcription,
        streaming_transcription,
        show_overlay,
        override_prompt_id,
        app_name,
        window_title,
        match_pattern,
        match_type,
        history_id,
        skill_mode,
        review_editor_active,
        selected_text,
        review_document_text,
        cursor_context,
        true, // skip_smart_routing: already done by unified_post_process
    )
    .await;
```

- [ ] **Step 5: Verify compilation fails with expected error about PromptBuilder::cursor_context**

Run: `cd src-tauri && cargo check 2>&1 | head -20`
Expected: Error about `cursor_context` method not found on PromptBuilder — this is correct because we haven't added it yet.

- [ ] **Step 6: Commit (WIP)**

```bash
git add src-tauri/src/actions/post_process/pipeline.rs
git commit -m "WIP: Thread cursor_context through pipeline"
```

---

### Task 5: Add cursor_context to PromptBuilder

**Files:**

- Modify: `src-tauri/src/actions/post_process/prompt_builder.rs`

- [ ] **Step 1: Write tests for cursor context rendering**

Add to the existing `#[cfg(test)] mod tests` block at the end of `prompt_builder.rs`:

```rust
#[test]
fn test_cursor_context_renders_both_sections() {
    let prompt = make_prompt("# Expert\nProcess input.");

    let ctx = crate::clipboard::CursorContext {
        before: "I want to confirm".to_string(),
        after: "please reply".to_string(),
    };
    let built = PromptBuilder::new(&prompt, "the attendee list")
        .cursor_context(Some(&ctx))
        .build();

    let input = built.user_message.unwrap();
    assert!(input.contains("[cursor-context]"));
    assert!(input.contains("--- before cursor ---"));
    assert!(input.contains("I want to confirm"));
    assert!(input.contains("--- after cursor ---"));
    assert!(input.contains("please reply"));
    assert!(input.ends_with("[input-text]\nthe attendee list"));
}

#[test]
fn test_cursor_context_only_before() {
    let prompt = make_prompt("# Expert\nProcess input.");

    let ctx = crate::clipboard::CursorContext {
        before: "Some text before cursor".to_string(),
        after: String::new(),
    };
    let built = PromptBuilder::new(&prompt, "hello")
        .cursor_context(Some(&ctx))
        .build();

    let input = built.user_message.unwrap();
    assert!(input.contains("[cursor-context]"));
    assert!(input.contains("--- before cursor ---"));
    assert!(input.contains("Some text before cursor"));
    assert!(!input.contains("--- after cursor ---"));
}

#[test]
fn test_cursor_context_not_in_protocol_when_absent() {
    let prompt = make_prompt("# Expert\nProcess input.");

    let built = PromptBuilder::new(&prompt, "hello").build();

    let sys = built.system_prompt;
    assert!(!sys.contains("cursor-context"));
}

#[test]
fn test_cursor_context_in_protocol_when_present() {
    let prompt = make_prompt("# Expert\nProcess input.");

    let ctx = crate::clipboard::CursorContext {
        before: "context".to_string(),
        after: String::new(),
    };
    let built = PromptBuilder::new(&prompt, "hello")
        .cursor_context(Some(&ctx))
        .build();

    let sys = built.system_prompt;
    assert!(sys.contains("cursor-context"));
    assert!(sys.contains("disambiguation"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd src-tauri && cargo test --lib post_process::prompt_builder::tests::test_cursor_context 2>&1 | head -20`
Expected: compilation errors — `cursor_context` method not found.

- [ ] **Step 3: Add CursorContext FieldTag variant**

In `prompt_builder.rs`, add `CursorContext` to the `FieldTag` enum (after `SelectedText`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FieldTag {
    Instruction,
    SelectedText,
    CursorContext,
    PersonNames,
    ProductNames,
    DomainTerms,
    Hotwords,
    HistoryHints,
    AsrReference,
    AsrCorrections,
    InputText,
}
```

Add the description and placeholder in the `impl FieldTag` block:

```rust
FieldTag::CursorContext => "cursor-context: text surrounding the cursor in the active editor — use only for disambiguation and contextual understanding, do not copy or reference this text in output",
```

```rust
FieldTag::CursorContext => "{{cursor-context}}",
```

- [ ] **Step 4: Add cursor_context field and builder method to PromptBuilder**

Add to the `PromptBuilder` struct (after `selected_text`):

```rust
    /// Text surrounding the cursor in the active editor (macOS Accessibility API).
    cursor_context: Option<&'a crate::clipboard::CursorContext>,
```

Initialize in `new()`:

```rust
            cursor_context: None,
```

Add the builder method (after `selected_text` method):

```rust
    pub fn cursor_context(mut self, ctx: Option<&'a crate::clipboard::CursorContext>) -> Self {
        self.cursor_context = ctx.filter(|c| !c.before.is_empty() || !c.after.is_empty());
        self
    }
```

- [ ] **Step 5: Add cursor context rendering function**

Add after `render_session_context_block`:

```rust
fn render_cursor_context_block(ctx: &crate::clipboard::CursorContext) -> Option<String> {
    if ctx.before.is_empty() && ctx.after.is_empty() {
        return None;
    }
    let mut parts = vec!["[cursor-context]".to_string()];
    if !ctx.before.is_empty() {
        parts.push("--- before cursor ---".to_string());
        parts.push(ctx.before.clone());
    }
    if !ctx.after.is_empty() {
        parts.push("--- after cursor ---".to_string());
        parts.push(ctx.after.clone());
    }
    Some(parts.join("\n"))
}
```

- [ ] **Step 6: Wire cursor context into the build() method**

In the `build()` method, add `CursorContext` to `present_fields` (after the `SelectedText` block, around line 568):

```rust
        if self.cursor_context.is_some() {
            present_fields.push(FieldTag::CursorContext);
        }
```

Add the rendering in the sections assembly (after `selected_text` rendering, around line 681):

```rust
        if let Some(ctx) = self.cursor_context {
            if !explicit_field_references.contains(&FieldTag::CursorContext) {
                if let Some(block) = render_cursor_context_block(ctx) {
                    sections.push(block);
                }
            }
        }
```

Also add `CursorContext` to the explicit placeholder replacement block (around line 601):

```rust
                FieldTag::CursorContext => self
                    .cursor_context
                    .and_then(|ctx| {
                        let mut parts = Vec::new();
                        if !ctx.before.is_empty() {
                            parts.push(format!("--- before cursor ---\n{}", ctx.before));
                        }
                        if !ctx.after.is_empty() {
                            parts.push(format!("--- after cursor ---\n{}", ctx.after));
                        }
                        if parts.is_empty() { None } else { Some(parts.join("\n")) }
                    }),
```

- [ ] **Step 7: Add cursor-context rule to Input Protocol**

In `build_input_protocol_note`, add after the `asr-corrections` rule (around line 95):

```rust
    if fields.iter().any(|f| *f == FieldTag::CursorContext) {
        rules.push(
            "- cursor-context shows text around the user's cursor position in the active editor; use it only for disambiguation and understanding the user's writing context — do not copy, quote, or reference this text in your output".to_string(),
        );
    }
```

- [ ] **Step 8: Add [cursor-context] to sanitize_history_entry filter**

In `sanitize_history_entry` (around line 228), add a filter line:

```rust
                && !trimmed.starts_with("[cursor-context]")
```

- [ ] **Step 9: Run tests**

Run: `cd src-tauri && cargo test --lib post_process::prompt_builder::tests 2>&1 | tail -20`
Expected: all tests PASS including the new cursor context tests.

- [ ] **Step 10: Commit**

```bash
git add src-tauri/src/actions/post_process/prompt_builder.rs
git commit -m "Add cursor context support to PromptBuilder with field tag and rendering"
```

---

### Task 6: Full compilation and integration verification

**Files:**

- All previously modified files

- [ ] **Step 1: Verify full project compilation**

Run: `cd src-tauri && cargo check 2>&1 | tail -10`
Expected: clean compilation with no errors. If there are unused import warnings from `CursorContext`, that's expected and acceptable.

- [ ] **Step 2: Fix any compilation issues**

If there are errors, fix them. Common issues:

- Missing `use` for `CursorContext` in pipeline.rs — add `use crate::clipboard::CursorContext;` if needed
- Parameter ordering mismatches — ensure `cursor_context` is passed in the right position in all call sites

- [ ] **Step 3: Run all existing tests**

Run: `cd src-tauri && cargo test 2>&1 | tail -20`
Expected: all existing tests still pass (our changes are additive — no existing behavior changed).

- [ ] **Step 4: Verify no compiler warnings in changed files**

Run: `cd src-tauri && cargo check 2>&1 | grep -E "warning|error"`
Expected: no warnings in clipboard.rs, prompt_builder.rs, pipeline.rs, or transcribe.rs. Fix any that appear.

- [ ] **Step 5: Commit if any fixes were needed**

```bash
git add -A
git commit -m "Fix compilation and resolve warnings for cursor context integration"
```

---

### Task 7: Pipeline decision logging

**Files:**

- Modify: `src-tauri/src/actions/post_process/pipeline.rs`

- [ ] **Step 1: Add has_cursor_context to pipeline decision record**

Find where `decision` is initialized in `unified_post_process` (around line 53):

```rust
    let mut decision = crate::managers::pipeline_log::PipelineDecisionRecord {
        input_length: char_count,
        app_name: app_name.clone(),
        smart_routing_enabled,
        history_id,
        ..Default::default()
    };
```

After this initialization, add:

```rust
    decision.has_cursor_context = cursor_context.is_some();
```

- [ ] **Step 2: Add has_cursor_context field to PipelineDecisionRecord**

Check if `PipelineDecisionRecord` has a generic metadata/extras field or needs a new bool. Find the struct definition:

Run: `grep -n "struct PipelineDecisionRecord" src-tauri/src/managers/pipeline_log.rs`

If the struct doesn't have a `has_cursor_context` field, add one:

```rust
    pub has_cursor_context: bool,
```

And ensure the `Default` impl sets it to `false`.

If the struct uses a JSON extras field or similar pattern, store it there instead to avoid schema changes.

- [ ] **Step 3: Verify compilation**

Run: `cd src-tauri && cargo check 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/actions/post_process/pipeline.rs src-tauri/src/managers/pipeline_log.rs
git commit -m "Log has_cursor_context in pipeline decision record"
```
