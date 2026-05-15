# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Votype is a cross-platform desktop application built with Tauri for speech-to-text transcription with a focus on efficiency, privacy, and system integration. It supports local and online ASR, AI-powered post-processing with smart routing, and a multi-model comparison system.

## Architecture

The application adopts a hybrid architecture:

- **Frontend (`src/`)**: Web-based UI using React, TypeScript, and Vite
- **Backend (`src-tauri/`)**: Rust application handling system-level operations and native APIs

### Frontend-Backend Communication

- Frontend calls backend using Tauri's `invoke` mechanism: `import { invoke } from '@tauri-apps/api/core'`
- Backend commands are defined with `#[tauri::command]` attribute and registered in `src-tauri/src/lib.rs`
- Backend emits events to frontend using `app.emit("event-name", payload)`

### Key Technologies

**Frontend:**

- React 18, TypeScript, Vite 6
- Radix UI for accessible components, styled with Tailwind CSS 4
- Zustand for state management
- Sonner for notifications, Zod for validation
- TipTap for rich text editing (review window)

**Backend:**

- Rust with Tauri 2.x framework
- Tauri plugins for native functionality (autostart, clipboard, filesystem, global shortcuts, etc.)
- Audio processing: `cpal`, `vad-rs`, `rubato`
- Speech-to-text: `transcribe-rs`, `async-openai`

## Project Structure

- `src/`: Frontend source
  - `src/components/`: UI components with `src/components/ui/` for primitives
  - `src/hooks/`, `src/stores/`: React hooks and Zustand stores
  - `src/lib/`: Utility functions
  - `src/review/`: Review/confidence window (separate Tauri webview)
- `src-tauri/`: Rust backend
  - `src-tauri/src/actions/`: Core actions (transcribe, post_process/)
  - `src-tauri/src/actions/post_process/`: Unified pipeline (pipeline.rs, routing.rs, extensions.rs, core.rs, recent_context.rs, prompt_builder.rs, reference_resolver.rs)
  - `src-tauri/src/managers/`: Business logic managers (history, model, transcription, hotword, prompt, llm_metrics, pipeline_log, etc.)
  - `src-tauri/src/shortcut/`: Shortcut handlers and review commands
  - `src-tauri/src/review_window.rs`: Review window lifecycle and state
  - `src-tauri/resources/prompts/`: AI prompt templates (loaded at runtime)
  - `src-tauri/resources/skills/`: Built-in skill definitions
- `docs/`: Project documentation, specs, and plans

## Development Commands

- `bun dev`: Start frontend development server
- `bun tauri dev`: Run Tauri app in development mode
- `bun build`: Build frontend
- `bun tauri build`: Build production Tauri desktop app
- `bun preview`: Preview built frontend assets
- `bun format`: Format both frontend and backend code
- `bun format:frontend`: Format frontend only
- `bun format:backend`: Format backend only

## Core Functionality

- **Speech-to-Text**: Local (SenseVoice, Whisper) and online (API) transcription
- **Unified Post-Processing Pipeline**: 4-step routing (history → intent → model selection → execution)
- **Smart Routing**: Intent model classifies text as PassThrough / LitePolish / FullPolish
- **Multi-Model Comparison**: Concurrent model execution with candidate ranking
- **Review Window**: Polish mode (single editor) and multi-candidate mode (comparison panels)
- **Voice Rewrite**: Spoken instructions to edit text in review window
- **Audio Management**: Microphone input/output device control
- **Model Management**: Download, delete, and select STT models
- **History**: Transcription history stored in SQLite database
- **Global Shortcuts**: Configurable hotkeys for transcription control
- **System Tray**: Application runs in system tray
- **Overlay**: Visual indicator during recording

## Post-Processing Pipeline

All post-processing flows through `unified_post_process()` in `pipeline.rs`:

```
Step 1: History exact match (short text + smart routing enabled)
Step 2: Intent analysis → PassThrough / LitePolish / FullPolish + needs_hotword + language
Step 3: Model selection → lightweight model / full model / multi-model
Step 4: Execute polish
```

