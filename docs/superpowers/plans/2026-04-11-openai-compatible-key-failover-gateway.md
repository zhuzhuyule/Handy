# OpenAI-Compatible Key Failover Gateway Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify all provider-backed model calls behind a single execution layer that performs per-provider key rotation, request-scoped failover, cooldown-aware fallback, and provider-level exhaustion reporting, then reuse that layer for future OpenAI-compatible northbound APIs.

**Architecture:** Keep `cached_models` as the source of truth for model identity, with northbound `model` values mapped to `cached_model.id`. Upgrade `KeySelector` into a request-aware KeyPool that can return the next healthy key, or a single earliest-expiring cooldown key as a fallback when all keys are cooling down. Add a dedicated execution gateway that owns retry/failover behavior and migrate all direct `first_key()` reads to that gateway before exposing `/v1/models` and request handlers.

**Tech Stack:** Rust (Tauri 2.x, reqwest, serde, log), existing `SecretKeyRing`/`KeySelector`, existing post-process and ASR actions, future OpenAI-compatible command surface

---

## Task 1: Upgrade `KeySelector` Into a Request-Aware KeyPool

**Files:**

- Modify: `src-tauri/src/key_selector.rs`
- Test: `src-tauri/src/key_selector.rs`

- [ ] **Step 1: Add the failing tests for request-scoped selection rules**

Append a test module to `src-tauri/src/key_selector.rs` that covers:

```rust
#[cfg(test)]
mod tests {
    use super::KeySelector;
    use crate::settings::KeyEntry;

    fn key(value: &str) -> KeyEntry {
        KeyEntry {
            key: value.to_string(),
            enabled: true,
            label: None,
        }
    }

    #[test]
    fn rotates_across_healthy_keys_without_reusing_attempted_indices() {
        let selector = KeySelector::new();
        let keys = vec![key("k1"), key("k2"), key("k3")];

        let first = selector
            .acquire_next_key("openai", &keys, &[])
            .expect("first key");
        let second = selector
            .acquire_next_key("openai", &keys, &[first.key_index])
            .expect("second key");

        assert_ne!(first.key_index, second.key_index);
    }

    #[test]
    fn falls_back_to_earliest_expiring_key_when_all_keys_are_cooling_down() {
        let selector = KeySelector::new();
        let keys = vec![key("k1"), key("k2")];

        selector.mark_error("openai", 0, 401);
        selector.mark_error("openai", 1, 429);

        let acquired = selector
            .acquire_next_key("openai", &keys, &[])
            .expect("cooldown fallback key");

        assert!(acquired.from_cooldown_fallback);
        assert_eq!(acquired.key_index, 1);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail against the current API**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml key_selector
```

Expected: FAIL because `acquire_next_key()` and the returned key metadata do not exist yet.

- [ ] **Step 3: Replace `next_key()` with a richer acquire contract**

Refactor `src-tauri/src/key_selector.rs` to introduce request-aware return metadata and selection behavior:

```rust
pub struct AcquiredKey<'a> {
    pub key_index: usize,
    pub api_key: &'a str,
    pub from_cooldown_fallback: bool,
}

impl KeySelector {
    pub fn acquire_next_key<'a>(
        &self,
        provider_id: &str,
        keys: &'a [KeyEntry],
        attempted_indices: &[usize],
    ) -> Option<AcquiredKey<'a>> {
        // 1. Filter enabled + non-empty keys
        // 2. Exclude attempted_indices
        // 3. Prefer healthy keys in round-robin order
        // 4. If no healthy keys remain, return the earliest-expiring cooldown key once
    }
}
```

Keep `mark_error()` and `reset()`, but extend the provider runtime state so it can track:

```rust
struct ProviderState {
    index: usize,
    cooldowns: Vec<Option<Instant>>,
    consecutive_failures: Vec<u32>,
    last_error_code: Vec<Option<u16>>,
    last_used_at: Vec<Option<Instant>>,
    last_success_at: Vec<Option<Instant>>,
}
```

- [ ] **Step 4: Add success reporting so callers can clear failure state**

Add:

```rust
pub fn report_success(&self, provider_id: &str, key_index: usize) {
    let mut state = self.state.lock().unwrap();
    if let Some(provider_state) = state.get_mut(provider_id) {
        if key_index < provider_state.consecutive_failures.len() {
            provider_state.consecutive_failures[key_index] = 0;
            provider_state.last_error_code[key_index] = None;
            provider_state.last_success_at[key_index] = Some(Instant::now());
            provider_state.cooldowns[key_index] = None;
        }
    }
}
```

Also update `mark_error()` to resize all runtime vectors and record `last_used_at`.

