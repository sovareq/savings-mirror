//! Billing-mode detection for savings-mirror.
//!
//! Distinguishes Anthropic pay-per-token API usage from flat-rate subscription
//! plans (Pro/Max/Team/Enterprise) so the dashboard can render the right
//! metric: USD-savings for API users, 5h/7d utilization headroom for
//! subscription users.
//!
//! See `docs/intelligence/2026-05-28-billing-mode-research.md` for the
//! authoritative findings backing the env-var precedence and the
//! `/api/oauth/usage` contract.

#![allow(dead_code)]

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Which billing mode the dashboard should render for the current host.
///
/// `Auto` is only ever returned from `detect_mode` when the operator has
/// explicitly forced auto-detection via the env var; the actual classification
/// then proceeds through the rest of the precedence chain. In practice the
/// public API of `detect_mode` resolves to `Api` or `Subscription`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BillingMode {
    /// Pay-per-token Anthropic API. USD savings are meaningful.
    Api,
    /// Flat-rate subscription (Pro/Max/Team/Enterprise). Show utilization.
    Subscription,
    /// Explicit "let the tool decide" — placeholder; never the final answer.
    Auto,
}

impl BillingMode {
    /// Lowercase string form used in JSON payloads and the env-var contract.
    pub fn as_str(&self) -> &'static str {
        match self {
            BillingMode::Api => "api",
            BillingMode::Subscription => "subscription",
            BillingMode::Auto => "auto",
        }
    }
}

/// Single rolling-window utilization datapoint as reported by Anthropic.
///
/// `utilization` is a 0-100 percentage. `resets_at` is an ISO-8601 UTC
/// timestamp marking when the window rolls over.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageWindow {
    pub utilization: f64,
    pub resets_at: Option<String>,
}

/// Full subscription-quota snapshot from `/api/oauth/usage`.
///
/// Anthropic reports two windows: a 5-hour rolling cap and a 7-day rolling
/// cap. Both surface as percentages so the dashboard does not need to know
/// per-tier numeric caps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OauthUsage {
    pub five_hour: UsageWindow,
    pub seven_day: UsageWindow,
}

/// Combined billing state used by the `/api/billing` endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct BillingState {
    pub mode: BillingMode,
    pub usage: Option<OauthUsage>,
    pub detected_via: String,
}

/// Resolve the active billing mode using full precedence.
///
/// Order:
///   0. UI-override file (`~/.config/savings-mirror/billing-mode-override`):
///      written by the dashboard's clickable mode pill. Wins over everything
///      else so an operator can pin the mode without restarting.
///   1. `SAVINGS_MIRROR_BILLING_MODE` ∈ {api, subscription, auto}.
///      `api`/`subscription` win; `auto` falls through to the rest.
///   2. `ANTHROPIC_API_KEY` set → Api.
///   3. `ANTHROPIC_AUTH_TOKEN` set → Api (other env API auth path).
///   4. `CLAUDE_CODE_OAUTH_TOKEN` set → Subscription.
///   5. `read_oauth_token()` returns Some → Subscription.
///   6. Fallback → Api (preserves existing dashboard semantics).
pub fn detect_mode() -> BillingMode {
    detect_mode_with(
        &read_override,
        &|k| std::env::var(k).ok(),
        &read_oauth_token,
    )
}

/// Returns a human-readable label for the detection step that produced
/// `detect_mode`'s result.
///
/// Intended for the `detected_via` field of the `/api/billing` response so
/// operators can debug "why does the dashboard think I'm on a subscription?"
pub fn detection_source() -> &'static str {
    detection_source_with(
        &read_override,
        &|k| std::env::var(k).ok(),
        &read_oauth_token,
    )
}

/// On-disk override file path. The dashboard's clickable mode pill writes
/// here via `POST /api/billing/override`. Empty / missing file = no override
/// active.
pub fn override_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join(".config/savings-mirror/billing-mode-override")
}

/// Read the UI-set override, if any. Recognises `api`, `subscription`,
/// `auto`. Anything else (including empty file, parse error, missing file)
/// returns None so detection falls through to env/oauth.
pub fn read_override() -> Option<BillingMode> {
    read_override_at(&override_path())
}