**Bypass rules:**

- Text > `length_routing_threshold` → skip Steps 1-2, go to FullPolish
- Smart routing disabled → skip Steps 1-2
- ReviewRewrite mode → skip Steps 1-2 and multi-model

**Key types:**

- `PipelineResult` enum: Skipped, Cached, PassThrough, SingleModel, MultiModel, PendingSkillConfirmation
- `IntentDecision`: action + needs_hotword + language + token_count + model_id + provider_id + duration_ms

**Context enrichment (injected into user message by PromptBuilder):**

- `[session-context]`: Recent transcriptions from the same app (5-min window, 500-char budget, managed by `recent_context.rs`)
- `{{scenario-hint}}`: App-category-aware tone guidance (CodeEditor → preserve terms, IM → keep casual, Email → formal)
- `[asr-corrections]`: Known ASR error → correction pairs from hotword system

**Observability:**

- `pipeline_decisions` table: One row per pipeline run with step timing, routing decisions, error types (written by `PipelineLogManager`)
- `llm_call_log` table: Per-LLM-call metrics (model, provider, tokens, speed, error, is_fallback)
- `review_action` / `review_edit_distance` / `review_selected_candidate`: User feedback from review window

## Review Window

Two display modes based on candidate count:

- **Polish mode** (1 candidate): Large TipTap editor with diff, undo/redo, direct editing
- **Multi-candidate mode** (2+ candidates): Candidate panels with ranking, speed, comparison

**Voice rewrite flow:**

1. Voice key in review window → `freeze_review_editor_content_snapshot()` captures text
2. ASR produces spoken instruction
3. Pipeline uses `execute_votype_rewrite_prompt()` with frozen content as target
4. Result emitted via `review-window-rewrite-apply` event
5. `REVIEW_EDITOR_CONTENT` updated synchronously before emit (prevents race condition)

**ESC behavior:** Text modified → double-ESC required; no modification → single ESC closes.

## Prompt System

### Built-in Prompts

| ID                 | Name     | File                    |
| ------------------ | -------- | ----------------------- |
| `__PASS_THROUGH__` | 无需润色 | —                       |
| `__LITE_POLISH__`  | 轻量润色 | `system_lite_polish.md` |

### Prompt Files (`src-tauri/resources/prompts/`)

| File                       | Purpose                                             |
| -------------------------- | --------------------------------------------------- |
| `system_smart_routing.md`  | Intent analysis (action + needs_hotword + language) |
| `system_lite_polish.md`    | Lightweight ASR post-processing                     |
| `system_votype_rewrite.md` | Voice instruction rewrite prompt                    |

## UI Architecture

- Component-based using React functional components
- Built on Radix UI primitives with Tailwind CSS styling
- Zustand stores for global state management
- Focus on accessibility and reusability

## UI Component Rules

- **Do NOT use Radix Card:** Do not import `Card` from `@radix-ui/themes`. The project has a custom `Card` component at `src/components/ui/Card.tsx` that must be used instead. ESLint is configured to forbid importing Radix Card.
- **Custom Card:** Use `import { Card } from "@/components/ui/Card"` or relative path imports.
- **SettingsGroup:** Used for settings page group containers, includes border and shadow effects.

## Spec Writing Rules

- **Template:** All new feature specs must follow `docs/SPEC_TEMPLATE.md`.
- **Location:** `docs/specs/{date}-{feature-name}.spec.md`
- **Required Sections:** Intent, Constraints, Decisions, Boundaries (allow/forbid files), Acceptance Scenarios.
- **Acceptance Scenarios:** Must cover happy path, error path, and edge cases using Given/When/Then format.
- **Deviation Log:** After implementation, append an "Implementation Deviations" table recording actual vs planned differences.

## Git Commit Rules

