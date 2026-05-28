# History Error Indicator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface polish + ASR-empty failures on each Dashboard history item via an amber ⚠️ icon in the action area, with hover tooltip + click-to-expand inline detail panel, plus a "show only failures" toggle in the Dashboard top toolbar.

**Architecture:** Extend the `HistoryEntry` Rust struct with a nullable `error_summary: HistoryError` field. Populate it via a LEFT JOIN against `pipeline_decisions` (newest row per `history_id`) inside the existing `get_history_entries` / `get_history_entries_paginated` queries, with a fallback rule that maps empty `transcription_text` to an `asr_empty` error. No DB schema changes. The frontend reads `entry.error_summary` and renders a conditional IconButton with a collapsible detail panel; the toolbar filter is client-side.

**Tech Stack:** Rust + rusqlite + specta (backend), React + TypeScript + Radix UI + Zustand (frontend), Tauri 2.x IPC.

**Spec:** `docs/specs/2026-05-28-history-error-indicator.spec.md`

---

## File Structure

**Create:** none

**Modify:**

- `src-tauri/src/managers/history.rs` — `HistoryError` struct, `HistoryEntry.error_summary` field, both query functions
- `src/bindings.ts` — regenerated (auto by specta)
- `src/components/settings/dashboard/dashboardTypes.ts` — frontend `HistoryEntry` + new `HistoryError` interface
- `src/components/settings/dashboard/DashboardEntryCard.tsx` — ⚠️ IconButton + inline error panel
- `src/components/settings/dashboard/Dashboard.tsx` — toolbar Switch + client-side filter
- `src/i18n/locales/en/translation.json` + `src/i18n/locales/zh/translation.json` — new copy keys

**Out of scope (per spec §禁止):**

- `pipeline_decisions` / `llm_call_log` table schema
- polish/ASR/LLM call paths
- New Tauri commands
- Retry / reprocess buttons
- Toast / modal surfaces

---

## Task 1: Add `HistoryError` struct + extend `HistoryEntry`

**Files:**

- Modify: `src-tauri/src/managers/history.rs` (struct definitions near line 465)

- [ ] **Step 1: Add the `HistoryError` struct above `HistoryEntry`**

Insert immediately before `pub struct HistoryEntry {` (around line 465):

```rust
/// Error summary attached to a HistoryEntry. Populated by LEFT JOIN against
/// `pipeline_decisions` + an `transcription_text=''` fallback. See
/// docs/specs/2026-05-28-history-error-indicator.spec.md §决策.
#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub struct HistoryError {
    /// "polish" or "asr"
    pub stage: String,
    /// Raw error_type string from pipeline_decisions (e.g. "llm_timeout",
    /// "llm_api_error") or the constant "asr_empty" for ASR fallback.
    pub error_type: String,
    /// Full error_detail string from pipeline_decisions, or None for asr_empty.
    pub detail: Option<String>,
    /// Model id captured when the error occurred (selected_model_id for polish,
    /// asr_model for ASR), or None if unavailable.
    pub model: Option<String>,
}
```

Verify the `specta::Type` derive works by checking that nearby structs (e.g., `HistoryEntry`) use it. If they use `specta::Type` without a `use` statement, follow the same pattern.

- [ ] **Step 2: Add `error_summary` field to `HistoryEntry`**

Find `pub struct HistoryEntry {` and add the new field at the end, just before the closing brace:

```rust
    pub post_process_rejected: Option<i64>,
    pub deleted: bool,
    /// Populated from pipeline_decisions JOIN or asr_empty fallback;
    /// null when the entry has no recorded failure.
    pub error_summary: Option<HistoryError>,
}
```

- [ ] **Step 3: Verify backend compiles**

```bash
rtk cargo check --manifest-path /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml -p votype 2>&1 | tail -10
```

Expected: compile errors at the `HistoryEntry { ... }` literal sites that construct entries in `get_history_entries` (~line 1657) and `get_history_entries_paginated` (~line 1740) saying "missing field `error_summary`". This is the TDD-style red state we want — it signals exactly the two sites Task 2 / 3 must update.