/// Testable sibling of `read_override`: reads the override file at an
/// arbitrary path so tests can use `TempDir` without env mutation.
pub fn read_override_at(path: &std::path::Path) -> Option<BillingMode> {
    let raw = std::fs::read_to_string(path).ok()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "api" => Some(BillingMode::Api),
        "subscription" => Some(BillingMode::Subscription),
        // "auto" or anything else clears the override (None = no override).
        _ => None,
    }
}

/// Persist the UI-selected override. `None` clears the override (deletes
/// the file). Returns an error if the directory could not be created or the
/// write failed; callers in handler code should surface the error in the
/// JSON response.
pub fn write_override(mode: Option<BillingMode>) -> Result<()> {
    write_override_at(&override_path(), mode)
}

/// Testable sibling of `write_override`.
pub fn write_override_at(path: &std::path::Path, mode: Option<BillingMode>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating override dir {parent:?}"))?;
    }
    match mode {
        Some(m) => std::fs::write(path, m.as_str())
            .with_context(|| format!("writing override file {path:?}")),
        None => {
            // None == clear. Missing file is fine.
            match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e).with_context(|| format!("removing override file {path:?}")),
            }
        }
    }
}

/// Pure-function variant of `detect_mode` for testability.
///
/// `env` reads env vars; `oauth` returns the OAuth token if available.
/// Both are injected so unit tests don't have to mutate process state.
fn detect_mode_with(
    override_fn: &dyn Fn() -> Option<BillingMode>,
    env: &dyn Fn(&str) -> Option<String>,
    oauth: &dyn Fn() -> Option<String>,
) -> BillingMode {
    if let Some(m) = override_fn() {
        return m;
    }
    let non_empty = |k: &str| env(k).filter(|s| !s.trim().is_empty());

    if let Some(forced) = non_empty("SAVINGS_MIRROR_BILLING_MODE") {
        match forced.to_ascii_lowercase().as_str() {
            "api" => return BillingMode::Api,
            "subscription" => return BillingMode::Subscription,
            "auto" => {} // explicit auto: fall through to detection chain
            other => {
                eprintln!(
                    "savings-mirror: unknown SAVINGS_MIRROR_BILLING_MODE value `{other}` (expected api|subscription|auto); falling through to auto-detect"
                );
            }
        }
    }
    if non_empty("ANTHROPIC_API_KEY").is_some() {
        return BillingMode::Api;
    }
    if non_empty("ANTHROPIC_AUTH_TOKEN").is_some() {
        return BillingMode::Api;
    }
    if non_empty("CLAUDE_CODE_OAUTH_TOKEN").is_some() {
        return BillingMode::Subscription;
    }
    if oauth().is_some() {
        return BillingMode::Subscription;
    }
    BillingMode::Api
}

/// Pure-function variant of `detection_source` for testability.
fn detection_source_with(
    override_fn: &dyn Fn() -> Option<BillingMode>,
    env: &dyn Fn(&str) -> Option<String>,
    oauth: &dyn Fn() -> Option<String>,
) -> &'static str {
    if override_fn().is_some() {
        return "ui-override";
    }
    let non_empty = |k: &str| env(k).filter(|s| !s.trim().is_empty());

    if let Some(forced) = non_empty("SAVINGS_MIRROR_BILLING_MODE") {
        match forced.to_ascii_lowercase().as_str() {
            "api" | "subscription" => return "env-forced",
            "auto" => {}
            _ => {} // already warned in detect_mode_with
        }
    }
    if non_empty("ANTHROPIC_API_KEY").is_some() {
        return "env-anthropic-api-key";
    }
    if non_empty("ANTHROPIC_AUTH_TOKEN").is_some() {
        return "env-anthropic-auth-token";
    }
    if non_empty("CLAUDE_CODE_OAUTH_TOKEN").is_some() {
        return "env-claude-code-oauth-token";
    }
    if oauth().is_some() {
        return "oauth-token";
    }
    "fallback-api"
}

