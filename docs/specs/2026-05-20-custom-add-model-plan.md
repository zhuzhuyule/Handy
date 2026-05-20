# Custom Add Model Dialog Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `+ 自定义` entry in `AddModelDialog` that opens a sub-dialog allowing users to manually enter a model ID with type/label/body-params/headers, run a ping inference test that honors the entered config, and persist the model as a `CachedModel` only after the test passes or the user explicitly skips it.

**Architecture:** Extend the existing `test_post_process_model_inference` Tauri command with two optional override parameters so the same command serves both the "existing CachedModel" path and the "inline pre-save" path. Add a `testInferenceInline` helper in `usePostProcessProviderState` that wraps the new signature. Build a new `CustomAddModelDialog` React component that reuses `KeyValueEditor` for body/headers and overlays on top of `AddModelDialog`.

**Tech Stack:** Rust + Tauri 2.x (backend), React 18 + TypeScript + Radix UI + Zustand (frontend), specta (type generation), `bun` toolchain.

**Spec:** `docs/specs/2026-05-20-custom-add-model.spec.md`

---

## File Structure

**Create**

- `src/components/settings/post-processing/dialogs/CustomAddModelDialog.tsx` — sub-dialog component (~250 lines)

**Modify**

- `src-tauri/src/shortcut/test_cmds.rs` — extend command signature; add `resolve_test_extras` helper + `#[cfg(test)]` module
- `src/bindings.ts` — regenerated via specta build
- `src/stores/settingsStore.ts` — `testPostProcessInference` accepts optional overrides
- `src/hooks/useSettings.ts` — type sync for the store function
- `src/components/settings/PostProcessingSettingsApi/usePostProcessProviderState.ts` — expose `testInferenceInline(modelId, extraParams, extraHeaders)` helper
- `src/components/settings/post-processing/dialogs/AddModelDialog.tsx` — toolbar `+ 自定义` button + sub-dialog open state

**Out of scope (per spec §禁止)**

- `EditModelDialog.tsx`, `KeyValueEditor.tsx`, `add_cached_model`, `CachedModel` data structure, any prompt file in `src-tauri/resources/prompts/`

---

## Task 1: Extract `resolve_test_extras` helper in `test_cmds.rs`

**Files:**

- Modify: `src-tauri/src/shortcut/test_cmds.rs:21-46`

**Rationale:** The current lookup-and-merge block (lines 22-46) is hard to unit-test because it lives inside the `#[tauri::command]` body. Extracting it lets us TDD the resolution rule.

- [ ] **Step 1: Write the failing test module**

Append to `src-tauri/src/shortcut/test_cmds.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::CachedModel;
    use std::collections::HashMap;

    fn make_cached_model(id: &str, is_thinking: bool) -> CachedModel {
        CachedModel {
            id: id.to_string(),
            name: "test".to_string(),
            model_type: crate::settings::ModelType::Text,
            provider_id: "openai".to_string(),
            model_id: "gpt-4o".to_string(),
            added_at: "2026-05-20T00:00:00Z".to_string(),
            is_thinking_model: is_thinking,
            prompt_message_role: "system".to_string(),
            custom_label: None,
            extra_params: Some({
                let mut m = HashMap::new();
                m.insert("temperature".to_string(), serde_json::json!(0.7));
                m
            }),
            extra_headers: Some({
                let mut m = HashMap::new();
                m.insert("X-Cached".to_string(), "yes".to_string());
                m
            }),
            model_family: None,
        }
    }

    #[test]
    fn override_path_uses_overrides_and_ignores_cached() {
        let models = vec![make_cached_model("m1", true)];
        let mut params_override = HashMap::new();
        params_override.insert("top_p".to_string(), serde_json::json!(0.9));
        let mut headers_override = HashMap::new();
        headers_override.insert("X-Inline".to_string(), "yes".to_string());

        let (params, headers) = resolve_test_extras(
            &models,
            Some("m1"),
            Some(params_override.clone()),
            Some(headers_override.clone()),
        );

        assert_eq!(params, Some(params_override), "should use override params, not cached");
        assert_eq!(headers, Some(headers_override), "should use override headers, not cached");
    }

    #[test]
    fn override_partial_only_params_yields_none_headers() {
        let models = vec![make_cached_model("m1", false)];
        let mut params_override = HashMap::new();
        params_override.insert("top_p".to_string(), serde_json::json!(0.9));

        let (params, headers) = resolve_test_extras(
            &models,
            Some("m1"),
            Some(params_override.clone()),
            None,
        );

        assert_eq!(params, Some(params_override));
        assert_eq!(headers, None, "override path drops cached headers when only params provided");
    }

    #[test]
    fn legacy_path_returns_cached_model_extras() {
        let models = vec![make_cached_model("m1", false)];
        let (params, headers) = resolve_test_extras(&models, Some("m1"), None, None);
        assert!(params.is_some(), "should pull params from cached model");
        assert!(headers.is_some(), "should pull headers from cached model");
        let p = params.unwrap();
        assert_eq!(p.get("temperature"), Some(&serde_json::json!(0.7)));
    }

    #[test]
    fn legacy_path_no_cached_id_returns_none() {
        let models = vec![make_cached_model("m1", false)];
        let (params, headers) = resolve_test_extras(&models, None, None, None);
        assert_eq!(params, None);
        assert_eq!(headers, None);
    }
}
```