- [ ] **Step 5: Run the tests to verify the new selection rules**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml key_selector
```

Expected: PASS for the new round-robin and cooldown-fallback tests.

- [ ] **Step 6: Commit the KeyPool refactor**

```bash
git add src-tauri/src/key_selector.rs
git commit -m "Refine key selector into request-aware key pool"
```

## Task 2: Add a Unified Execution Gateway for Provider Calls

**Files:**

- Create: `src-tauri/src/provider_gateway.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `src-tauri/src/provider_gateway.rs`

- [ ] **Step 1: Add failing tests for fatal-vs-retryable behavior**

Create `src-tauri/src/provider_gateway.rs` with a test-first skeleton:

```rust
#[cfg(test)]
mod tests {
    use super::{AttemptError, AttemptResult, ExecutionOutcome, execute_with_failover};

    #[tokio::test]
    async fn retries_on_retryable_errors_and_returns_success() {
        // attempt 1 -> retryable 429
        // attempt 2 -> success
    }

    #[tokio::test]
    async fn stops_immediately_on_fatal_400() {
        // attempt 1 -> fatal bad request
    }
}
```

The tests should assert:

- retryable errors consume another key attempt
- fatal errors stop without trying another key
- request exhaustion returns an explicit exhausted outcome instead of a generic string

- [ ] **Step 2: Run the tests to verify the gateway is not implemented yet**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml provider_gateway
```

Expected: FAIL because the module, result types, and execution function are still incomplete.

- [ ] **Step 3: Implement the gateway types and retry loop**

Define the new module contents:

```rust
use tauri::{AppHandle, Manager};

pub enum AttemptError {
    Retryable { status: Option<u16>, detail: String },
    Fatal { status: Option<u16>, detail: String },
}

pub type AttemptResult<T> = Result<T, AttemptError>;

pub struct ExecutionPlan {
    pub provider_id: String,
    pub cached_model_id: String,
    pub remote_model_id: String,
    pub max_attempts: usize,
}

pub enum ExecutionOutcome<T> {
    Success(T),
    Exhausted { provider_id: String, attempts: usize, last_error: String },
    Fatal { provider_id: String, detail: String },
}
```

Then implement:

```rust
pub async fn execute_with_failover<T, F, Fut>(
    app: &AppHandle,
    settings: &crate::settings::AppSettings,
    plan: &ExecutionPlan,
    mut attempt: F,
) -> ExecutionOutcome<T>
where
    F: FnMut(usize, String) -> Fut,
    Fut: std::future::Future<Output = AttemptResult<T>>,
{
    // read keys from settings.post_process_api_keys
    // call KeySelector::acquire_next_key()
    // retry up to plan.max_attempts
    // call report_success()/mark_error()
    // collapse final state into Success / Exhausted / Fatal
}
```

- [ ] **Step 4: Register the module in the crate root**

Update `src-tauri/src/lib.rs` near the existing module declarations:

```rust
mod provider_gateway;
```

Do not add a Tauri command yet; this module is an internal execution primitive.

- [ ] **Step 5: Run the tests for the gateway**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml provider_gateway
```

Expected: PASS for the retryable and fatal-path tests.

- [ ] **Step 6: Commit the execution gateway**

```bash
git add src-tauri/src/provider_gateway.rs src-tauri/src/lib.rs
git commit -m "Add unified provider execution gateway"
```

## Task 3: Migrate Core LLM Execution Paths to the Gateway

**Files:**

- Modify: `src-tauri/src/actions/post_process/core.rs`
- Modify: `src-tauri/src/actions/post_process/extensions.rs`
- Test: `src-tauri/src/actions/post_process/core.rs`

- [ ] **Step 1: Add a failing unit test for retry classification**

Extend the existing `mod tests` in `src-tauri/src/actions/post_process/core.rs` with classification coverage:

```rust
#[test]
fn classify_http_status_for_failover() {
    assert!(matches!(classify_status_code(429), AttemptError::Retryable { .. }));
    assert!(matches!(classify_status_code(503), AttemptError::Retryable { .. }));
    assert!(matches!(classify_status_code(400), AttemptError::Fatal { .. }));
}
```

- [ ] **Step 2: Run the focused tests to verify the helper is missing**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml classify_http_status_for_failover
```

Expected: FAIL because the helper does not exist.

- [ ] **Step 3: Refactor `core.rs` to classify errors and delegate retries**

In `src-tauri/src/actions/post_process/core.rs`:

- keep request-body construction local
- move key acquisition and retry orchestration into `provider_gateway::execute_with_failover`
- convert HTTP and transport failures into `AttemptError`

The integration shape should look like:

```rust
let plan = crate::provider_gateway::ExecutionPlan {
    provider_id: provider.id.clone(),
    cached_model_id: cached_model_id.unwrap_or_default().to_string(),
    remote_model_id: model.to_string(),
    max_attempts: 3,
};