/// Fetch the current 5h/7d utilization snapshot from Anthropic.
///
/// `token` is a short-lived OAuth access-token (NOT an `sk-ant-…` API key).
/// `base_url` is the protocol+host root; production callers pass
/// `https://api.anthropic.com`, tests pass the mockito server URL.
///
/// 5-second hard timeout. 4xx/5xx surface as `anyhow::Error` so the caller can
/// decide whether to degrade to a stale snapshot.
// TODO: verify against live endpoint once OAuth token available
pub async fn fetch_oauth_usage(token: &str, base_url: &str) -> Result<OauthUsage> {
    let url = format!("{base_url}/api/oauth/usage");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent(concat!("savings-mirror/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("build reqwest client")?;

    let resp = client
        .get(&url)
        .bearer_auth(token)
        // TODO: verify against live endpoint once OAuth token available
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("content-type", "application/json")
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;

    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("oauth-usage endpoint returned HTTP {status}");
    }

    resp.json::<OauthUsage>()
        .await
        .context("decode /api/oauth/usage response")
}

/// macOS Keychain reader. Best-effort, returns None on any failure.
///
/// Shells out to `security find-generic-password -s "Claude Code-credentials"
/// -a "$USER" -w` and parses the resulting JSON for
/// `claudeAiOauth.accessToken`.
pub fn read_oauth_token_macos() -> Option<String> {
    let user = std::env::var("USER").ok()?;
    let output = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-a",
            &user,
            "-w",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    let raw = raw.trim();
    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    parsed
        .get("claudeAiOauth")?
        .get("accessToken")?
        .as_str()
        .map(str::to_string)
}

/// Linux/Windows fallback reader. Reads `~/.claude/.credentials.json` and
/// parses the same `claudeAiOauth.accessToken` field.
pub fn read_oauth_token_unix() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let path = std::path::Path::new(&home).join(".claude/.credentials.json");
    read_oauth_token_unix_at(&path)
}

/// Testable sibling of `read_oauth_token_unix`: parses the OAuth credentials
/// JSON at an arbitrary path. Returns None on missing file, IO error, invalid
/// JSON, or missing `claudeAiOauth.accessToken` nested key.
///
/// The public no-arg `read_oauth_token_unix` delegates here after resolving
/// `~/.claude/.credentials.json` from `$HOME`; tests inject a `TempDir` path
/// directly to avoid touching process env or the real home directory.
pub fn read_oauth_token_unix_at(path: &std::path::Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    parsed
        .get("claudeAiOauth")?
        .get("accessToken")?
        .as_str()
        .map(str::to_string)
}

/// OS-dispatching OAuth-token reader. Returns None on any failure.
#[cfg(target_os = "macos")]
pub fn read_oauth_token() -> Option<String> {
    read_oauth_token_macos()
}

