//! savings-mirror — local read-only savings tracker.
//!
//! Serves a brutalist HTML dashboard that visualises the USD savings reported
//! by `caveman::build_report`. Optionally augments those figures with totals
//! proxied (read-only) from a local cost-endpoint on `127.0.0.1:8989`.
//!
//! Binds to the address in env-var `BIND_ADDR` (default `127.0.0.1:8991`).
//! For LAN-access from a second machine, set `BIND_ADDR=0.0.0.0:8991`.
//!
//! All API endpoints return HTTP 200 even on failure, with `{"error": "..."}`
//! in the body, so the frontend can degrade gracefully without 500-handling.

#![forbid(unsafe_code)]

use axum::{
    Json, Router,
    response::Html,
    routing::{get, post},
};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::time::{Duration, Instant};

mod billing;
mod caveman;
mod mode_history;
mod sovacount;

/// TTL-cache for `billing::detect_mode()` so the 15-second `/api/combined`
/// polling loop (and the per-request `/api/billing` handler) don't shell out
/// to the macOS Keychain every poll. Five-minute TTL matches the frontend's
/// `loadBilling` cadence (`assets/dashboard.html`), so a user-visible env
/// change shows up on the dashboard within one full refresh window.
static BILLING_CACHE: Mutex<Option<(Instant, billing::BillingMode, &'static str)>> =
    Mutex::new(None);
const BILLING_CACHE_TTL: Duration = Duration::from_secs(300);

fn cached_billing() -> (billing::BillingMode, &'static str) {
    let mut cache = BILLING_CACHE.lock().expect("billing cache mutex poisoned");
    if let Some((at, mode, src)) = cache.as_ref()
        && at.elapsed() < BILLING_CACHE_TTL
    {
        return (*mode, *src);
    }
    let mode = billing::detect_mode();
    let src = billing::detection_source();
    *cache = Some((Instant::now(), mode, src));
    (mode, src)
}

/// Drop the cached billing decision so the next request re-runs the full
/// precedence chain. Called after a successful `POST /api/billing/override`
/// so the dashboard sees the new mode immediately, not after a 5-min TTL.
fn invalidate_billing_cache() {
    if let Ok(mut cache) = BILLING_CACHE.lock() {
        *cache = None;
    }
}

// T-SM-OAUTH-FIX-01 — caches successful `/api/oauth/usage` responses and
// honours Anthropic's 429 `retry-after` so we don't pile on rate-limit
// errors. The endpoint is rate-limited per-account; multiple browser
// refreshes + the 5-minute frontend `loadBilling` polling cadence can
// already exceed the limit if the user has another tool polling the same
// endpoint.
//
// Layout: two statics. `OAUTH_USAGE_CACHE` holds the last successful
// snapshot with a wall-clock timestamp; `OAUTH_RATE_LIMITED_UNTIL` holds
// a deadline parsed from `retry-after`. Both can be Some independently:
// when rate-limited we still return the cached snapshot (stale-while-
// rate-limited) so the dashboard keeps showing useful data instead of
// flickering to "data niet beschikbaar".
static OAUTH_USAGE_CACHE: Mutex<Option<(Instant, billing::OauthUsage)>> = Mutex::new(None);
static OAUTH_RATE_LIMITED_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);
const OAUTH_USAGE_TTL: Duration = Duration::from_secs(240); // 4 min, under-polls Anthropic
const OAUTH_DEFAULT_RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(300); // fallback if retry-after missing

/// Internal outcome of an `/api/oauth/usage` lookup.
enum OauthUsageOutcome {
    /// Fresh snapshot.
    Fresh(billing::OauthUsage),
    /// Cached snapshot returned because the live fetch failed (rate-limit
    /// or error). `note` describes why the live fetch wasn't used.
    Stale {
        snapshot: billing::OauthUsage,
        note: String,
    },
    /// No usable snapshot. `error` carries the human-readable reason.
    Unavailable { error: String },
}