match crate::provider_gateway::execute_with_failover(app_handle, settings, &plan, |_, api_key| async move {
    // perform one HTTP attempt with api_key
}).await {
    ExecutionOutcome::Success(resp) => Ok(resp),
    ExecutionOutcome::Exhausted { last_error, .. } => Err(LlmError::ApiError {
        provider: provider.id.clone(),
        model: model.to_string(),
        status: 503,
        body: last_error,
    }),
    ExecutionOutcome::Fatal { detail, .. } => Err(LlmError::ClientInit {
        provider: provider.id.clone(),
        model: model.to_string(),
        detail,
    }),
}
```

- [ ] **Step 4: Apply the same retry integration to multi-model execution**

Refactor `src-tauri/src/actions/post_process/extensions.rs` so each multi-model participant uses the same `ExecutionPlan` + `execute_with_failover` flow instead of directly calling `next_key()`.

The per-item closure should still build its own body and headers, but key selection and failover must be owned by the gateway.

- [ ] **Step 5: Run the focused Rust tests for post-process core**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml actions::post_process::core
```

Expected: PASS, including the new failover classification test.

- [ ] **Step 6: Commit the core migration**

```bash
git add src-tauri/src/actions/post_process/core.rs src-tauri/src/actions/post_process/extensions.rs
git commit -m "Route core LLM execution through provider gateway"
```

## Task 4: Remove Remaining `first_key()` Reads From Routing, ASR, and Hotword Paths

**Files:**

- Modify: `src-tauri/src/actions/post_process/routing.rs`
- Modify: `src-tauri/src/actions/transcribe.rs`
- Modify: `src-tauri/src/managers/hotword.rs`
- Test: `src-tauri/src/managers/hotword.rs`

- [ ] **Step 1: Add a regression test around strict model binding**

Extend the existing `mod tests` in `src-tauri/src/managers/hotword.rs` with a small helper-level test that ensures model/provider resolution keeps the original `cached_model.id` semantics:

```rust
#[test]
fn cached_model_binding_remains_provider_stable() {
    // build a tiny AppSettings fixture with one cached_model
    // verify helper resolution returns that provider/model pair
}
```

- [ ] **Step 2: Run the hotword tests to verify the helper is still missing**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml managers::hotword
```

Expected: FAIL because the shared resolution helper is not in place yet.

- [ ] **Step 3: Replace direct key lookup in routing**

Update `src-tauri/src/actions/post_process/routing.rs` to stop returning raw `api_key` strings from `resolve_intent_routing_model()`.

Change the shape from:

```rust
Option<(&PostProcessProvider, String, String)>
```

To:

```rust
Option<(&PostProcessProvider, String, String)>
```

Where the third `String` becomes the resolved `cached_model_id`, not the actual API key. `perform_skill_routing()` should then call the provider gateway to execute the request for that exact cached model instead of receiving a pre-fetched key.

- [ ] **Step 4: Replace direct key lookup in online ASR and hotword**

In `src-tauri/src/actions/transcribe.rs` and `src-tauri/src/managers/hotword.rs`:

- remove all `post_process_api_keys.first_key(...)` calls
- build `ExecutionPlan` from the resolved `cached_model`
- pass a single-attempt ASR/hotword closure into `execute_with_failover`

The call pattern should look like:

```rust
let plan = ExecutionPlan {
    provider_id: cached.provider_id.clone(),
    cached_model_id: cached.id.clone(),
    remote_model_id: cached.model_id.clone(),
    max_attempts: 3,
};
```

- [ ] **Step 5: Run the migrated tests and one broader backend test pass**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml managers::hotword
cargo test --manifest-path src-tauri/Cargo.toml actions::post_process::routing
```

Expected: PASS, with no remaining compile-time uses of `first_key()` in migrated files.

- [ ] **Step 6: Commit the call-site cleanup**

```bash
git add src-tauri/src/actions/post_process/routing.rs src-tauri/src/actions/transcribe.rs src-tauri/src/managers/hotword.rs
git commit -m "Remove direct provider key reads from runtime paths"
```

## Task 5: Add Provider Exhaustion Reporting and OpenAI-Compatible Model Listing

**Files:**

- Create: `src-tauri/src/commands/openai_gateway.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `src-tauri/src/commands/openai_gateway.rs`

- [ ] **Step 1: Add failing tests for `/v1/models` serialization**

Create `src-tauri/src/commands/openai_gateway.rs` with a test-first model-list helper:

```rust
#[cfg(test)]
mod tests {
    use super::build_openai_models_response;