- **Language:** English.
- **Format:** Standard best practices.
- **Summary:** Use a single, concise sentence in the imperative mood to describe the update. Do not use trailing punctuation.
- **Description:** Add a detailed description only if necessary to explain complex changes.

## AI Prompt Rules

- **External Files:** All AI prompts must be stored in external files at `src-tauri/resources/prompts/*.md`, NOT hardcoded in Rust code.
- **Runtime Loading:** Prompts are loaded at runtime using `PromptManager`, which first checks the user's data directory for customizations, then falls back to built-in resources.
- **Template Variables:** Use `{{variable_name}}` syntax for dynamic values in prompts. Available variables: `{{prompt}}`, `{{app-name}}`, `{{app-category}}`, `{{window-title}}`, `{{time}}`, `{{scenario-hint}}`.
- **User Customization:** Users can override built-in prompts by placing modified files in their app data directory.

## LLM Execution Layer

### API Surface (core.rs)

Four public entry points exist — use the right one:

| Function                            | Returns      | Use When                                         |
| ----------------------------------- | ------------ | ------------------------------------------------ |
| `execute_llm_request`               | Legacy tuple | Simple single-prompt calls (existing code)       |
| `execute_llm_request_with_messages` | Legacy tuple | Multi-message calls (existing code)              |
| `execute_llm_request_typed`         | `LlmResult`  | New code, caller handles errors                  |
| `execute_llm_request_with_retry`    | `LlmResult`  | New code, automatic retry for transient failures |

**For new code, prefer `execute_llm_request_with_retry`.** It handles Network errors, 429 rate limits, and 5xx server errors with up to 2 retries (budget ≤1.5s). Legacy wrappers delegate to the same `execute_llm_request_inner` internally.

### Error Types (LlmError)

| Variant                | Error Code          | Retryable         |
| ---------------------- | ------------------- | ----------------- |
| `ClientInit`           | `llm_init_failed`   | No                |
| `Network`              | `llm_network_error` | Yes (0ms + 500ms) |
| `ApiError { 429 }`     | `llm_rate_limited`  | Yes (1000ms)      |
| `ApiError { 401/403 }` | `llm_auth_failed`   | No                |
| `ApiError { 5xx }`     | `llm_api_error`     | Yes (0ms + 500ms) |
| `ParseError`           | `llm_parse_error`   | No                |

### Known Limitation: extensions.rs

`execute_single_model_post_process` in `extensions.rs` has its **own HTTP implementation** that does NOT call `core.rs` functions. This means:

- `LlmError` types are not used in multi-model execution
- `execute_llm_request_with_retry` does not apply to multi-model candidates
- Unifying this is tracked as future work

## Critical Runtime Rules

- **NEVER use `tokio::spawn` from non-async contexts** (e.g., shortcut handler threads). Use `tauri::async_runtime::spawn` instead.
- **NEVER use `tauri::async_runtime::block_on()` from coordinator thread** — causes deadlocks.
- `TranscriptionCoordinator` runs on a dedicated `std::thread`, not a tokio task.
- Shortcut register/unregister must be synchronous — never wrap in async spawn + block_on.

<!-- rtk-instructions v2 -->

# RTK (Rust Token Killer) - Token-Optimized Commands

## Golden Rule

**Always prefix commands with `rtk`**. If RTK has a dedicated filter, it uses it. If not, it passes through unchanged. This means RTK is always safe to use.

**Important**: Even in command chains with `&&`, use `rtk`:

```bash
# ❌ Wrong
git add . && git commit -m "msg" && git push

# ✅ Correct
rtk git add . && rtk git commit -m "msg" && rtk git push
```

## RTK Commands by Workflow

### Build & Compile (80-90% savings)

```bash
rtk cargo build         # Cargo build output
rtk cargo check         # Cargo check output
rtk cargo clippy        # Clippy warnings grouped by file (80%)
rtk tsc                 # TypeScript errors grouped by file/code (83%)
rtk lint                # ESLint/Biome violations grouped (84%)
rtk prettier --check    # Files needing format only (70%)
rtk next build          # Next.js build with route metrics (87%)
```