- [ ] **Step 4: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src-tauri/src/managers/history.rs
git -C /Users/zac/code/github/asr/Handy commit -m "Add HistoryError struct and error_summary field on HistoryEntry"
```

---

## Task 2: Populate `error_summary` in `get_history_entries_paginated`

**Files:**

- Modify: `src-tauri/src/managers/history.rs` (around line 1696-1780)

This is the function used by the Dashboard. It must produce the `error_summary` value per spec §决策 priority rules.

- [ ] **Step 1: Add helper that resolves `error_summary` from row fields**

Insert this helper inside `impl HistoryManager`, just above `get_history_entries_paginated` (around line 1695). The helper takes the JOINed pipeline_decisions fields (which may all be NULL) and the transcription_text/asr_model fields, and returns `Option<HistoryError>`:

```rust
fn resolve_error_summary(
    pd_error_type: Option<String>,
    pd_error_detail: Option<String>,
    pd_selected_model_id: Option<String>,
    transcription_text: &str,
    asr_model: Option<&str>,
) -> Option<HistoryError> {
    // Priority 1: pipeline_decisions has a real error_type.
    if let Some(et) = pd_error_type {
        if !et.is_empty() {
            return Some(HistoryError {
                stage: "polish".to_string(),
                error_type: et,
                detail: pd_error_detail,
                model: pd_selected_model_id,
            });
        }
    }
    // Priority 2: ASR transcription is literally empty.
    if transcription_text.is_empty() {
        return Some(HistoryError {
            stage: "asr".to_string(),
            error_type: "asr_empty".to_string(),
            detail: None,
            model: asr_model.map(String::from),
        });
    }
    // Priority 3: no error.
    None
}
```

- [ ] **Step 2: Rewrite the SQL query in `get_history_entries_paginated` to LEFT JOIN pipeline_decisions**

Find the `let query_sql = format!(...)` (around line 1731-1734). Replace the SELECT statement with one that LEFT JOINs the latest pipeline_decisions row per history_id.

SQLite-compatible approach: use a correlated subquery to pick the latest `pipeline_decisions.id` per `history_id`, then join on that. Replace the entire `let query_sql = format!(...)`:

```rust
        // Build paginated query. LEFT JOIN the LATEST pipeline_decisions row
        // (by id) per history_id so we surface the most recent error if any.
        let query_sql = format!(
            "SELECT th.id, th.file_name, th.timestamp, th.saved, th.title,
                    th.transcription_text, th.streaming_text, th.streaming_asr_model,
                    th.post_processed_text, th.post_process_prompt, th.post_process_prompt_id,
                    th.post_process_model, th.duration_ms, th.char_count, th.corrected_char_count,
                    th.transcription_ms, th.language, th.asr_model, th.app_name, th.window_title,
                    th.post_process_history, th.token_count, th.llm_call_count,
                    th.post_process_rejected, th.deleted,
                    pd.error_type AS pd_error_type,
                    pd.error_detail AS pd_error_detail,
                    pd.selected_model_id AS pd_selected_model_id
             FROM transcription_history th
             LEFT JOIN pipeline_decisions pd
                ON pd.id = (
                    SELECT id FROM pipeline_decisions
                    WHERE history_id = th.id
                    ORDER BY id DESC
                    LIMIT 1
                )
             {} ORDER BY th.timestamp DESC LIMIT {} OFFSET {}",
            where_clause.replace("timestamp", "th.timestamp"),
            limit,
            offset
        );
```

Note that the `where_clause` built earlier references the bare column name `timestamp`; after JOIN we must qualify it as `th.timestamp`, hence the `.replace(...)`. Similarly fix the COUNT query just above:

```rust
        // Get total count (no JOIN needed for counting transcription rows)
        let count_sql = format!(
            "SELECT COUNT(*) FROM transcription_history {}",
            where_clause
        );