/// Fetch `/api/oauth/usage` with cache + rate-limit deadline honour.
async fn cached_oauth_usage(token: &str) -> OauthUsageOutcome {
    // 1. Honour an active rate-limit deadline. If still pending, return
    //    the most recent cached snapshot (Stale) or Unavailable.
    {
        let until = OAUTH_RATE_LIMITED_UNTIL
            .lock()
            .expect("oauth rate-limit mutex poisoned");
        if let Some(deadline) = *until
            && Instant::now() < deadline
        {
            let secs_left = (deadline - Instant::now()).as_secs();
            let cache = OAUTH_USAGE_CACHE
                .lock()
                .expect("oauth usage cache mutex poisoned");
            return match cache.as_ref() {
                Some((_, snapshot)) => OauthUsageOutcome::Stale {
                    snapshot: snapshot.clone(),
                    note: format!(
                        "rate-limited by Anthropic; serving cached snapshot ({secs_left}s left)"
                    ),
                },
                None => OauthUsageOutcome::Unavailable {
                    error: format!(
                        "rate-limited by Anthropic; no cached snapshot yet ({secs_left}s left)"
                    ),
                },
            };
        }
    }

    // 2. Cache hit within TTL: return fresh without hitting the network.
    {
        let cache = OAUTH_USAGE_CACHE
            .lock()
            .expect("oauth usage cache mutex poisoned");
        if let Some((at, snapshot)) = cache.as_ref()
            && at.elapsed() < OAUTH_USAGE_TTL
        {
            return OauthUsageOutcome::Fresh(snapshot.clone());
        }
    }

    // 3. Network fetch.
    match billing::fetch_oauth_usage(token, "https://api.anthropic.com").await {
        Ok(usage) => {
            // Persist + clear any prior rate-limit deadline.
            if let Ok(mut cache) = OAUTH_USAGE_CACHE.lock() {
                *cache = Some((Instant::now(), usage.clone()));
            }
            if let Ok(mut until) = OAUTH_RATE_LIMITED_UNTIL.lock() {
                *until = None;
            }
            OauthUsageOutcome::Fresh(usage)
        }
        Err(e) => {
            let msg = e.to_string();
            if billing::is_rate_limited_err(&msg) {
                // Parse retry-after if present, otherwise fallback.
                let backoff = billing::parse_retry_after_from_err(&msg)
                    .map(Duration::from_secs)
                    .unwrap_or(OAUTH_DEFAULT_RATE_LIMIT_BACKOFF);
                if let Ok(mut until) = OAUTH_RATE_LIMITED_UNTIL.lock() {
                    *until = Some(Instant::now() + backoff);
                }
                let secs = backoff.as_secs();
                let cache = OAUTH_USAGE_CACHE
                    .lock()
                    .expect("oauth usage cache mutex poisoned");
                match cache.as_ref() {
                    Some((_, snapshot)) => OauthUsageOutcome::Stale {
                        snapshot: snapshot.clone(),
                        note: format!(
                            "rate-limited by Anthropic; serving cached snapshot ({secs}s deadline)"
                        ),
                    },
                    None => OauthUsageOutcome::Unavailable {
                        error: format!("rate-limited by Anthropic ({secs}s deadline): {msg}"),
                    },
                }
            } else {
                // Non-429 errors don't set a deadline. Surface cached snapshot
                // if available, so transient blips don't blank the dashboard.
                let cache = OAUTH_USAGE_CACHE
                    .lock()
                    .expect("oauth usage cache mutex poisoned");
                match cache.as_ref() {
                    Some((at, snapshot)) if at.elapsed() < OAUTH_USAGE_TTL * 4 => {
                        OauthUsageOutcome::Stale {
                            snapshot: snapshot.clone(),
                            note: format!("live fetch failed; serving cached snapshot: {msg}"),
                        }
                    }
                    _ => OauthUsageOutcome::Unavailable { error: msg },
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let bind = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8991".to_string());

    let app = Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        .route("/api/caveman", get(api_caveman))
        .route("/api/sovacount", get(api_sovacount))
        .route("/api/combined", get(api_combined))
        .route("/api/billing", get(api_billing))
        .route("/api/billing/override", post(api_billing_override))
        .route("/api/reset", post(api_reset));

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    eprintln!("savings-mirror listening on http://{bind}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../assets/dashboard.html"))
}

async fn health() -> &'static str {
    "ok"
}

async fn api_caveman() -> Json<Value> {
    Json(match caveman::build_report() {
        Ok(r) => serde_json::to_value(r).unwrap_or_else(|e| json!({ "error": e.to_string() })),
        Err(e) => json!({ "error": e.to_string() }),
    })
}

async fn api_reset() -> Json<Value> {
    Json(match caveman::reset_baseline() {
        Ok(ts) => {
            // Mirror the wipe on the mode-history file so the per-mode bucket
            // restarts from the same instant. Failure here is non-fatal: the
            // baseline wipe already took effect.
            let history_ok = mode_history::reset(ts).is_ok();
            json!({
                "ok":          true,
                "baseline":    ts.to_rfc3339(),
                "history_ok":  history_ok,
            })
        }
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    })
}

async fn api_sovacount() -> Json<Value> {
    Json(match sovacount::fetch_cost().await {
        Ok(Some(c)) => {
            serde_json::to_value(&c).unwrap_or_else(|e| json!({ "error": e.to_string() }))
        }
        Ok(None) => json!({ "error": "sovacount unreachable" }),
        Err(e) => json!({ "error": e.to_string() }),
    })
}

async fn api_combined() -> Json<Value> {
    let cav = caveman::build_report().ok();
    let sov = sovacount::fetch_cost().await.ok().flatten();

    let caveman_v = match &cav {
        Some(r) => json!({
            "today":      r.today,
            "last_7d":    r.last_7d,
            "cumulative": r.cumulative,
        }),
        None => Value::Null,
    };

    let sov_v = match &sov {
        Some(s) => {
            let today = chrono::Utc::now().date_naive().to_string();
            let today_tier = s.by_day.get(&today).cloned().unwrap_or_default();
            json!({
                "today":      tier_to_bucket(&today_tier, &today),
                "cumulative": tier_to_bucket(&s.totals, "all-time"),
            })
        }
        None => Value::Null,
    };

    let combined_v = match (&cav, &sov) {
        (Some(c), Some(s)) => {
            // SovaCount's totals are pure tier-savings: it doesn't know about
            // caveman compression. Combine them as such so the dashboard's
            // "tier-winst" column sums both sources honestly.
            let calls = c.cumulative.calls + s.totals.count;
            let actual = c.cumulative.actual_usd + s.totals.total_usd;
            let if_opus = c.cumulative.if_opus_usd + s.totals.baseline_opus_usd;
            // No caveman compression on the SovaCount side, so its if-opus-no-
            // caveman value equals its opus baseline.
            let if_opus_nc = c.cumulative.if_opus_no_caveman_usd + s.totals.baseline_opus_usd;
            let tier = c.cumulative.tier_savings_usd + s.totals.savings_usd;
            let caveman = c.cumulative.caveman_savings_usd;
            let total = tier + caveman;
            let out_tokens_total = c.cumulative.out_tokens_total + s.totals.out_tokens_total;
            let total_pct = if if_opus_nc > 0.0 {
                (total / if_opus_nc) * 100.0
            } else {
                0.0
            };
            let tier_pct = if if_opus > 0.0 {
                (tier / if_opus) * 100.0
            } else {
                0.0
            };
            let caveman_pct = if if_opus_nc > 0.0 {
                (caveman / if_opus_nc) * 100.0
            } else {
                0.0
            };
            json!({
                "date":                   "all sources",
                "calls":                  calls,
                "actual_usd":             actual,
                "if_opus_usd":            if_opus,
                "if_opus_no_caveman_usd": if_opus_nc,
                "tier_savings_usd":       tier,
                "caveman_savings_usd":    caveman,
                "total_savings_usd":      total,
                "tier_savings_pct":       tier_pct,
                "caveman_savings_pct":    caveman_pct,
                "total_savings_pct":      total_pct,
                "out_tokens_total":       out_tokens_total,
                // backwards-compat aliases:
                "baseline_usd":           if_opus_nc,
                "savings_usd":            total,
                "savings_pct":            total_pct,
            })
        }
        _ => Value::Null,
    };

    let (billing_mode, _) = cached_billing();

    Json(json!({
        "caveman":      caveman_v,
        "sovacount":    sov_v,
        "combined":     combined_v,
        "billing_mode": billing_mode.as_str(),
    }))
}

/// Billing-mode endpoint. Returns the detected mode plus, when subscription,
/// a best-effort utilization snapshot from Anthropic's `/api/oauth/usage`.
///
/// Always HTTP 200 — failure shapes are encoded in the body, matching the
/// rest of this codebase (see the module-level doc comment).
async fn api_billing() -> Json<Value> {
    let (mode, detected_via) = cached_billing();

    match mode {
        billing::BillingMode::Api | billing::BillingMode::Auto => Json(json!({
            "mode":         "api",
            "usage":        Value::Null,
            "detected_via": detected_via,
        })),
        billing::BillingMode::Subscription => {
            let token = match billing::read_oauth_token() {
                Some(t) => t,
                None => {
                    return Json(json!({
                        "mode":         "subscription",
                        "usage":        Value::Null,
                        "detected_via": "oauth-token-missing",
                        "error":        "no oauth access token available",
                    }));
                }
            };
            match cached_oauth_usage(&token).await {
                OauthUsageOutcome::Fresh(usage) => Json(json!({
                    "mode":         "subscription",
                    "usage":        usage,
                    "detected_via": detected_via,
                })),
                OauthUsageOutcome::Stale { snapshot, note } => Json(json!({
                    "mode":         "subscription",
                    "usage":        snapshot,
                    "detected_via": detected_via,
                    "stale":        true,
                    "note":         note,
                })),
                OauthUsageOutcome::Unavailable { error } => Json(json!({
                    "mode":         "subscription",
                    "usage":        Value::Null,
                    "detected_via": "oauth-fetch-failed",
                    "error":        error,
                })),
            }
        }
    }
}

/// Persist a UI-driven billing-mode override.
///
/// Body: `{"mode": "api" | "subscription" | "auto" | null}`.
/// - `"api"` / `"subscription"` → pin that mode (overrides env + OAuth).
/// - `"auto"` or `null` → clear override (back to auto-detect).
///
/// The cache is invalidated on success so the dashboard's next `loadBilling`
/// call sees the new mode without waiting for the 5-minute TTL.
async fn api_billing_override(Json(payload): Json<Value>) -> Json<Value> {
    let raw_mode = payload.get("mode");
    let parsed: Option<Option<billing::BillingMode>> = match raw_mode {
        Some(Value::Null) | None => Some(None), // clear
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "api" => Some(Some(billing::BillingMode::Api)),
            "subscription" => Some(Some(billing::BillingMode::Subscription)),
            "auto" | "" => Some(None), // clear
            _ => None,                 // unknown string
        },
        _ => None, // wrong JSON type
    };

    let target = match parsed {
        Some(t) => t,
        None => {
            return Json(json!({
                "ok":    false,
                "error": "mode must be one of: api, subscription, auto, null",
            }));
        }
    };

    match billing::write_override(target) {
        Ok(()) => {
            invalidate_billing_cache();
            let (mode, detected_via) = cached_billing();
            Json(json!({
                "ok":            true,
                "mode":          mode.as_str(),
                "detected_via":  detected_via,
                "override_active": target.is_some(),
            }))
        }
        Err(e) => Json(json!({
            "ok":    false,
            "error": e.to_string(),
        })),
    }
}

fn tier_to_bucket(t: &sovacount::TierBucket, label: &str) -> Value {
    // SovaCount is pure tier-savings (model-tier substitution). It has no
    // notion of caveman compression, so caveman_savings is zero and the
    // if-opus-no-caveman baseline collapses to the plain opus baseline.
    let pct = if t.baseline_opus_usd > 0.0 {
        (t.savings_usd / t.baseline_opus_usd) * 100.0
    } else {
        0.0
    };
    json!({
        "date":                   label,
        "calls":                  t.count,
        "actual_usd":             t.total_usd,
        "if_opus_usd":            t.baseline_opus_usd,
        "if_opus_no_caveman_usd": t.baseline_opus_usd,
        "tier_savings_usd":       t.savings_usd,
        "caveman_savings_usd":    0.0,
        "total_savings_usd":      t.savings_usd,
        "tier_savings_pct":       pct,
        "caveman_savings_pct":    0.0,
        "total_savings_pct":      pct,
        "out_tokens_total":       t.out_tokens_total,
        // backwards-compat aliases:
        "baseline_usd":           t.baseline_opus_usd,
        "savings_usd":            t.savings_usd,
        "savings_pct":            pct,
    })
}