### Test (90-99% savings)

```bash
rtk cargo test          # Cargo test failures only (90%)
rtk vitest run          # Vitest failures only (99.5%)
rtk playwright test     # Playwright failures only (94%)
rtk test <cmd>          # Generic test wrapper - failures only
```

### Git (59-80% savings)

```bash
rtk git status          # Compact status
rtk git log             # Compact log (works with all git flags)
rtk git diff            # Compact diff (80%)
rtk git show            # Compact show (80%)
rtk git add             # Ultra-compact confirmations (59%)
rtk git commit          # Ultra-compact confirmations (59%)
rtk git push            # Ultra-compact confirmations
rtk git pull            # Ultra-compact confirmations
rtk git branch          # Compact branch list
rtk git fetch           # Compact fetch
rtk git stash           # Compact stash
rtk git worktree        # Compact worktree
```

Note: Git passthrough works for ALL subcommands, even those not explicitly listed.

### GitHub (26-87% savings)

```bash
rtk gh pr view <num>    # Compact PR view (87%)
rtk gh pr checks        # Compact PR checks (79%)
rtk gh run list         # Compact workflow runs (82%)
rtk gh issue list       # Compact issue list (80%)
rtk gh api              # Compact API responses (26%)
```

### JavaScript/TypeScript Tooling (70-90% savings)

```bash
rtk pnpm list           # Compact dependency tree (70%)
rtk pnpm outdated       # Compact outdated packages (80%)
rtk pnpm install        # Compact install output (90%)
rtk npm run <script>    # Compact npm script output
rtk npx <cmd>           # Compact npx command output
rtk prisma              # Prisma without ASCII art (88%)
```

### Files & Search (60-75% savings)

```bash
rtk ls <path>           # Tree format, compact (65%)
rtk read <file>         # Code reading with filtering (60%)
rtk grep <pattern>      # Search grouped by file (75%)
rtk find <pattern>      # Find grouped by directory (70%)
```

### Analysis & Debug (70-90% savings)

```bash
rtk err <cmd>           # Filter errors only from any command
rtk log <file>          # Deduplicated logs with counts
rtk json <file>         # JSON structure without values
rtk deps                # Dependency overview
rtk env                 # Environment variables compact
rtk summary <cmd>       # Smart summary of command output
rtk diff                # Ultra-compact diffs
```

### Infrastructure (85% savings)

```bash
rtk docker ps           # Compact container list
rtk docker images       # Compact image list
rtk docker logs <c>     # Deduplicated logs
rtk kubectl get         # Compact resource list
rtk kubectl logs        # Deduplicated pod logs
```

### Network (65-70% savings)

```bash
rtk curl <url>          # Compact HTTP responses (70%)
rtk wget <url>          # Compact download output (65%)
```

### Meta Commands

```bash
rtk gain                # View token savings statistics
rtk gain --history      # View command history with savings
rtk discover            # Analyze Claude Code sessions for missed RTK usage
rtk proxy <cmd>         # Run command without filtering (for debugging)
rtk init                # Add RTK instructions to CLAUDE.md
rtk init --global       # Add RTK to ~/.claude/CLAUDE.md
```

## Token Savings Overview

| Category         | Commands                       | Typical Savings |
| ---------------- | ------------------------------ | --------------- |
| Tests            | vitest, playwright, cargo test | 90-99%          |
| Build            | next, tsc, lint, prettier      | 70-87%          |
| Git              | status, log, diff, add, commit | 59-80%          |
| GitHub           | gh pr, gh run, gh issue        | 26-87%          |
| Package Managers | pnpm, npm, npx                 | 70-90%          |
| Files            | ls, read, grep, find           | 60-75%          |
| Infrastructure   | docker, kubectl                | 85%             |
| Network          | curl, wget                     | 65-70%          |

Overall average: **60-90% token reduction** on common development operations.

<!-- /rtk-instructions -->
