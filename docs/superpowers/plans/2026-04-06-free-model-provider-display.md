# Free Model Provider Display Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show provider info on free model cards and auto-detect free model availability from provider template metadata.

**Architecture:** Add `freeModelProvider` field to `ProviderTemplate`, use it to replace the hardcoded `PROVIDER_TO_WORKER` mapping in `AddModelDialog`, conditionally show/hide the free models tab, and display the provider label on model cards.

**Tech Stack:** TypeScript, React

---

### Task 1: Add `freeModelProvider` to ProviderTemplate

**Files:**

- Modify: `src/components/settings/post-processing/providerTemplates.ts`

- [ ] **Step 1: Add field to type and templates**

Add `freeModelProvider?: string` to the `ProviderTemplate` type, and set it on the two templates that have free models:

```typescript
export type ProviderTemplate = {
  id: string;
  label: string;
  baseUrl: string;
  category: string;
  modelsEndpoint?: string;
  websiteUrl?: string;
  signupUrl?: string;
  freeModelProvider?: string;
};
```

In the `PROVIDER_TEMPLATES` array:

For the `gitee` entry (id: `"gitee"`), add:

```typescript
freeModelProvider: "gitee",
```

For the `xingchen` entry (id: `"xingchen"`), add:

```typescript
freeModelProvider: "xunfei",
```

- [ ] **Step 2: Verify no TypeScript errors**

Run: `cd /Users/zac/code/github/asr/Handy && npx tsc --noEmit --pretty 2>&1 | head -20`
Expected: No errors (field is optional, no consumers break).

- [ ] **Step 3: Commit**

```bash
git add src/components/settings/post-processing/providerTemplates.ts
git commit -m "Add freeModelProvider field to ProviderTemplate"
```

---

### Task 2: Refactor AddModelDialog to use template metadata

**Files:**

- Modify: `src/components/settings/post-processing/dialogs/AddModelDialog.tsx`

- [ ] **Step 1: Replace hardcoded mapping with template lookup**

At the top of the file, delete the `PROVIDER_TO_WORKER` constant (lines 28-31):

```typescript
// DELETE THIS:
const PROVIDER_TO_WORKER: Record<string, string> = {
  gitee: "gitee",
  xingchen: "xunfei",
};
```

Add an import for `PROVIDER_TEMPLATES`:

```typescript
import { PROVIDER_TEMPLATES } from "../providerTemplates";
```

- [ ] **Step 2: Add helper to resolve free model provider**

Add this helper inside the file (below imports, before the component):

```typescript
function resolveTemplateMeta(providerId: string) {
  return PROVIDER_TEMPLATES.find((t) => t.id === providerId) ?? null;
}
```

- [ ] **Step 3: Update component to derive freeModelProvider**

Inside the `AddModelDialog` component, after `const [adding, setAdding] = useState(false);` (line 112), add:

```typescript
const templateMeta = resolveTemplateMeta(providerState.selectedProviderId);
const freeModelProviderKey = templateMeta?.freeModelProvider ?? null;
const hasFreeModels = freeModelProviderKey !== null;
```

- [ ] **Step 4: Update free model loading to use derived key**

In the `useEffect` that loads free models (the one triggered by `[open, providerState.selectedProviderId]`), replace:

```typescript
const workerProvider =
  PROVIDER_TO_WORKER[providerState.selectedProviderId] ?? null;
invoke<FreeModel[]>("get_free_models", { provider: workerProvider });
```

with:

```typescript
invoke<FreeModel[]>("get_free_models", { provider: freeModelProviderKey });
```

- [ ] **Step 5: Default source based on free model availability**

In the same `useEffect`, after `setQuery("");`, add logic to set the default source:

```typescript
setSource(freeModelProviderKey ? "free" : "api");
```

- [ ] **Step 6: Conditionally render the tab switcher**

Wrap the `SegmentedControl.Root` block (lines 241-261) in a conditional:

```typescript
{hasFreeModels && (
  <SegmentedControl.Root
    size="1"
    value={source}
    onValueChange={(v) => {
      setSource(v as "free" | "api");
      setSelectedIds(new Set());
    }}
  >
    <SegmentedControl.Item value="free">
      {t(
        "settings.postProcessing.models.selectModel.sourceBuiltin",
        "Free",
      )}
    </SegmentedControl.Item>
    <SegmentedControl.Item value="api">
      {t(
        "settings.postProcessing.models.selectModel.sourceOfficial",
        "API",
      )}
    </SegmentedControl.Item>
  </SegmentedControl.Root>
)}
```

- [ ] **Step 7: Show provider label on free model cards**

In the model card rendering (inside the `filteredOptions.map` callback), find the Row 2 section with `model.capabilities`. Replace:

```typescript
{model.capabilities && (
  <Text size="1" className="text-(--gray-8)">
    {model.capabilities}
  </Text>
)}
```

with:

```typescript
{model.capabilities && (
  <Text size="1" className="text-(--gray-8)">
    {model.capabilities}
  </Text>
)}
{source === "free" && templateMeta?.label && (
  <Text size="1" className="text-(--gray-8)">
    · {templateMeta.label}
  </Text>
)}
```

- [ ] **Step 8: Verify no TypeScript errors**

Run: `cd /Users/zac/code/github/asr/Handy && npx tsc --noEmit --pretty 2>&1 | head -20`
Expected: No errors.

- [ ] **Step 9: Manual smoke test**

Run: `cd /Users/zac/code/github/asr/Handy && bun tauri dev`

Test scenarios:

1. Select gitee provider → open Add Model → both tabs visible, "内置免费模型" default, cards show "Gitee AI" label
2. Select xingchen provider → open Add Model → both tabs visible, cards show "讯飞星辰" label
3. Select openai provider → open Add Model → no tab switcher, only API model list shown
4. Select a custom provider → open Add Model → no tab switcher, only API model list shown

- [ ] **Step 10: Commit**

```bash
git add src/components/settings/post-processing/dialogs/AddModelDialog.tsx
git commit -m "Use provider template metadata for free model display and tab visibility"
```
