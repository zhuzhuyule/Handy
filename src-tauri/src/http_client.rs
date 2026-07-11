use log::debug;
use once_cell::sync::Lazy;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

// Process-wide pool of long-lived HTTP clients, keyed by proxy URL.
//
// Why a pool of clients (instead of building one per request):
//   1. cookie_store is enabled — some providers (notably Gitee AI behind 百度云 WAF)
//      use a Set-Cookie (e.g. `BEC=...`) to mark a session as trusted. The very
//      first POST without that cookie gets 405 from the WAF in <1ms. The client
//      must outlive a single request for the Set-Cookie to take effect on
//      subsequent calls.
//   2. Avoids paying the TLS root-store load + connection pool setup cost on
//      every LLM call.
//
// Only `proxy_url` is part of the key because:
//   - Authorization / custom headers must be applied per-request via the
//     `RequestBuilder` (`.bearer_auth`, `.headers`). They MUST NOT be baked
//     into `default_headers`, since clients are shared across api_keys / models.
//   - Timeouts are applied per-request via `RequestBuilder::timeout`.
static ASYNC_POOL: Lazy<Mutex<HashMap<Option<String>, reqwest::Client>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

static BLOCKING_POOL: Lazy<Mutex<HashMap<Option<String>, reqwest::blocking::Client>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn default_minimal_headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    h
}

/// Get (or lazily create) the process-wide async reqwest client for the given proxy.
///
/// The returned client:
///   - has `cookie_store(true)` so providers behind WAFs can issue/refresh
///     trust cookies (required for Gitee AI etc.).
///   - has no Authorization header — callers must use `.bearer_auth(...)` per-request.
///   - has no per-call timeout baked in — callers should add `.timeout(...)`
///     on the `RequestBuilder` for each request.
///
/// Safe to call from any thread; clients are cheap to clone (internal Arc).
pub fn get_shared_client(proxy_url: Option<&str>) -> Result<reqwest::Client, String> {
    let key = proxy_url.map(|s| s.to_string());
    {
        let pool = ASYNC_POOL.lock().expect("ASYNC_POOL poisoned");
        if let Some(c) = pool.get(&key) {
            return Ok(c.clone());
        }
    }

    let mut builder = reqwest::Client::builder()
        .default_headers(default_minimal_headers())
        .cookie_store(true)
        .pool_max_idle_per_host(8);

    if let Some(url) = proxy_url {
        if !url.is_empty() {
            debug!("[HttpClient] Using proxy: {}", url);
            let proxy = reqwest::Proxy::all(url)
                .map_err(|e| format!("Invalid proxy URL '{}': {}", url, e))?;
            builder = builder.proxy(proxy);
        }
    }

    let client = builder
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let mut pool = ASYNC_POOL.lock().expect("ASYNC_POOL poisoned");
    // Re-check in case of race (cheap clone is fine if we lose).
    if let Some(existing) = pool.get(&key) {
        return Ok(existing.clone());
    }
    pool.insert(key, client.clone());
    Ok(client)
}

/// One-off async client with user-supplied timeout + default headers.
///
/// Use this for non-LLM HTTP calls that have specific timeout / header needs
/// and don't benefit from the shared cookie jar (avatar downloads, model
/// downloads, proxy probes, etc.). Each call builds a fresh client.
///
/// LLM call paths must NOT use this — use `get_shared_client` + per-request
/// configuration instead.
pub fn build_http_client(
    proxy_url: Option<&str>,
    timeout: Duration,
    default_headers: HeaderMap,
) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .default_headers(default_headers)
        .cookie_store(true)
        .timeout(timeout);

    if let Some(url) = proxy_url {
        if !url.is_empty() {
            debug!("[HttpClient] Using proxy: {}", url);
            let proxy = reqwest::Proxy::all(url)
                .map_err(|e| format!("Invalid proxy URL '{}': {}", url, e))?;
            builder = builder.proxy(proxy);
        }
    }

    builder
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))
}

/// One-off blocking client. Used by online ASR (multipart audio upload runs
/// on its own non-tokio thread).
pub fn build_blocking_http_client(
    proxy_url: Option<&str>,
    timeout: Duration,
) -> Result<reqwest::blocking::Client, String> {
    build_blocking_http_client_with_policy(proxy_url, true, timeout)
}

/// Blocking client with explicit control over system-proxy inheritance.
///
/// reqwest reads `HTTP(S)_PROXY` / `ALL_PROXY` from the environment by default.
/// When `use_system_proxy` is false and no explicit `proxy_url` is given, this
/// calls `.no_proxy()` so the request connects directly — letting a provider
/// with `use_proxy=false` (e.g. a domestic ASR endpoint) bypass a system proxy
/// that the user only set to reach overseas providers. Without this, both the
/// domestic and overseas providers share the one proxy path and fail together.
pub fn build_blocking_http_client_with_policy(
    proxy_url: Option<&str>,
    use_system_proxy: bool,
    timeout: Duration,
) -> Result<reqwest::blocking::Client, String> {
    let mut builder = reqwest::blocking::Client::builder()
        .cookie_store(true)
        .timeout(timeout);

    match proxy_url.filter(|u| !u.is_empty()) {
        Some(url) => {
            debug!("[HttpClient] Using blocking proxy: {}", url);
            let proxy = reqwest::Proxy::all(url)
                .map_err(|e| format!("Invalid proxy URL '{}': {}", url, e))?;
            builder = builder.proxy(proxy);
        }
        None if !use_system_proxy => {
            debug!("[HttpClient] Blocking client: direct connect (no_proxy)");
            builder = builder.no_proxy();
        }
        None => {}
    }

    builder
        .build()
        .map_err(|e| format!("Failed to build blocking HTTP client: {}", e))
}

/// Same as `get_shared_client` but returns a `reqwest::blocking::Client`.
/// Used by online ASR-style cases that want pooling on the blocking side too.
#[allow(dead_code)]
pub fn get_shared_blocking_client(
    proxy_url: Option<&str>,
    timeout: Duration,
) -> Result<reqwest::blocking::Client, String> {
    let key = proxy_url.map(|s| s.to_string());
    {
        let pool = BLOCKING_POOL.lock().expect("BLOCKING_POOL poisoned");
        if let Some(c) = pool.get(&key) {
            return Ok(c.clone());
        }
    }

    let mut builder = reqwest::blocking::Client::builder()
        .cookie_store(true)
        .pool_max_idle_per_host(4)
        .timeout(timeout);

    if let Some(url) = proxy_url {
        if !url.is_empty() {
            debug!("[HttpClient] Using blocking proxy: {}", url);
            let proxy = reqwest::Proxy::all(url)
                .map_err(|e| format!("Invalid proxy URL '{}': {}", url, e))?;
            builder = builder.proxy(proxy);
        }
    }

    let client = builder
        .build()
        .map_err(|e| format!("Failed to build blocking HTTP client: {}", e))?;

    let mut pool = BLOCKING_POOL.lock().expect("BLOCKING_POOL poisoned");
    if let Some(existing) = pool.get(&key) {
        return Ok(existing.clone());
    }
    pool.insert(key, client.clone());
    Ok(client)
}