- [ ] **Step 2: Run tests to confirm they fail to compile (function doesn't exist)**

```bash
rtk cargo test --manifest-path src-tauri/Cargo.toml -p votype shortcut::test_cmds::tests 2>&1 | tail -20
```

Expected: compile error `cannot find function 'resolve_test_extras' in this scope`.

- [ ] **Step 3: Add the helper above the `#[tauri::command]` block in `test_cmds.rs`**

Insert just after the `use` lines at the top:

```rust
/// Resolve the effective `(extra_params, extra_headers)` for a test inference call.
///
/// Rule (per docs/specs/2026-05-20-custom-add-model.spec.md):
/// - If any override is `Some`, both come from the overrides as-is. Thinking
///   auto-inject is skipped (user is responsible for writing the thinking
///   keys via the dialog's preset buttons).
/// - Otherwise, look up the CachedModel by `cached_model_id` and merge its
///   `extra_params` with any thinking params derived from `is_thinking_model`.
fn resolve_test_extras(
    cached_models: &[crate::settings::CachedModel],
    cached_model_id: Option<&str>,
    extra_params_override: Option<std::collections::HashMap<String, serde_json::Value>>,
    extra_headers_override: Option<std::collections::HashMap<String, String>>,
) -> (
    Option<std::collections::HashMap<String, serde_json::Value>>,
    Option<std::collections::HashMap<String, String>>,
) {
    if extra_params_override.is_some() || extra_headers_override.is_some() {
        return (extra_params_override, extra_headers_override);
    }

    let cached_model =
        cached_model_id.and_then(|id| cached_models.iter().find(|m| m.id == id));
    let user_params = cached_model.and_then(|m| m.extra_params.clone());
    let headers = cached_model.and_then(|m| m.extra_headers.clone());
    let thinking_params = cached_model.and_then(|cm| {
        crate::settings::thinking_extra_params_with_aliases(
            &cm.model_id,
            &cm.provider_id,
            cm.is_thinking_model,
            &[cm.custom_label.as_deref().unwrap_or("")],
        )
    });
    let merged_params = match (thinking_params, user_params) {
        (Some(mut tp), Some(up)) => {
            tp.extend(up);
            Some(tp)
        }
        (Some(tp), None) => Some(tp),
        (None, Some(up)) => Some(up),
        (None, None) => None,
    };
    (merged_params, headers)
}
```

- [ ] **Step 4: Run tests to confirm all four pass**

```bash
rtk cargo test --manifest-path src-tauri/Cargo.toml -p votype shortcut::test_cmds::tests 2>&1 | tail -10
```

Expected: `test result: ok. 4 passed; 0 failed`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/shortcut/test_cmds.rs
git commit -m "Extract resolve_test_extras helper with unit tests"
```

---

## Task 2: Wire `resolve_test_extras` into the Tauri command + add override args

**Files:**

- Modify: `src-tauri/src/shortcut/test_cmds.rs:6-46`

- [ ] **Step 1: Replace the command signature + lookup block**

Find lines 6-46 (the `#[tauri::command]` block up through the `merged_extra_params` `match`). Replace with:

```rust
// Group: Inference Testing
#[tauri::command]
#[specta::specta]
pub async fn test_post_process_model_inference(
    app: AppHandle,
    model_id: String,
    provider_id: String,
    cached_model_id: Option<String>,
    extra_params_override: Option<std::collections::HashMap<String, serde_json::Value>>,
    extra_headers_override: Option<std::collections::HashMap<String, String>>,
) -> Result<crate::llm_client::InferenceResult, String> {
    let settings = settings::get_settings(&app);
    let provider = settings
        .post_process_providers
        .iter()
        .find(|p| p.id == provider_id)
        .ok_or("Provider not found")?;

    let (merged_extra_params, resolved_headers) = resolve_test_extras(
        &settings.cached_models,
        cached_model_id.as_deref(),
        extra_params_override,
        extra_headers_override,
    );
    let extra_headers = resolved_headers;
```

Below this, **delete** the original `let cached_model = ...` through the end of the `match (thinking_params, extra_params.cloned())` block (the part that was just replaced). Keep everything from `let effective_proxy = ...` (originally line 47) onward.

Inside the closure (originally around line 75-95), update the variable captures:

- The original `let extra_headers = extra_headers.cloned();` → change to `let extra_headers = extra_headers.clone();` (because `resolved_headers` is now an owned `Option<HashMap>`, not a reference).

Concretely the closure becomes:

```rust
        {
            let provider = provider.clone();
            let model_id = model_id.clone();
            let prompt = "你是啥模型？".to_string();
            let merged_extra_params = merged_extra_params.clone();
            let extra_headers = extra_headers.clone();
            let effective_proxy = effective_proxy.clone();

            move |api_key| {
                let provider = provider.clone();
                let model_id = model_id.clone();
                let prompt = prompt.clone();
                let merged_extra_params = merged_extra_params.clone();
                let extra_headers = extra_headers.clone();
                let effective_proxy = effective_proxy.clone();
                let api_key = api_key.to_string();

                async move {
                    match crate::llm_client::send_chat_completion_with_params(
                        &provider,
                        api_key,
                        &model_id,
                        prompt,
                        merged_extra_params.as_ref(),
                        extra_headers.as_ref(),
                        effective_proxy.as_deref(),
                    )
                    .await
                    {
                        // ... existing error mapping unchanged
```

The error-mapping `match` block and the rest of the function body (metrics emit, etc.) remain unchanged.

- [ ] **Step 2: Verify compile**

```bash
rtk cargo check --manifest-path src-tauri/Cargo.toml -p votype 2>&1 | tail -15
```

Expected: no errors. Warnings are blockers (must fix per project rule — see [feedback_fix_all_warnings.md]).

- [ ] **Step 3: Verify tests still pass + add backward-compat test**

Add to the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn override_some_with_cached_model_id_still_ignores_cached() {
        let models = vec![make_cached_model("m1", true)];
        let mut params_override = HashMap::new();
        params_override.insert("top_p".to_string(), serde_json::json!(0.9));

        let (params, _) = resolve_test_extras(
            &models,
            Some("m1"),
            Some(params_override.clone()),
            None,
        );
        let p = params.unwrap();
        assert!(p.contains_key("top_p"), "override-only key present");
        assert!(!p.contains_key("temperature"), "cached extra_params must NOT leak in");
    }
```

Run:

```bash
rtk cargo test --manifest-path src-tauri/Cargo.toml -p votype shortcut::test_cmds::tests 2>&1 | tail -10
```

Expected: 5 passed.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/shortcut/test_cmds.rs
git commit -m "Accept inline params/headers overrides in test_post_process_model_inference"
```

---

## Task 3: Regenerate `src/bindings.ts`

**Files:**

- Modify (generated): `src/bindings.ts`

- [ ] **Step 1: Run the build that triggers specta export**

The project regenerates `bindings.ts` as part of `cargo build`. Run:

```bash
rtk cargo build --manifest-path src-tauri/Cargo.toml -p votype 2>&1 | tail -10
```

Expected: build succeeds; `src/bindings.ts` updated with the two new optional fields in `test_post_process_model_inference`.

- [ ] **Step 2: Verify the new fields appear**

```bash
rtk grep -n "test_post_process_model_inference" src/bindings.ts
```

Expected: signature contains `extraParamsOverride` and `extraHeadersOverride` as optional parameters.

- [ ] **Step 3: Commit**

```bash
git add src/bindings.ts
git commit -m "Regenerate bindings for inline override params"
```

---

## Task 4: Update `testPostProcessInference` in `settingsStore.ts` + `useSettings.ts`

**Files:**

- Modify: `src/stores/settingsStore.ts:61-64, 804-820`
- Modify: `src/hooks/useSettings.ts` (the typed re-export around line 88)

- [ ] **Step 1: Update the store interface signature** (line ~61)

```typescript
testPostProcessInference: (
  providerId: string,
  modelId: string,
  overrides?: {
    extraParams?: Record<string, unknown> | null;
    extraHeaders?: Record<string, string> | null;
  },
) =>
  Promise<{
    content?: string;
    reasoning_content?: string;
    duration_ms?: number;
    total_tokens?: number;
  }>;
```

- [ ] **Step 2: Update the implementation** (line ~804)

```typescript
    testPostProcessInference: async (
      providerId: string,
      modelId: string,
      overrides?: {
        extraParams?: Record<string, unknown> | null;
        extraHeaders?: Record<string, string> | null;
      },
    ) => {
      const updateKey = `test_post_process_inference:${providerId}`;
      const { setUpdating } = get();
      setUpdating(updateKey, true);
      try {
        const result = (await invoke("test_post_process_model_inference", {
          providerId,
          modelId,
          extraParamsOverride: overrides?.extraParams ?? null,
          extraHeadersOverride: overrides?.extraHeaders ?? null,
        })) as {
          content?: string;
          reasoning_content?: string;
          duration_ms?: number;
          total_tokens?: number;
        };
        return result;
      } catch (error) {
        console.error("Failed to test post-process inference:", error);
        throw error;
      } finally {
        setUpdating(updateKey, false);
      }
    },
```

- [ ] **Step 3: Sync the hook re-export type** in `src/hooks/useSettings.ts`

Find the `testPostProcessInference` line (around 88) and update its type to match:

```typescript
testPostProcessInference: (
  providerId: string,
  modelId: string,
  overrides?: {
    extraParams?: Record<string, unknown> | null;
    extraHeaders?: Record<string, string> | null;
  },
) =>
  Promise<{
    content?: string;
    reasoning_content?: string;
    duration_ms?: number;
    total_tokens?: number;
  }>;
```

- [ ] **Step 4: Verify existing callers still type-check**

```bash
rtk tsc --noEmit 2>&1 | tail -10
```

Expected: no errors. Existing call sites in `usePostProcessProviderState.ts:223` and `ModelConfigurationPanel.tsx:142` already pass only `(providerId, modelId)`, so the new optional third arg is backward-compatible.

- [ ] **Step 5: Commit**

```bash
git add src/stores/settingsStore.ts src/hooks/useSettings.ts
git commit -m "Accept optional overrides in testPostProcessInference store call"
```

---

## Task 5: Add `testInferenceInline` helper in `usePostProcessProviderState.ts`

**Files:**

- Modify: `src/components/settings/PostProcessingSettingsApi/usePostProcessProviderState.ts:32 (interface)`, `:218-247 (testInference impl block)`, `:325-330 (return obj)`

- [ ] **Step 1: Read the surrounding `testInference` block first to keep style consistent**

```bash
rtk read src/components/settings/PostProcessingSettingsApi/usePostProcessProviderState.ts | head -260 | tail -60
```

- [ ] **Step 2: Add the new helper right after `testInference`**

Insert this block immediately after the closing of `testInference` (search for `const testInference = useCallback(`):

```typescript
const testInferenceInline = useCallback(
  async (
    modelId: string,
    extraParams: Record<string, unknown> | null,
    extraHeaders: Record<string, string> | null,
  ): Promise<
    | {
        content: string;
        hasThinking: boolean;
        durationMs: number | null;
        totalTokens: number | null;
        error?: undefined;
      }
    | { error: string }
  > => {
    try {
      const result = await testPostProcessInference(
        viewingProviderId,
        modelId,
        {
          extraParams,
          extraHeaders,
        },
      );
      const rawContent = result.content ?? "";
      const mainContent = rawContent
        .replace(/<think>[\s\S]*?<\/think>/g, "")
        .trim();
      if (!mainContent) {
        return { error: "Empty response from provider" };
      }
      const hasThinking =
        (typeof result.reasoning_content === "string" &&
          result.reasoning_content.trim().length > 0) ||
        /<think>[\s\S]*?<\/think>/.test(rawContent);
      return {
        content: mainContent,
        hasThinking,
        durationMs: result.duration_ms ?? null,
        totalTokens: result.total_tokens ?? null,
      };
    } catch (e) {
      return { error: typeof e === "string" ? e : JSON.stringify(e) };
    }
  },
  [testPostProcessInference, viewingProviderId],
);
```

- [ ] **Step 3: Add `testInferenceInline` to the interface (around line 32)**

Find the `interface PostProcessProviderState` (or equivalent type declaration). Add:

```typescript
testInferenceInline: (
  modelId: string,
  extraParams: Record<string, unknown> | null,
  extraHeaders: Record<string, string> | null,
) =>
  Promise<
    | {
        content: string;
        hasThinking: boolean;
        durationMs: number | null;
        totalTokens: number | null;
        error?: undefined;
      }
    | { error: string }
  >;
```

- [ ] **Step 4: Include `testInferenceInline` in the returned object (around line 325)**

Find the return statement of the hook (look for `return {` near line 325). Add the new property next to `testInference`:

```typescript
    testInference,
    testInferenceInline,
```

- [ ] **Step 5: Verify type-check**

```bash
rtk tsc --noEmit 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/components/settings/PostProcessingSettingsApi/usePostProcessProviderState.ts
git commit -m "Add testInferenceInline helper for pre-save test calls"
```

---

## Task 6: Scaffold `CustomAddModelDialog` — form fields + cancel

**Files:**

- Create: `src/components/settings/post-processing/dialogs/CustomAddModelDialog.tsx`

This task creates the component **without** test logic or add logic — just the visible form. The next tasks add behavior incrementally.

- [ ] **Step 1: Create the file with initial structure**

```typescript
import {
  Button,
  Callout,
  Dialog,
  Flex,
  SegmentedControl,
  Text,
  TextField,
} from "@radix-ui/themes";
import { IconInfoCircle } from "@tabler/icons-react";
import React, { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { useSettings } from "../../../../hooks/useSettings";
import type { ModelType } from "../../../../lib/types";

interface CustomAddModelDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  providerId: string;
}

export const CustomAddModelDialog: React.FC<CustomAddModelDialogProps> = ({
  open,
  onOpenChange,
  providerId,
}) => {
  const { t } = useTranslation();
  const { settings } = useSettings();

  const [modelId, setModelId] = useState("");
  const [modelType, setModelType] = useState<ModelType>("text");
  const [label, setLabel] = useState("");

  const cachedModels = settings?.cached_models ?? [];
  const isDuplicate = useMemo(
    () =>
      cachedModels.some(
        (m) => m.provider_id === providerId && m.model_id === modelId.trim(),
      ) && modelId.trim().length > 0,
    [cachedModels, providerId, modelId],
  );

  const handleClose = () => {
    setModelId("");
    setModelType("text");
    setLabel("");
    onOpenChange(false);
  };

  return (
    <Dialog.Root open={open} onOpenChange={(o) => !o && handleClose()}>
      <Dialog.Content maxWidth="540px">
        <Dialog.Title>
          {t(
            "settings.postProcessing.models.customAdd.title",
            "Add Custom Model",
          )}
        </Dialog.Title>

        <Flex direction="column" gap="4" mt="3">
          {isDuplicate && (
            <Callout.Root color="amber" size="1">
              <Callout.Icon>
                <IconInfoCircle size={14} />
              </Callout.Icon>
              <Callout.Text>
                {t(
                  "settings.postProcessing.models.customAdd.duplicateWarning",
                  "A model with this ID already exists for this provider. Adding will create a copy.",
                )}
              </Callout.Text>
            </Callout.Root>
          )}

          <Flex direction="column" gap="1">
            <Text size="2" weight="medium" color="gray">
              {t(
                "settings.postProcessing.models.customAdd.modelId",
                "Model ID",
              )}
            </Text>
            <TextField.Root
              value={modelId}
              onChange={(e) => setModelId(e.target.value)}
              placeholder="e.g. gpt-4o, my-custom-llama"
              autoFocus
            />
          </Flex>

          <Flex direction="column" gap="1">
            <Text size="2" weight="medium" color="gray">
              {t(
                "settings.postProcessing.models.selectModel.usageTypeTitle",
                "Usage Type",
              )}
            </Text>
            <SegmentedControl.Root
              value={modelType}
              onValueChange={(v) => setModelType(v as ModelType)}
              size="1"
            >
              <SegmentedControl.Item value="text">
                {t(
                  "settings.postProcessing.models.modelTypes.text.label",
                  "Text",
                )}
              </SegmentedControl.Item>
              <SegmentedControl.Item value="asr">
                {t(
                  "settings.postProcessing.models.modelTypes.asr.label",
                  "ASR",
                )}
              </SegmentedControl.Item>
              <SegmentedControl.Item value="other">
                {t(
                  "settings.postProcessing.models.modelTypes.other.label",
                  "Other",
                )}
              </SegmentedControl.Item>
            </SegmentedControl.Root>
          </Flex>

          <Flex direction="column" gap="1">
            <Text size="2" weight="medium" color="gray">
              {t(
                "settings.postProcessing.models.selectModel.customLabel",
                "Display Name",
              )}
            </Text>
            <TextField.Root
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder={modelId || t("common.optional", "Optional")}
            />
          </Flex>

          {/* Body params, headers, test panel, footer added in later tasks */}

          <Flex justify="end" gap="3" mt="2">
            <Button variant="soft" color="gray" onClick={handleClose}>
              {t("common.cancel")}
            </Button>
          </Flex>
        </Flex>
      </Dialog.Content>
    </Dialog.Root>
  );
};
```

- [ ] **Step 2: Verify type-check (component is unused but must compile)**

```bash
rtk tsc --noEmit 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add src/components/settings/post-processing/dialogs/CustomAddModelDialog.tsx
git commit -m "Scaffold CustomAddModelDialog with form fields and duplicate warning"
```

---

## Task 7: Add Body params + Headers editors (KeyValueEditor)

**Files:**

- Modify: `src/components/settings/post-processing/dialogs/CustomAddModelDialog.tsx`

- [ ] **Step 1: Update imports**

At the top of the file, replace the existing imports with:

```typescript
import {
  Button,
  Callout,
  Dialog,
  Flex,
  IconButton,
  SegmentedControl,
  Text,
  TextField,
  Tooltip,
} from "@radix-ui/themes";
import { IconBrain, IconInfoCircle, IconPlus } from "@tabler/icons-react";
import React, { useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { useSettings } from "../../../../hooks/useSettings";
import type { ModelType } from "../../../../lib/types";
import {
  KeyValueEditor,
  type KeyValueEditorHandle,
} from "../../../ui/KeyValueEditor";
```

- [ ] **Step 2: Add state hooks below the existing `label` state**

```typescript
const [extraParams, setExtraParams] = useState<Record<string, unknown>>({});
const [extraHeaders, setExtraHeaders] = useState<Record<string, unknown>>({});
const bodyEditorRef = useRef<KeyValueEditorHandle>(null);
const headersEditorRef = useRef<KeyValueEditorHandle>(null);
const [bodyEntryCount, setBodyEntryCount] = useState(0);
const [headerEntryCount, setHeaderEntryCount] = useState(0);
```

Update `handleClose` to reset them as well:

```typescript
const handleClose = () => {
  setModelId("");
  setModelType("text");
  setLabel("");
  setExtraParams({});
  setExtraHeaders({});
  onOpenChange(false);
};
```

- [ ] **Step 3: Insert the Body params + Headers blocks** (before the footer)

Place this between the Display Name `Flex` block and the `{/* Body params, headers, test panel, footer added in later tasks */}` comment. Remove that placeholder comment:

```typescript
          <Flex direction="column" gap="1">
            <Flex align="center" gap="2" wrap="wrap">
              <Text size="2" weight="medium" color="gray">
                Body 参数
              </Text>
              {bodyEntryCount > 0 && (
                <Tooltip content="添加参数">
                  <IconButton
                    size="1"
                    variant="outline"
                    className="h-5! w-5!"
                    color="gray"
                    onClick={() => bodyEditorRef.current?.addEntry()}
                  >
                    <IconPlus size={12} />
                  </IconButton>
                </Tooltip>
              )}
              <Button
                size="1"
                variant="soft"
                color="blue"
                onClick={() =>
                  setExtraParams((prev) => ({
                    ...prev,
                    thinking: { type: "enabled" },
                  }))
                }
              >
                <IconBrain size={12} />
                启用思考
              </Button>
              <Button
                size="1"
                variant="soft"
                color="orange"
                onClick={() =>
                  setExtraParams((prev) => ({
                    ...prev,
                    thinking: { type: "disabled" },
                  }))
                }
              >
                <IconBrain size={12} />
                禁用思考
              </Button>
            </Flex>
            <KeyValueEditor
              value={extraParams}
              onChange={setExtraParams}
              addLabel="添加 Body 参数"
              addTooltip="添加参数"
              addRef={bodyEditorRef}
              onEntryCountChange={setBodyEntryCount}
            />
          </Flex>

          <Flex direction="column" gap="1">
            <Flex align="center" gap="2">
              <Text size="2" weight="medium" color="gray">
                Headers
              </Text>
              {headerEntryCount > 0 && (
                <Tooltip content="添加 Header">
                  <IconButton
                    size="1"
                    variant="outline"
                    color="gray"
                    className="h-5! w-5!"
                    onClick={() => headersEditorRef.current?.addEntry()}
                  >
                    <IconPlus size={12} />
                  </IconButton>
                </Tooltip>
              )}
            </Flex>
            <KeyValueEditor
              value={extraHeaders}
              onChange={setExtraHeaders}
              addLabel="添加 Header"
              addTooltip="添加 Header"
              addRef={headersEditorRef}
              onEntryCountChange={setHeaderEntryCount}
            />
          </Flex>
```

- [ ] **Step 4: Type-check**

```bash
rtk tsc --noEmit 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/components/settings/post-processing/dialogs/CustomAddModelDialog.tsx
git commit -m "Add Body params and Headers editors to CustomAddModelDialog"
```

---

## Task 8: Add Test button + result panel

**Files:**

- Modify: `src/components/settings/post-processing/dialogs/CustomAddModelDialog.tsx`

- [ ] **Step 1: Add `testInferenceInline` to props**

Update the prop interface and destructure:

```typescript
interface CustomAddModelDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  providerId: string;
  testInferenceInline: (
    modelId: string,
    extraParams: Record<string, unknown> | null,
    extraHeaders: Record<string, string> | null,
  ) => Promise<
    | {
        content: string;
        hasThinking: boolean;
        durationMs: number | null;
        totalTokens: number | null;
        error?: undefined;
      }
    | { error: string }
  >;
}
```

Update the destructure: `({ open, onOpenChange, providerId, testInferenceInline })`.

- [ ] **Step 2: Add test state**

Below the editor refs:

```typescript
type TestState =
  | { kind: "idle" }
  | { kind: "testing" }
  | {
      kind: "passed";
      content: string;
      hasThinking: boolean;
      durationMs: number | null;
      totalTokens: number | null;
    }
  | { kind: "failed"; error: string };

const [testState, setTestState] = useState<TestState>({ kind: "idle" });

const tokensPerSec = (() => {
  if (testState.kind !== "passed") return null;
  const { totalTokens, durationMs } = testState;
  if (!totalTokens || !durationMs || durationMs <= 0) return null;
  return (totalTokens / durationMs) * 1000;
})();

const headersAsStringMap = (): Record<string, string> | null => {
  const entries = Object.entries(extraHeaders);
  if (entries.length === 0) return null;
  const out: Record<string, string> = {};
  for (const [k, v] of entries) {
    out[k] = typeof v === "string" ? v : JSON.stringify(v);
  }
  return out;
};

const handleTest = async () => {
  const id = modelId.trim();
  if (!id) return;
  setTestState({ kind: "testing" });
  const result = await testInferenceInline(
    id,
    Object.keys(extraParams).length > 0 ? extraParams : null,
    headersAsStringMap(),
  );
  if ("error" in result && result.error) {
    setTestState({ kind: "failed", error: result.error });
  } else if (!("error" in result)) {
    setTestState({
      kind: "passed",
      content: result.content,
      hasThinking: result.hasThinking,
      durationMs: result.durationMs,
      totalTokens: result.totalTokens,
    });
  }
};
```

Also extend `handleClose` to reset `testState`:

```typescript
setTestState({ kind: "idle" });
```

- [ ] **Step 3: Insert the test panel + Test button** (just before the existing footer `Flex`)

```typescript
          <Flex direction="column" gap="2">
            <Flex align="center" gap="2">
              <Tooltip
                content={
                  modelId.trim().length === 0
                    ? "请先填入模型 ID"
                    : "发送一句话 ping 测试"
                }
              >
                <Button
                  variant="soft"
                  color="blue"
                  disabled={
                    modelId.trim().length === 0 || testState.kind === "testing"
                  }
                  onClick={handleTest}
                >
                  {testState.kind === "testing" ? "测试中…" : "测试"}
                </Button>
              </Tooltip>
            </Flex>

            {testState.kind === "passed" && (
              <Callout.Root color="green" size="1">
                <Callout.Icon>
                  <IconInfoCircle size={14} />
                </Callout.Icon>
                <Callout.Text>
                  <Flex direction="column" gap="1">
                    <Text size="2">
                      {testState.content.length > 200
                        ? testState.content.slice(0, 200) + "…"
                        : testState.content}
                    </Text>
                    <Flex gap="3">
                      {tokensPerSec !== null && (
                        <Text size="1" color="gray">
                          {tokensPerSec.toFixed(1)} t/s
                        </Text>
                      )}
                      {testState.hasThinking && (
                        <Text size="1" color="blue">
                          🧠 Thinking
                        </Text>
                      )}
                    </Flex>
                  </Flex>
                </Callout.Text>
              </Callout.Root>
            )}

            {testState.kind === "failed" && (
              <Callout.Root color="red" size="1">
                <Callout.Icon>
                  <IconInfoCircle size={14} />
                </Callout.Icon>
                <Callout.Text>{testState.error}</Callout.Text>
              </Callout.Root>
            )}
          </Flex>
```

- [ ] **Step 4: Type-check**

```bash
rtk tsc --noEmit 2>&1 | tail -10
```

- [ ] **Step 5: Commit**

```bash
git add src/components/settings/post-processing/dialogs/CustomAddModelDialog.tsx
git commit -m "Add inline test button and result callout to CustomAddModelDialog"
```

---

## Task 9: Add Skip-test link + Add button + persist logic

**Files:**

- Modify: `src/components/settings/post-processing/dialogs/CustomAddModelDialog.tsx`

- [ ] **Step 1: Add `addCachedModel` import and skip state**

In the top imports, add to the existing `useSettings` destructure call inside the component:

```typescript
const { settings, addCachedModel } = useSettings();
```

Add type imports near `ModelType`:

```typescript
import type { CachedModel, ModelType } from "../../../../lib/types";
```

Add state next to `testState`:

```typescript
const [skipped, setSkipped] = useState(false);
const [adding, setAdding] = useState(false);
```

When `handleTest` is invoked, **reset `skipped` to false** at the top of `handleTest`:

```typescript
const handleTest = async () => {
  const id = modelId.trim();
  if (!id) return;
  setSkipped(false);
  setTestState({ kind: "testing" });
  // ... rest unchanged
};
```

Extend `handleClose` to reset both:

```typescript
setSkipped(false);
setAdding(false);
```

- [ ] **Step 2: Add the persist handler**

Below `handleTest`:

```typescript
const buildCacheId = () => {
  if (globalThis.crypto?.randomUUID) {
    return globalThis.crypto.randomUUID();
  }
  return `${providerId}-${modelId.trim()}-${Date.now()}`;
};

const handleAdd = async () => {
  const id = modelId.trim();
  if (!id) return;
  setAdding(true);
  try {
    const headersAsStringMap_: Record<string, string> = {};
    for (const [k, v] of Object.entries(extraHeaders)) {
      headersAsStringMap_[k] = typeof v === "string" ? v : JSON.stringify(v);
    }
    const newModel: CachedModel = {
      id: buildCacheId(),
      name: label.trim() || id,
      model_type: modelType,
      provider_id: providerId,
      model_id: id,
      added_at: new Date().toISOString(),
      is_thinking_model: false,
      prompt_message_role: "system",
      extra_params:
        Object.keys(extraParams).length > 0 ? extraParams : undefined,
      extra_headers:
        Object.keys(headersAsStringMap_).length > 0
          ? headersAsStringMap_
          : undefined,
    };
    await addCachedModel(newModel);
    handleClose();
  } catch (e) {
    setTestState({
      kind: "failed",
      error: `添加失败: ${typeof e === "string" ? e : JSON.stringify(e)}`,
    });
  } finally {
    setAdding(false);
  }
};
```

> Type note (verified against `src/lib/types.ts:153-155`): `extra_params` is `Record<string, unknown> | undefined`, `extra_headers` is `Record<string, string> | undefined`. Pass objects, not JSON strings.

- [ ] **Step 3: Compute add button enablement and skip link visibility**

Above the return:

```typescript
const canAdd =
  modelId.trim().length > 0 &&
  (testState.kind === "passed" || skipped) &&
  !adding;

const showSkipLink =
  modelId.trim().length > 0 &&
  !skipped &&
  (testState.kind === "idle" || testState.kind === "failed");
```

- [ ] **Step 4: Replace the footer Flex with the full footer**

```typescript
          <Flex justify="end" align="center" gap="3" mt="2">
            {showSkipLink && (
              <Button
                variant="ghost"
                color="gray"
                size="1"
                onClick={() => setSkipped(true)}
              >
                跳过测试直接添加
              </Button>
            )}
            <Button variant="soft" color="gray" onClick={handleClose}>
              {t("common.cancel")}
            </Button>
            <Button
              variant="solid"
              onClick={handleAdd}
              disabled={!canAdd}
            >
              {t("common.add")}
            </Button>
          </Flex>
```

- [ ] **Step 5: Type-check + build**

```bash
rtk tsc --noEmit 2>&1 | tail -15
```

If `CachedModel` field types complain, inspect `src/lib/types.ts` and adjust the literal in `handleAdd` accordingly. Common alternatives:

- `extra_params: extraParams` (object literal directly)
- `extra_params: Object.keys(extraParams).length > 0 ? extraParams : null`

- [ ] **Step 6: Commit**

```bash
git add src/components/settings/post-processing/dialogs/CustomAddModelDialog.tsx
git commit -m "Add Skip-test link and Add button to CustomAddModelDialog"
```

---

## Task 10: Wire `+ 自定义` button into `AddModelDialog`

**Files:**

- Modify: `src/components/settings/post-processing/dialogs/AddModelDialog.tsx`

- [ ] **Step 1: Add the import**

Near the top, alongside existing local imports:

```typescript
import { CustomAddModelDialog } from "./CustomAddModelDialog";
```

- [ ] **Step 2: Add state**

After the existing `const [adding, setAdding] = useState(false);`:

```typescript
const [customAddOpen, setCustomAddOpen] = useState(false);
```

- [ ] **Step 3: Add the button to the toolbar**

Find the toolbar `Flex` block (currently containing the `SegmentedControl`, search `TextField`, and selected-count `Badge`). It's around line 262-310 of the current file.

Insert this `Button` right **after** the `<Box className="min-w-[160px] flex-1">...</Box>` block (which wraps the search TextField), and **before** the `{selectedIds.size > 0 && (<Badge ...>)}` block:

```typescript
            <Tooltip content="添加自定义模型 ID">
              <Button
                size="1"
                variant="soft"
                color="gray"
                onClick={() => setCustomAddOpen(true)}
              >
                <IconPlus size={14} />
                自定义
              </Button>
            </Tooltip>
```

Also add the needed icon import if missing — `IconPlus` is already imported via other places; verify and add to the existing `@tabler/icons-react` import in `AddModelDialog.tsx` if not present.

Add `Tooltip` to the existing `@radix-ui/themes` import line if not already there.

- [ ] **Step 4: Render the sub-dialog at the bottom of the component**

Inside the existing `<Dialog.Root>` block, **outside** of `<Dialog.Content>` but still inside the JSX returned (Radix `Dialog.Root` accepts arbitrary children; the sub-dialog renders into a portal so order is fine). Concretely, add it just before the closing `</Dialog.Root>`:

```typescript
      <CustomAddModelDialog
        open={customAddOpen}
        onOpenChange={setCustomAddOpen}
        providerId={providerState.selectedProviderId}
        testInferenceInline={providerState.testInferenceInline}
      />
```

- [ ] **Step 5: Type-check**

```bash
rtk tsc --noEmit 2>&1 | tail -10
```

- [ ] **Step 6: Commit**

```bash
git add src/components/settings/post-processing/dialogs/AddModelDialog.tsx
git commit -m "Wire + 自定义 button and CustomAddModelDialog into AddModelDialog"
```

---

## Task 11: Manual BDD verification

**Files:** None (manual)

This task walks through the 8 acceptance scenarios from the spec. Mark each scenario as a separate sub-task. Run the dev server once:

```bash
bun tauri dev
```

- [ ] **Scenario 1 — happy_path_test_then_add**
  - Open Settings → Post-Processing → choose any OpenAI-compatible provider with a valid API key
  - Open Add Model dialog → click `+ 自定义`
  - Type a real model ID for the provider (e.g. `gpt-4o-mini` for OpenAI), leave headers/body empty
  - Click Test → confirm green Callout shows response text + t/s
  - Click Add → confirm sub-dialog auto-closes and main Add Model dialog stays open
  - Verify the new model appears under the provider in the main models list

- [ ] **Scenario 2 — error_path_test_failure_still_allows_skip_add**
  - Open `+ 自定义` again
  - Type a bogus model ID like `non-existent-model-xyz`
  - Click Test → expect red Callout with 404 / "Model not found"
  - Confirm "跳过测试直接添加" link is visible
  - Click the skip link → Add button enables
  - Click Add → model is persisted anyway

- [ ] **Scenario 3 — edge_case_duplicate_id_warning_but_allow**
  - Open `+ 自定义`
  - Type a model ID that already exists for the current provider
  - Confirm amber Callout warns about duplicate at the top
  - Test + Add still work; verify a second `CachedModel` is created with the same `model_id` but a different `id`

- [ ] **Scenario 4 — edge_case_empty_id_disables_test_button**
  - Open `+ 自定义`, leave model ID blank
  - Confirm Test button is disabled with tooltip "请先填入模型 ID"
  - Confirm Add button is disabled
  - Confirm "跳过测试直接添加" link is not visible

- [ ] **Scenario 5 — edge_case_thinking_detected_in_response**
  - Open `+ 自定义`
  - Enter a thinking-capable model (e.g. `deepseek-reasoner` or `qwen3-thinking` depending on provider)
  - Click "启用思考" to add `thinking: { type: "enabled" }` to Body params
  - Test → green Callout shows 🧠 Thinking marker alongside the response

- [ ] **Scenario 6 — happy_path_close_subdialog_preserves_main_state**
  - In main Add Model dialog, switch to API tab, type "gpt" in search, check 2 models
  - Click `+ 自定义` → sub-dialog opens, type random text, click Cancel
  - Confirm main dialog still shows API tab, "gpt" in search, and the 2 checked models

- [ ] **Scenario 7 — error_path_backend_signature_backward_compat**
  - In `ModelConfigurationPanel` (the per-model row), click the existing per-row Test button (the one that calls `test_post_process_model_inference` without overrides)
  - Confirm thinking-injected models still trigger thinking; non-thinking models behave as before

- [ ] **Scenario 8 — edge_case_test_with_no_extra_config**
  - Open `+ 自定义`, type only model ID, leave body params and headers empty
  - Test → should succeed (uses provider default config + API key only)
  - Add → persisted with `extra_params=null, extra_headers=null`

- [ ] **Step: After all scenarios verified, commit anything that was tweaked during verification**

```bash
git status
# If any small fix was needed, stage and commit individually
```

---

## Task 12: Final lint + warning sweep

**Files:** any modified during the implementation

Per CLAUDE.md & memory `feedback_fix_all_warnings.md`: treat all warnings as errors before final commit.

- [ ] **Step 1: Backend warnings**

```bash
rtk cargo clippy --manifest-path src-tauri/Cargo.toml -p votype --all-targets -- -D warnings 2>&1 | tail -30
```

Expected: 0 warnings. Fix any introduced by the new override params (most common: unused import, unused variable).

- [ ] **Step 2: Backend build + tests**

```bash
rtk cargo build --manifest-path src-tauri/Cargo.toml -p votype 2>&1 | tail -10
rtk cargo test --manifest-path src-tauri/Cargo.toml -p votype shortcut::test_cmds::tests 2>&1 | tail -10
```

Expected: build clean; 5 tests pass.

- [ ] **Step 3: Frontend build (includes type-check via `tsc`)**

```bash
rtk bun run build 2>&1 | tail -20
```

Expected: clean. The `build` script is `tsc && vite build --debug` (see `package.json:9`); this catches all TS errors.

- [ ] **Step 4: Format (prettier + cargo fmt)**

```bash
bun format
```

- [ ] **Step 5: Commit any format / warning fixes**

```bash
git add -A
git diff --cached --stat
git commit -m "Polish formatting and address warnings for custom add model"
```

---

## Task 13: Append Implementation Deviations to spec

**Files:**

- Modify: `docs/specs/2026-05-20-custom-add-model.spec.md` (the `## 实施偏差` table at the bottom)

Per CLAUDE.md "Spec Writing Rules" → "Deviation Log: After implementation, append an Implementation Deviations table".

- [ ] **Step 1: Fill the `## 实施偏差` table** with rows for each deviation observed during implementation. If none, write a single row "None observed — implementation matched spec exactly."

Format:

```markdown
| 原计划                                                         | 实际实现                                   | 原因                                                                                            |
| -------------------------------------------------------------- | ------------------------------------------ | ----------------------------------------------------------------------------------------------- |
| `extra_headers_override` 类型 `Option<HashMap<String, Value>>` | 实际签名 `Option<HashMap<String, String>>` | 后端 send_chat_completion_with_params 已要求 headers 为 String→String，避免不必要的 JSON 序列化 |
| ...                                                            | ...                                        | ...                                                                                             |
```

- [ ] **Step 2: Commit**

```bash
git add docs/specs/2026-05-20-custom-add-model.spec.md
git commit -m "Record implementation deviations for custom add model feature"
```

---

## Self-Review Summary

After plan completion, the implementer should be able to point to:

- **Spec §约束 "复用现有后端命令"** → Task 1, 2 (extend, not duplicate)
- **Spec §决策 "测试默认必需可跳过"** → Task 9 (Skip-test link + can-add logic)
- **Spec §决策 "表单字段 ID+类型+显示名+Body+Headers"** → Task 6 (basic fields) + Task 7 (Body+Headers)
- **Spec §决策 "叠加子对话框"** → Task 10 (render inside `<Dialog.Root>` of parent)
- **Spec §决策 "后端命令扩展两个可选 override"** → Task 1 (helper + tests) + Task 2 (signature)
- **Spec §决策 "重复 ID 警告不阻断"** → Task 6 (Callout in scaffold) + Scenario 3
- **Spec §决策 "InferenceResult 信号映射"** → Task 5 (`testInferenceInline` strips `<think>`, detects thinking, computes t/s)
- **Spec §决策 "自动关闭"** → Task 9 (`handleAdd` calls `handleClose()` on success)
- **Spec §约束 "前端类型必须 specta 重新生成"** → Task 3
- **Spec §约束 "消除所有 warning"** → Task 12
- **Spec §验收场景 1-8** → Task 11 (one sub-task per scenario)