/// OS-dispatching OAuth-token reader. Returns None on any failure.
#[cfg(not(target_os = "macos"))]
pub fn read_oauth_token() -> Option<String> {
    read_oauth_token_unix()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build an env-reader closure from a static map, so tests don't mutate
    /// process-wide state (which would require unsafe in edition 2024 and
    /// race other parallel tests).
    fn env_from(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<&'static str, &'static str> = pairs.iter().copied().collect();
        move |k: &str| map.get(k).map(|s| s.to_string())
    }

    fn no_oauth() -> Option<String> {
        None
    }

    fn yes_oauth() -> Option<String> {
        Some("fake-oauth-token".into())
    }

    /// No UI-override active — used by every legacy test that predates
    /// the override layer.
    fn no_override() -> Option<BillingMode> {
        None
    }

    #[test]
    fn detect_mode_explicit_api() {
        // Explicit `api` wins even when an OAuth-ish env hint is present.
        let env = env_from(&[
            ("SAVINGS_MIRROR_BILLING_MODE", "api"),
            ("ANTHROPIC_AUTH_TOKEN", "stub"),
        ]);
        assert_eq!(
            detect_mode_with(&no_override, &env, &yes_oauth),
            BillingMode::Api
        );
    }

    #[test]
    fn detect_mode_explicit_subscription() {
        // Explicit `subscription` wins even when an API key is set.
        let env = env_from(&[
            ("SAVINGS_MIRROR_BILLING_MODE", "subscription"),
            ("ANTHROPIC_API_KEY", "sk-ant-stub"),
        ]);
        assert_eq!(
            detect_mode_with(&no_override, &env, &no_oauth),
            BillingMode::Subscription
        );
    }

    #[test]
    fn detect_mode_explicit_auto_falls_through() {
        // `auto` → fall through → API key wins → Api.
        let env = env_from(&[
            ("SAVINGS_MIRROR_BILLING_MODE", "auto"),
            ("ANTHROPIC_API_KEY", "sk-ant-stub"),
        ]);
        assert_eq!(
            detect_mode_with(&no_override, &env, &yes_oauth),
            BillingMode::Api
        );
    }

    #[test]
    fn detect_mode_api_key_wins_over_oauth_helper() {
        // API-key precedence comes BEFORE the OAuth helper.
        let env = env_from(&[("ANTHROPIC_API_KEY", "sk-ant-stub")]);
        assert_eq!(
            detect_mode_with(&no_override, &env, &yes_oauth),
            BillingMode::Api
        );
    }

    #[test]
    fn detect_mode_auth_token_wins_over_oauth_helper() {
        // `ANTHROPIC_AUTH_TOKEN` (gateway-style) also classifies as Api.
        let env = env_from(&[("ANTHROPIC_AUTH_TOKEN", "stub")]);
        assert_eq!(
            detect_mode_with(&no_override, &env, &yes_oauth),
            BillingMode::Api
        );
    }

    #[test]
    fn detect_mode_oauth_helper_yields_subscription() {
        // Nothing in env, OAuth reader returns Some → Subscription.
        let env = env_from(&[]);
        assert_eq!(
            detect_mode_with(&no_override, &env, &yes_oauth),
            BillingMode::Subscription
        );
    }

    #[test]
    fn detect_mode_fallback_when_nothing_set() {
        // Nothing anywhere → safe fallback Api.
        let env = env_from(&[]);
        let mode = detect_mode_with(&no_override, &env, &no_oauth);
        assert_eq!(mode, BillingMode::Api);
        assert_ne!(mode, BillingMode::Auto);
    }

    #[test]
    fn detect_mode_override_wins_over_everything() {
        // UI override beats env, beats OAuth, beats fallback.
        let env = env_from(&[
            ("SAVINGS_MIRROR_BILLING_MODE", "subscription"),
            ("ANTHROPIC_API_KEY", "sk-ant-stub"),
        ]);
        let override_api = || Some(BillingMode::Api);
        assert_eq!(
            detect_mode_with(&override_api, &env, &yes_oauth),
            BillingMode::Api
        );
        let override_sub = || Some(BillingMode::Subscription);
        let env_api = env_from(&[("ANTHROPIC_API_KEY", "sk-ant-stub")]);
        assert_eq!(
            detect_mode_with(&override_sub, &env_api, &no_oauth),
            BillingMode::Subscription
        );
    }

    #[test]
    fn detection_source_labels_match_chain() {
        assert_eq!(
            detection_source_with(
                &no_override,
                &env_from(&[("SAVINGS_MIRROR_BILLING_MODE", "api")]),
                &no_oauth
            ),
            "env-forced"
        );
        assert_eq!(
            detection_source_with(
                &no_override,
                &env_from(&[("ANTHROPIC_API_KEY", "x")]),
                &no_oauth
            ),
            "env-anthropic-api-key"
        );
        assert_eq!(
            detection_source_with(
                &no_override,
                &env_from(&[("ANTHROPIC_AUTH_TOKEN", "x")]),
                &no_oauth
            ),
            "env-anthropic-auth-token"
        );
        assert_eq!(
            detection_source_with(&no_override, &env_from(&[]), &yes_oauth),
            "oauth-token"
        );
        assert_eq!(
            detection_source_with(&no_override, &env_from(&[]), &no_oauth),
            "fallback-api"
        );
    }

    #[test]
    fn detection_source_override_label() {
        let override_api = || Some(BillingMode::Api);
        assert_eq!(
            detection_source_with(
                &override_api,
                &env_from(&[("SAVINGS_MIRROR_BILLING_MODE", "subscription")]),
                &yes_oauth
            ),
            "ui-override"
        );
    }

    #[test]
    fn override_roundtrip_api() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("billing-mode-override");
        assert_eq!(read_override_at(&path), None);
        write_override_at(&path, Some(BillingMode::Api)).expect("write");
        assert_eq!(read_override_at(&path), Some(BillingMode::Api));
    }

    #[test]
    fn override_roundtrip_subscription() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("billing-mode-override");
        write_override_at(&path, Some(BillingMode::Subscription)).expect("write");
        assert_eq!(read_override_at(&path), Some(BillingMode::Subscription));
    }

    #[test]
    fn override_clear_removes_file() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("billing-mode-override");
        write_override_at(&path, Some(BillingMode::Api)).expect("write");
        assert!(path.exists());
        write_override_at(&path, None).expect("clear");
        assert!(!path.exists());
        assert_eq!(read_override_at(&path), None);
    }

    #[test]
    fn override_clear_when_already_missing_is_ok() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("billing-mode-override");
        write_override_at(&path, None).expect("clear no-op");
    }

    #[test]
    fn override_auto_value_in_file_clears() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("billing-mode-override");
        std::fs::write(&path, "auto").expect("seed");
        assert_eq!(read_override_at(&path), None);
    }

    #[test]
    fn override_unknown_value_in_file_clears() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("billing-mode-override");
        std::fs::write(&path, "garbage").expect("seed");
        assert_eq!(read_override_at(&path), None);
    }

    #[test]
    fn override_creates_missing_parent_dir() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("nested/deeper/billing-mode-override");
        assert!(!path.parent().unwrap().exists());
        write_override_at(&path, Some(BillingMode::Subscription)).expect("write");
        assert!(path.exists());
        assert_eq!(read_override_at(&path), Some(BillingMode::Subscription));
    }

    #[tokio::test]
    async fn fetch_oauth_usage_parses_mock_response() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/oauth/usage")
            .match_header("authorization", "Bearer test-token")
            .match_header("anthropic-beta", "oauth-2025-04-20")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"five_hour":{"utilization":42.0,"resets_at":"2026-05-28T12:00:00Z"},
                    "seven_day":{"utilization":17.0,"resets_at":"2026-06-04T00:00:00Z"}}"#,
            )
            .create_async()
            .await;

        let usage = fetch_oauth_usage("test-token", &server.url())
            .await
            .expect("mock response must parse");
        assert!((usage.five_hour.utilization - 42.0).abs() < f64::EPSILON);
        assert_eq!(
            usage.seven_day.resets_at.as_deref(),
            Some("2026-06-04T00:00:00Z")
        );
        _m.assert_async().await;
    }

    #[tokio::test]
    async fn fetch_oauth_usage_surfaces_http_error() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/oauth/usage")
            .with_status(401)
            .with_body("unauthorized")
            .create_async()
            .await;

        let err = fetch_oauth_usage("bad-token", &server.url())
            .await
            .expect_err("401 must surface as Err");
        let msg = format!("{err}");
        assert!(msg.contains("401"), "error must mention status: {msg}");
    }

    // ---------- fetch_oauth_usage: additional error-path coverage ----------

    #[tokio::test]
    async fn fetch_oauth_usage_surfaces_403_forbidden() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/oauth/usage")
            .with_status(403)
            .with_body("forbidden")
            .create_async()
            .await;

        let err = fetch_oauth_usage("forbidden-token", &server.url())
            .await
            .expect_err("403 must surface as Err");
        let msg = format!("{err}");
        assert!(msg.contains("403"), "error must mention status: {msg}");
    }

    #[tokio::test]
    async fn fetch_oauth_usage_surfaces_429_rate_limit() {
        // Current impl does NOT retry on 429 — the error must surface so the
        // caller can decide. If a future tranche adds retry-with-backoff, this
        // test should be revisited (it will start failing on the retry path).
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/oauth/usage")
            .with_status(429)
            .with_header("retry-after", "30")
            .with_body("rate limited")
            .create_async()
            .await;

        let err = fetch_oauth_usage("rate-limited-token", &server.url())
            .await
            .expect_err("429 must surface as Err");
        let msg = format!("{err}");
        assert!(msg.contains("429"), "error must mention status: {msg}");
    }

    #[tokio::test]
    async fn fetch_oauth_usage_surfaces_503_server_error() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/oauth/usage")
            .with_status(503)
            .with_body("service unavailable")
            .create_async()
            .await;

        let err = fetch_oauth_usage("any-token", &server.url())
            .await
            .expect_err("5xx must surface as Err");
        let msg = format!("{err}");
        assert!(msg.contains("503"), "error must mention status: {msg}");
    }

    #[tokio::test]
    async fn fetch_oauth_usage_rejects_empty_body_on_200() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/oauth/usage")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("")
            .create_async()
            .await;

        let err = fetch_oauth_usage("ok-token", &server.url())
            .await
            .expect_err("empty body on 200 must fail to decode");
        let msg = format!("{err}");
        assert!(
            msg.contains("decode") || msg.to_lowercase().contains("eof"),
            "error must reference decode failure: {msg}"
        );
    }

    #[tokio::test]
    async fn fetch_oauth_usage_rejects_malformed_json_on_200() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/oauth/usage")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"five_hour": "not-an-object"}"#)
            .create_async()
            .await;

        let err = fetch_oauth_usage("ok-token", &server.url())
            .await
            .expect_err("malformed JSON on 200 must fail to decode");
        let msg = format!("{err}");
        assert!(
            msg.contains("decode"),
            "error must reference decode failure: {msg}"
        );
    }

    // Timeout case: SKIPPED.
    // The 5-second timeout in `fetch_oauth_usage` is enforced by reqwest's
    // client builder. Simulating it deterministically would require either
    // (a) a mockito feature for delayed responses (not in the 1.7 API surface
    // we depend on), or (b) a real TCP socket that accepts but never replies
    // — both add 5s of wall-clock time per CI run with marginal coverage
    // value. Reqwest's own test-suite covers the timeout primitive.

    // ---------- read_oauth_token_unix_at: filesystem coverage ----------

    #[test]
    fn read_oauth_token_unix_at_returns_token_on_valid_blob() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join(".credentials.json");
        std::fs::write(
            &path,
            r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat-deadbeef"}}"#,
        )
        .expect("write creds file");

        let token = read_oauth_token_unix_at(&path).expect("must find token");
        assert_eq!(token, "sk-ant-oat-deadbeef");
    }

    #[test]
    fn read_oauth_token_unix_at_returns_none_on_missing_file() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("does-not-exist.json");
        assert!(read_oauth_token_unix_at(&path).is_none());
    }

    #[test]
    fn read_oauth_token_unix_at_returns_none_on_malformed_json() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join(".credentials.json");
        std::fs::write(&path, "{ not valid json").expect("write garbage");
        assert!(read_oauth_token_unix_at(&path).is_none());
    }

    #[test]
    fn read_oauth_token_unix_at_returns_none_when_nested_key_missing() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join(".credentials.json");
        // Valid JSON, but no `claudeAiOauth.accessToken` path.
        std::fs::write(&path, r#"{"someOtherKey":{"foo":"bar"}}"#).expect("write json");
        assert!(read_oauth_token_unix_at(&path).is_none());
    }

    #[test]
    fn read_oauth_token_unix_at_returns_none_when_outer_key_present_but_inner_missing() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join(".credentials.json");
        // `claudeAiOauth` exists but lacks `accessToken`.
        std::fs::write(&path, r#"{"claudeAiOauth":{"refreshToken":"rt-only"}}"#)
            .expect("write json");
        assert!(read_oauth_token_unix_at(&path).is_none());
    }

    #[test]
    fn read_oauth_token_unix_at_returns_none_when_access_token_not_a_string() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join(".credentials.json");
        // `accessToken` exists but is a number — defensive coverage of as_str().
        std::fs::write(&path, r#"{"claudeAiOauth":{"accessToken":42}}"#).expect("write json");
        assert!(read_oauth_token_unix_at(&path).is_none());
    }
}
