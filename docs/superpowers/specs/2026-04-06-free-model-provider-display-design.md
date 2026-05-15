# Free Model Provider Display

**Date:** 2026-04-06
**Status:** Approved

## Intent

Improve the "选择模型" (Add Model) dialog to:

1. Show which API provider (gitee, xunfei) each free model belongs to.
2. Automatically determine free model availability from provider template metadata instead of a hardcoded mapping.
3. Hide the "内置免费模型" tab when the current provider has no associated free models.

## Constraints

- Only built-in (official) provider templates are in scope. Custom provider association with free models is future work.
- No backend changes required — the `freeModelProvider` key lives in the frontend `ProviderTemplate` type.
- Free model loading continues to call `get_free_models` with the resolved provider key.

## Decisions

### 1. ProviderTemplate new field

Add `freeModelProvider?: string` to `ProviderTemplate` in `providerTemplates.ts`.

Assignments:

- `gitee` template: `freeModelProvider: "gitee"`
- `xingchen` template: `freeModelProvider: "xunfei"`
- All other templates: field omitted (no free models)

### 2. AddModelDialog logic

**Tab visibility:**

- Look up the current `providerState.selectedProviderId` against `PROVIDER_TEMPLATES` to find a matching template.
- If the template has `freeModelProvider` → show both tabs, default to "内置免费模型".
- If no `freeModelProvider` → hide the SegmentedControl, show only "官方模型列表".

**Free model loading:**

- Delete the hardcoded `PROVIDER_TO_WORKER` mapping.
- Derive the worker provider key from the matched template's `freeModelProvider` field.
- Pass this key to `invoke("get_free_models", { provider: freeModelProviderKey })`.

**Adding models:**

- Continue using `providerState.selectedProviderId` as the `provider_id` on the `CachedModel`. The user is operating within that provider context.

### 3. Model card provider label

For `source === "free"` models, display the provider template's `label` (e.g., "Gitee AI", "讯飞星辰") on the card.

Layout change in Row 2:

```
Before: capabilities    已添加    Free
After:  capabilities · Gitee AI    已添加    Free
```

The provider label is resolved by finding the template whose `freeModelProvider` matches the current provider key.

## Boundaries

### Allowed files

- `src/components/settings/post-processing/providerTemplates.ts`
- `src/components/settings/post-processing/dialogs/AddModelDialog.tsx`

### Forbidden

- No backend (Rust) changes.
- No changes to `FreeModel` type or `free_models.rs`.
- No custom provider ↔ free model association (future work).

## Acceptance Scenarios

### Happy path: Provider with free models

**Given** the user selects a provider whose template has `freeModelProvider` (e.g., gitee)
**When** the Add Model dialog opens
**Then** both tabs are visible, "内置免费模型" is selected by default, free models load filtered by "gitee", and each model card shows "Gitee AI" as the provider label.

### Happy path: Provider without free models

**Given** the user selects a provider whose template has no `freeModelProvider` (e.g., openai)
**When** the Add Model dialog opens
**Then** the tab switcher is hidden, "官方模型列表" is shown directly, and the API model list loads.

### Edge case: Provider not in templates

**Given** the user has a custom provider not matching any template
**When** the Add Model dialog opens
**Then** behaves the same as "no free models" — only the API model list is shown.