    #[test]
    fn uses_cached_model_id_as_public_model_id() {
        // fixture AppSettings with one cached_model
        // assert response.data[0].id == cached_model.id
    }
}
```

- [ ] **Step 2: Run the focused tests to verify the command module does not exist yet**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml openai_gateway
```

Expected: FAIL because the module and helper are not implemented yet.

- [ ] **Step 3: Implement model-list serialization and exhaustion event logging**

In `src-tauri/src/commands/openai_gateway.rs`, add:

```rust
#[derive(serde::Serialize)]
pub struct OpenAiModelItem {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub owned_by: String,
}

#[derive(serde::Serialize)]
pub struct OpenAiModelsResponse {
    pub object: &'static str,
    pub data: Vec<OpenAiModelItem>,
}

pub fn build_openai_models_response(settings: &crate::settings::AppSettings) -> OpenAiModelsResponse {
    // map cached_models -> OpenAiModelItem using cached_model.id as public id
}

#[tauri::command]
pub fn get_openai_models(app: tauri::AppHandle) -> OpenAiModelsResponse {
    let settings = crate::settings::get_settings(&app);
    build_openai_models_response(&settings)
}
```

At the same time, add one shared helper in `provider_gateway.rs` or this command module to log/report:

```rust
pub fn report_provider_exhausted(provider_id: &str, attempts: usize, detail: &str) {
    log::error!(
        "[ProviderGateway] provider exhausted: provider={} attempts={} detail={}",
        provider_id,
        attempts,
        detail
    );
}
```

Use this helper when `ExecutionOutcome::Exhausted` is returned.

- [ ] **Step 4: Register the command and module**

Update `src-tauri/src/commands/mod.rs`:

```rust
pub mod openai_gateway;
```

Update the `generate_handler![]` list in `src-tauri/src/lib.rs` to include:

```rust
commands::openai_gateway::get_openai_models,
```

- [ ] **Step 5: Run the gateway tests and a full backend compile check**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml openai_gateway
cargo test --manifest-path src-tauri/Cargo.toml --no-run
```

Expected: PASS, with the command registered and the backend compiling cleanly.

- [ ] **Step 6: Commit the northbound skeleton**

```bash
git add src-tauri/src/commands/openai_gateway.rs src-tauri/src/commands/mod.rs src-tauri/src/lib.rs src-tauri/src/provider_gateway.rs
git commit -m "Add OpenAI-compatible model listing and exhaustion reporting"
```

## Task 6: Verify End-to-End Behavior and Document Remaining Follow-Ups

**Files:**

- Modify: `docs/superpowers/specs/2026-04-11-openai-compatible-key-failover-gateway-design.md`
- Modify: `docs/superpowers/plans/2026-04-11-openai-compatible-key-failover-gateway.md`

- [ ] **Step 1: Run the backend test suite for the touched areas**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml key_selector
cargo test --manifest-path src-tauri/Cargo.toml provider_gateway
cargo test --manifest-path src-tauri/Cargo.toml actions::post_process::core
cargo test --manifest-path src-tauri/Cargo.toml managers::hotword
cargo test --manifest-path src-tauri/Cargo.toml --no-run
```

Expected: PASS for all focused tests and successful compile for the backend crate.

- [ ] **Step 2: Smoke-check that no migrated runtime path still reads `first_key()`**

Run:

```bash
rg -n "first_key\\(" src-tauri/src/actions src-tauri/src/managers src-tauri/src/commands
```

Expected: no matches in migrated runtime paths; any remaining matches must be in untouched legacy or test-only code and should be documented explicitly.

- [ ] **Step 3: Update the design doc with implementation deviations**

Append a filled-in row to:

`docs/superpowers/specs/2026-04-11-openai-compatible-key-failover-gateway-design.md`

Example format:

```markdown
| Provider-level DA incident | Logged provider exhaustion only | User-facing DA hook was not part of this implementation scope |
```

- [ ] **Step 4: Record any intentionally deferred northbound API scope**

If `/v1/chat/completions` and `/v1/audio/transcriptions` remain unimplemented in this cycle, add a short note to this plan stating the exact shipped scope:

```markdown
This implementation shipped the shared execution substrate and `/v1/models` model-list exposure only.
`/v1/chat/completions` and `/v1/audio/transcriptions` were intentionally excluded from this cycle.
```

- [ ] **Step 5: Commit the verification and documentation updates**

```bash
git add docs/superpowers/specs/2026-04-11-openai-compatible-key-failover-gateway-design.md docs/superpowers/plans/2026-04-11-openai-compatible-key-failover-gateway.md
git commit -m "Document key failover gateway verification results"
```