```

The COUNT path stays as-is (no JOIN), because we're counting transcription rows, not pipeline_decisions rows. The `where_clause` for COUNT uses unqualified `timestamp` (no JOIN aliases there).

- [ ] **Step 3: Update the `map_row` closure to read the JOINed fields + populate `error_summary`**

Find the `let map_row = |row: &rusqlite::Row| -> rusqlite::Result<HistoryEntry> {` block (around line 1739). Add `error_summary` to the returned struct literal. The map_row closure body becomes:

```rust
        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<HistoryEntry> {
            let transcription_text: String = row.get("transcription_text")?;
            let asr_model: Option<String> = row.get("asr_model")?;
            let pd_error_type: Option<String> = row.get("pd_error_type")?;
            let pd_error_detail: Option<String> = row.get("pd_error_detail")?;
            let pd_selected_model_id: Option<String> = row.get("pd_selected_model_id")?;

            let error_summary = Self::resolve_error_summary(
                pd_error_type,
                pd_error_detail,
                pd_selected_model_id,
                &transcription_text,
                asr_model.as_deref(),
            );

            Ok(HistoryEntry {
                id: row.get("id")?,
                file_name: row.get("file_name")?,
                timestamp: row.get("timestamp")?,
                saved: row.get("saved")?,
                title: row.get("title")?,
                transcription_text,
                streaming_text: row.get("streaming_text")?,
                streaming_asr_model: row.get("streaming_asr_model")?,
                post_processed_text: row.get("post_processed_text")?,
                post_process_prompt: row.get("post_process_prompt")?,
                post_process_prompt_id: row.get("post_process_prompt_id")?,
                post_process_model: row.get("post_process_model")?,
                duration_ms: row.get("duration_ms")?,
                char_count: row.get("char_count")?,
                corrected_char_count: row.get("corrected_char_count")?,
                transcription_ms: row.get("transcription_ms")?,
                language: row.get("language")?,
                asr_model,
                app_name: row.get("app_name")?,
                window_title: row.get("window_title")?,
                post_process_history: row.get("post_process_history")?,
                token_count: row.get("token_count")?,
                llm_call_count: row.get("llm_call_count")?,
                post_process_rejected: row.get("post_process_rejected")?,
                deleted: row.get("deleted")?,
                error_summary,
            })
        };
```

Notice `transcription_text` and `asr_model` are pulled into local bindings first because they're consumed by `resolve_error_summary` (which takes `&str` / `Option<&str>`) and then moved into the struct.

- [ ] **Step 4: Build to verify the SQL aliases and field types line up**

```bash
rtk cargo check --manifest-path /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml -p votype 2>&1 | tail -15
```

Expected: clean build. If there's a "no such column: pd_error_type" runtime error at test-time, the alias quoting in the SQL above is the place to inspect.

- [ ] **Step 5: Verify no Clippy warnings introduced**

```bash
rtk cargo clippy --manifest-path /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml -p votype --all-targets 2>&1 | grep -E "history\.rs" | head -10
```

Expected: empty (no warnings cite history.rs lines we touched).

- [ ] **Step 6: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src-tauri/src/managers/history.rs
git -C /Users/zac/code/github/asr/Handy commit -m "LEFT JOIN pipeline_decisions into paginated history query"
```

---

## Task 3: Same JOIN treatment for `get_history_entries`

**Files:**

- Modify: `src-tauri/src/managers/history.rs:1650-1692`

The non-paginated function is used by other callers (search the repo to confirm). For API consistency, `error_summary` should be populated here too.

- [ ] **Step 1: Replace the SQL in `get_history_entries`**

Find `pub async fn get_history_entries(&self) -> Result<Vec<HistoryEntry>>` (around line 1650) and update its body. Replace the existing `let mut stmt = conn.prepare(...)` with the JOIN version:

```rust
    pub async fn get_history_entries(&self) -> Result<Vec<HistoryEntry>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT th.id, th.file_name, th.timestamp, th.saved, th.title,
                    th.transcription_text, th.streaming_text, th.streaming_asr_model,
                    th.post_processed_text, th.post_process_prompt, th.post_process_prompt_id,
                    th.post_process_model, th.duration_ms, th.char_count, th.corrected_char_count,
                    th.transcription_ms, th.language, th.asr_model, th.app_name, th.window_title,
                    th.post_process_history, th.token_count, th.llm_call_count,
                    th.post_process_rejected, th.deleted,
                    pd.error_type AS pd_error_type,
                    pd.error_detail AS pd_error_detail,
                    pd.selected_model_id AS pd_selected_model_id
             FROM transcription_history th
             LEFT JOIN pipeline_decisions pd
                ON pd.id = (
                    SELECT id FROM pipeline_decisions
                    WHERE history_id = th.id
                    ORDER BY id DESC
                    LIMIT 1
                )
             ORDER BY th.timestamp DESC"
        )?;

        let rows = stmt.query_map([], |row| {
            let transcription_text: String = row.get("transcription_text")?;
            let asr_model: Option<String> = row.get("asr_model")?;
            let pd_error_type: Option<String> = row.get("pd_error_type")?;
            let pd_error_detail: Option<String> = row.get("pd_error_detail")?;
            let pd_selected_model_id: Option<String> = row.get("pd_selected_model_id")?;

            let error_summary = Self::resolve_error_summary(
                pd_error_type,
                pd_error_detail,
                pd_selected_model_id,
                &transcription_text,
                asr_model.as_deref(),
            );

            Ok(HistoryEntry {
                id: row.get("id")?,
                file_name: row.get("file_name")?,
                timestamp: row.get("timestamp")?,
                saved: row.get("saved")?,
                title: row.get("title")?,
                transcription_text,
                streaming_text: row.get("streaming_text")?,
                streaming_asr_model: row.get("streaming_asr_model")?,
                post_processed_text: row.get("post_processed_text")?,
                post_process_prompt: row.get("post_process_prompt")?,
                post_process_prompt_id: row.get("post_process_prompt_id")?,
                post_process_model: row.get("post_process_model")?,
                duration_ms: row.get("duration_ms")?,
                char_count: row.get("char_count")?,
                corrected_char_count: row.get("corrected_char_count")?,
                transcription_ms: row.get("transcription_ms")?,
                language: row.get("language")?,
                asr_model,
                app_name: row.get("app_name")?,
                window_title: row.get("window_title")?,
                post_process_history: row.get("post_process_history")?,
                token_count: row.get("token_count")?,
                llm_call_count: row.get("llm_call_count")?,
                post_process_rejected: row.get("post_process_rejected")?,
                deleted: row.get("deleted")?,
                error_summary,
            })
        })?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }

        Ok(entries)
    }
```

- [ ] **Step 2: Verify**

```bash
rtk cargo check --manifest-path /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml -p votype 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 3: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src-tauri/src/managers/history.rs
git -C /Users/zac/code/github/asr/Handy commit -m "Apply same JOIN to non-paginated get_history_entries"
```

---

## Task 4: Regenerate `src/bindings.ts`

**Files:**

- Modify (generated): `src/bindings.ts`

Specta export runs inside `pub fn run()` under `#[cfg(debug_assertions)]`. Launching the app briefly triggers it.

- [ ] **Step 1: Launch tauri dev to regenerate bindings, then kill**

```bash
cd /Users/zac/code/github/asr/Handy && bun tauri dev > /tmp/votype-bindings-history.log 2>&1 &
sleep 30
pkill -f "tauri dev" ; pkill -f "votype" ; pkill -f "cargo-tauri"
sleep 2
ps aux | grep -E "tauri|votype" | grep -v grep | head -3
```

Expected: no leftover processes after kill.

- [ ] **Step 2: Verify bindings include `HistoryError`**

```bash
rtk grep "HistoryError\|error_summary" /Users/zac/code/github/asr/Handy/src/bindings.ts | head -10
```

Expected: at least one match showing `HistoryError` type definition and `error_summary` field on `HistoryEntry`.

- [ ] **Step 3: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src/bindings.ts
git -C /Users/zac/code/github/asr/Handy commit -m "Regenerate bindings for HistoryEntry.error_summary"
```

---

## Task 5: Sync frontend `dashboardTypes.ts`

**Files:**

- Modify: `src/components/settings/dashboard/dashboardTypes.ts`

The Dashboard uses a manually-maintained TS type (not the specta `bindings.ts`). Keep both in sync.

- [ ] **Step 1: Add `HistoryError` interface + `error_summary` field**

Edit `src/components/settings/dashboard/dashboardTypes.ts`. Add the interface and field:

```typescript
export interface HistoryError {
  /** "polish" or "asr" */
  stage: string;
  /** Raw error_type from pipeline_decisions, or "asr_empty" for ASR fallback. */
  error_type: string;
  /** Full error_detail string, or null for asr_empty. */
  detail?: string | null;
  /** Model id captured when the error occurred, if any. */
  model?: string | null;
}

export interface HistoryEntry {
  id: number;
  file_name: string;
  timestamp: number;
  saved: boolean;
  title: string;
  transcription_text: string;
  streaming_text?: string | null;
  streaming_asr_model?: string | null;
  post_processed_text?: string | null;
  post_process_prompt?: string | null;
  post_process_prompt_id?: string | null;
  post_process_model?: string | null;
  duration_ms?: number | null;
  char_count?: number | null;
  corrected_char_count?: number | null;
  transcription_ms?: number | null;
  language?: string | null;
  asr_model?: string | null;
  app_name?: string | null;
  window_title?: string | null;
  post_process_history?: string | null;
  token_count?: number | null;
  llm_call_count?: number | null;
  deleted: boolean;
  /** Populated by backend when this entry has a recorded failure (polish chain
   * error, 10s pipeline timeout, or empty ASR result). null otherwise. */
  error_summary?: HistoryError | null;
}
```

The existing `HistoryEntry` interface (lines 8-33) is preserved except for the new last field.

- [ ] **Step 2: Verify TS compiles**

```bash
cd /Users/zac/code/github/asr/Handy && rtk bun run build 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 3: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src/components/settings/dashboard/dashboardTypes.ts
git -C /Users/zac/code/github/asr/Handy commit -m "Add HistoryError + error_summary to dashboard frontend type"
```

---

## Task 6: Add ⚠️ IconButton to `DashboardEntryCard`

**Files:**

- Modify: `src/components/settings/dashboard/DashboardEntryCard.tsx`

The entry card has a top-right floating action area with `Quick Insert` and `Edit` icon buttons. We add a conditional ⚠️ alert icon **before** Quick Insert when `entry.error_summary` is non-null. Click toggles an inline expand state. Tooltip shows a one-line summary.

- [ ] **Step 1: Read the file and identify the action area + state hooks**

```bash
rtk read /Users/zac/code/github/asr/Handy/src/components/settings/dashboard/DashboardEntryCard.tsx | head -50
rtk grep -n "useState\|opacity-0 group-hover:opacity-100\|absolute top-2 right-2" /Users/zac/code/github/asr/Handy/src/components/settings/dashboard/DashboardEntryCard.tsx | head -10
```

You should see two action-button clusters at `absolute top-2 right-2` (one inside the Tabs view, one outside). Both need the ⚠️ icon. Identify the existing imports — we'll add `IconAlertTriangle` to the `@tabler/icons-react` import line, and add a `showErrorDetail` boolean state.

- [ ] **Step 2: Add the import + state**

At the top of the file, find the `@tabler/icons-react` import (something like `import { IconSend, IconCopy, IconPencil, ... } from "@tabler/icons-react";`). Add `IconAlertTriangle` to the list (alphabetical order).

Near the existing `useState` hooks inside the component body, add:

```typescript
const [showErrorDetail, setShowErrorDetail] = useState(false);
```

- [ ] **Step 3: Compute the tooltip summary string**

Just below the state declarations (before the JSX return), add:

```typescript
const errorTooltip = useMemo<string | null>(() => {
  const err = entry.error_summary;
  if (!err) return null;
  if (err.stage === "polish") {
    const isTimeout =
      err.error_type === "llm_timeout" || err.error_type === "timeout";
    return isTimeout
      ? t("dashboard.error.polishTimeout", "Polish 超时")
      : t("dashboard.error.polishFailed", "Polish 失败");
  }
  if (err.stage === "asr") {
    return t("dashboard.error.asrEmpty", "ASR 空结果");
  }
  return t("dashboard.error.unknown", "未知错误");
}, [entry.error_summary, t]);
```

This requires `useMemo` to be imported from `react`. If not already imported, add it to the existing `import { ... } from "react";` line.

- [ ] **Step 4: Add the conditional ⚠️ IconButton in BOTH action clusters**

Find both `<Box className="absolute top-2 right-2 opacity-0 group-hover:opacity-100 ...">` blocks (one inside Tabs view, one for the simpler non-tabs view). Inside each `<Flex gap="1">` action row, add this BEFORE the existing `<Tooltip content={... quickInsert ...}>` block:

```tsx
{
  entry.error_summary && (
    <Tooltip content={errorTooltip ?? ""}>
      <IconButton
        variant="ghost"
        size="1"
        onClick={() => setShowErrorDetail((v) => !v)}
        className="text-amber-500 hover:bg-amber-500/10 cursor-pointer"
      >
        <IconAlertTriangle size={14} />
      </IconButton>
    </Tooltip>
  );
}
```

Use the same `<Tooltip>` and `<IconButton>` primitives that the surrounding code uses (don't introduce new component libraries).

- [ ] **Step 5: Add the inline error detail panel**

Find a natural spot in the card body, just AFTER the main text/Tabs content but BEFORE the action buttons cluster (so the panel appears below the transcription text). The simplest location is right after the `<Tabs.Root>` block closes (or after `<Text>` for non-tabs branch). Add:

```tsx
{
  entry.error_summary && showErrorDetail && (
    <Box
      mt="2"
      p="3"
      className="rounded-md border border-amber-500/40 bg-amber-500/5"
    >
      <Flex direction="column" gap="1">
        <Text
          size="2"
          weight="medium"
          className="text-amber-700 dark:text-amber-300"
        >
          {entry.error_summary.stage === "polish"
            ? t("dashboard.error.detail.stagePolish", "Polish")
            : t("dashboard.error.detail.stageAsr", "ASR")}{" "}
          · {entry.error_summary.error_type}
        </Text>
        {entry.error_summary.model && (
          <Text size="1" color="gray">
            {t("dashboard.error.detail.model", "模型")}:{" "}
            {entry.error_summary.model}
          </Text>
        )}
        {entry.error_summary.detail && (
          <Text
            size="1"
            color="gray"
            className="whitespace-pre-wrap font-mono break-all"
          >
            {entry.error_summary.detail}
          </Text>
        )}
      </Flex>
    </Box>
  );
}
```

The panel must be inside the same parent that wraps the transcription text. If the card has two render branches (Tabs vs simple Text), add the panel in BOTH branches so it shows regardless.

- [ ] **Step 6: Verify build**

```bash
cd /Users/zac/code/github/asr/Handy && rtk bun run build 2>&1 | tail -10
```

Expected: clean. Common pitfalls:

- "Cannot find name 'IconAlertTriangle'" → add to import
- "Cannot find name 'useMemo'" → add to react import
- "Property 'error_summary' does not exist on type 'HistoryEntry'" → Task 5 must be merged first

- [ ] **Step 7: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src/components/settings/dashboard/DashboardEntryCard.tsx
git -C /Users/zac/code/github/asr/Handy commit -m "Render error indicator and inline detail on history entry cards"
```

---

## Task 7: Add "show only failures" toggle in Dashboard toolbar

**Files:**

- Modify: `src/components/settings/dashboard/Dashboard.tsx`

The Dashboard owns the detail entries state. Add a client-side filter switch.

- [ ] **Step 1: Find the toolbar area**

Read the Dashboard.tsx file and find where the date-range selector / preset buttons live (the visual area where filters belong). Typically a `<Flex>` near the top of the rendered JSX.

```bash
rtk grep -n "preset\|setSelection\|date-range\|select\|<Flex" /Users/zac/code/github/asr/Handy/src/components/settings/dashboard/Dashboard.tsx | head -20
```

- [ ] **Step 2: Add the `errorsOnly` state**

Near the other `useState` declarations at the top of the component (around line 27-30 where `allEntries` and `detailEntries` are declared):

```typescript
const [errorsOnly, setErrorsOnly] = useState(false);
```

- [ ] **Step 3: Compute the filtered list**

Find the point where `detailEntries` is rendered (passed to `VirtualDetailsList` or similar). Replace the prop with a memoized filtered version. Add this `useMemo` (importing it from React if not already imported):

```typescript
const filteredDetailEntries = useMemo(
  () =>
    errorsOnly
      ? detailEntries.filter((e) => e.error_summary != null)
      : detailEntries,
  [detailEntries, errorsOnly],
);
```

Then update the JSX to pass `filteredDetailEntries` wherever `detailEntries` was passed (typically `<VirtualDetailsList entries={...} />` or similar).

- [ ] **Step 4: Add the Switch in the toolbar**

Find the toolbar `<Flex>` (the one containing date range presets / selectors). Add a Switch with label at the end of that row. The exact pattern depends on what's already there — for example:

```tsx
<Flex align="center" gap="2">
  <Switch size="1" checked={errorsOnly} onCheckedChange={setErrorsOnly} />
  <Text size="1" color="gray">
    {t("dashboard.filter.errorsOnly", "仅显示失败")}
  </Text>
</Flex>
```

Import `Switch` from `@radix-ui/themes` if not already imported. Look at the existing imports at the top — likely `Switch` is already there since the project uses Radix Themes widely.

- [ ] **Step 5: Verify build**

```bash
cd /Users/zac/code/github/asr/Handy && rtk bun run build 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src/components/settings/dashboard/Dashboard.tsx
git -C /Users/zac/code/github/asr/Handy commit -m "Add Dashboard errorsOnly toggle with client-side filter"
```

---

## Task 8: i18n keys (en + zh)

**Files:**

- Modify: `src/i18n/locales/en/translation.json`
- Modify: `src/i18n/locales/zh/translation.json`

- [ ] **Step 1: Add keys under existing `dashboard` block**

Open each translation file and locate the `"dashboard"` object. Add (or extend) the nested keys. For `zh`:

```json
"dashboard": {
  ...existing keys...,
  "error": {
    "polishTimeout": "Polish 超时",
    "polishFailed": "Polish 失败",
    "asrEmpty": "ASR 空结果",
    "unknown": "未知错误",
    "detail": {
      "stagePolish": "Polish",
      "stageAsr": "ASR",
      "model": "模型"
    }
  },
  "filter": {
    "errorsOnly": "仅显示失败"
  }
}
```

For `en`:

```json
"dashboard": {
  ...existing keys...,
  "error": {
    "polishTimeout": "Polish timed out",
    "polishFailed": "Polish failed",
    "asrEmpty": "ASR returned empty",
    "unknown": "Unknown error",
    "detail": {
      "stagePolish": "Polish",
      "stageAsr": "ASR",
      "model": "Model"
    }
  },
  "filter": {
    "errorsOnly": "Failures only"
  }
}
```

If `dashboard.error` or `dashboard.filter` already exist, merge into them — do not clobber.

- [ ] **Step 2: Verify JSON parses**

```bash
python3 -m json.tool /Users/zac/code/github/asr/Handy/src/i18n/locales/en/translation.json > /dev/null && echo "en OK"
python3 -m json.tool /Users/zac/code/github/asr/Handy/src/i18n/locales/zh/translation.json > /dev/null && echo "zh OK"
```

Expected: both "OK".

- [ ] **Step 3: Verify build (catches missing key warnings)**

```bash
cd /Users/zac/code/github/asr/Handy && rtk bun run build 2>&1 | tail -8
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add src/i18n/locales/en/translation.json src/i18n/locales/zh/translation.json
git -C /Users/zac/code/github/asr/Handy commit -m "Add i18n keys for dashboard error indicator and filter toggle"
```

---

## Task 9: Manual BDD verification

**Files:** None (manual)

Walk through each of the 8 acceptance scenarios from `docs/specs/2026-05-28-history-error-indicator.spec.md`. Run the app once:

```bash
cd /Users/zac/code/github/asr/Handy && bun tauri dev
```

- [ ] **Scenario 1 — happy_path_no_error_no_icon**: trigger a normal recording that polishes successfully. Open Dashboard, verify no ⚠️ icon appears on that entry. Inspect `entry.error_summary` via React DevTools — should be null/undefined.

- [ ] **Scenario 2 — polish_timeout_shows_icon_and_detail**: induce a 10s pipeline timeout (e.g., configure a slow/unreachable LLM provider). After recording, find the entry in Dashboard. Verify amber ⚠️ icon appears, hover tooltip shows "Polish 超时", click expands a panel showing `bypass_reason='timeout'` content.

- [ ] **Scenario 3 — polish_404_provider_error**: configure a provider with a non-existent model id (e.g., `qwen-fake-model-id`). Record, check icon shows, tooltip says "Polish 失败", panel shows the 404 error_detail string with provider info.

- [ ] **Scenario 4 — asr_empty_shows_icon**: harder to reproduce naturally. To force: manually `UPDATE transcription_history SET transcription_text='' WHERE id=<row>` in the SQLite db, then refresh the Dashboard. Verify ⚠️ shows with `stage="asr", error_type="asr_empty"`.

- [ ] **Scenario 5 — filter_toggle_shows_only_errors**: with mixed entries (some with errors, some without), flip the "仅显示失败" toggle. Verify the list collapses to only entries with ⚠️ icons. Untoggle to restore full list.

- [ ] **Scenario 6 — multiple_pipeline_decisions_pick_latest**: harder to reproduce naturally. Either induce a retry via the review window's rerun feature, or insert a second `pipeline_decisions` row manually with a later `id` and `error_type=NULL`. Verify the ⚠️ disappears (latest decision wins).

- [ ] **Scenario 7 — typescript_types_synced**: already covered by `bun run build` clean status. Spot-check `src/bindings.ts` for `HistoryError` and `error_summary`.

- [ ] **Scenario 8 — edge_case_pipeline_decisions_no_history_id**: confirm Dashboard renders without errors when the database has older `pipeline_decisions` rows with `history_id=NULL`. The LEFT JOIN correlated subquery naturally ignores these.

- [ ] **Step: Commit any incidental fixes**

If a scenario reveals a bug, fix it and commit:

```bash
git -C /Users/zac/code/github/asr/Handy status
# stage and commit any fixes individually
```

---

## Task 10: Final lint + warning sweep + format

**Files:** any modified during impl

Per CLAUDE.md and `feedback_fix_all_warnings.md`: zero new warnings before merge.

- [ ] **Step 1: Backend clippy (filter to changed files)**

```bash
cd /Users/zac/code/github/asr/Handy && rtk cargo clippy --manifest-path src-tauri/Cargo.toml -p votype --all-targets 2>&1 | grep "managers/history\.rs" | head -10
```

Expected: empty.

- [ ] **Step 2: Backend build**

```bash
rtk cargo build --manifest-path /Users/zac/code/github/asr/Handy/src-tauri/Cargo.toml -p votype 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 3: Frontend build + type-check**

```bash
cd /Users/zac/code/github/asr/Handy && rtk bun run build 2>&1 | tail -8
```

Expected: clean.

- [ ] **Step 4: Format**

```bash
cd /Users/zac/code/github/asr/Handy && bun format
```

- [ ] **Step 5: Commit any format-only changes**

```bash
git -C /Users/zac/code/github/asr/Handy add -A
git -C /Users/zac/code/github/asr/Handy diff --cached --stat
git -C /Users/zac/code/github/asr/Handy commit -m "Polish formatting for history error indicator feature" || true
```

(The `|| true` keeps the step idempotent if there's nothing to commit.)

---

## Task 11: Append Implementation Deviations to spec

**Files:**

- Modify: `docs/specs/2026-05-28-history-error-indicator.spec.md`

- [ ] **Step 1: Fill the `## 实施偏差` table**

Open the spec. Replace the placeholder row with one row per deviation observed during impl. If none, a single row "None observed — implementation matched spec exactly."

Sample template:

```markdown
| 原计划 | 实际实现 | 原因 |
| ------ | -------- | ---- |
| ...    | ...      | ...  |
```

- [ ] **Step 2: Commit**

```bash
git -C /Users/zac/code/github/asr/Handy add docs/specs/2026-05-28-history-error-indicator.spec.md
git -C /Users/zac/code/github/asr/Handy commit -m "Record implementation deviations for history error indicator"
```

---

## Self-Review Summary

Mapping spec requirements to tasks:

- **Spec §约束 "不改 DB schema"** → Task 2/3 use LEFT JOIN, no migrations
- **Spec §约束 "不修改 polish / ASR / LLM 调用路径"** → no edits to pipeline.rs / fallback.rs / etc.
- **Spec §约束 "V1 只覆盖 Polish + ASR"** → Task 1 `HistoryError`, Task 2 `resolve_error_summary` only handles these two
- **Spec §约束 "一条 history 最多一条 error_summary，取最新"** → Task 2 SQL `ORDER BY pd.id DESC LIMIT 1`
- **Spec §约束 "Filter 客户端过滤"** → Task 7 `useMemo` filter
- **Spec §决策 "error_summary 三级优先级"** → Task 2 `resolve_error_summary` function
- **Spec §决策 "stage 小写"** → Task 2 helper writes `"polish"` / `"asr"`
- **Spec §决策 "右侧动作区 amber ⚠️"** → Task 6 IconButton with `text-amber-500`
- **Spec §决策 "hover tooltip + 点击行内展开"** → Task 6 Tooltip + showErrorDetail state + Box panel
- **Spec §决策 "toggle 位置 toolbar"** → Task 7 Switch in Dashboard toolbar
- **Spec §验收 1-8** → Task 9 sub-tasks 1-8
