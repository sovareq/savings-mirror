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
            match billing::fetch_oauth_usage(&token, "https://api.anthropic.com").await {
                Ok(usage) => Json(json!({
                    "mode":         "subscription",
                    "usage":        usage,
                    "detected_via": detected_via,
                })),
                Err(e) => Json(json!({
                    "mode":         "subscription",
                    "usage":        Value::Null,
                    "detected_via": "oauth-fetch-failed",
                    "error":        e.to_string(),
                })),
            }
        }
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
        // backwards-compat aliases:
        "baseline_usd":           t.baseline_opus_usd,
        "savings_usd":            t.savings_usd,
        "savings_pct":            pct,
    })
}
