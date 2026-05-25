//! caveman-dashboard — local read-only savings tracker.
//!
//! Serves a brutalist HTML dashboard on `127.0.0.1:8991` that visualises the
//! USD savings reported by `caveman::build_report`. Optionally augments those
//! figures with totals proxied (read-only) from SovaCount on `127.0.0.1:8989`.
//!
//! All API endpoints return HTTP 200 even on failure, with `{"error": "..."}`
//! in the body, so the frontend can degrade gracefully without 500-handling.

#![forbid(unsafe_code)]

use axum::{Json, Router, response::Html, routing::get};
use serde_json::{Value, json};

mod caveman;
mod sovacount;

const BIND_ADDR: &str = "127.0.0.1:8991";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        .route("/api/caveman", get(api_caveman))
        .route("/api/sovacount", get(api_sovacount))
        .route("/api/combined", get(api_combined));

    let listener = tokio::net::TcpListener::bind(BIND_ADDR).await?;
    eprintln!("caveman-dashboard listening on http://{BIND_ADDR}");
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
            let calls = c.cumulative.calls + s.totals.count;
            let baseline = c.cumulative.baseline_usd + s.totals.baseline_opus_usd;
            let actual = c.cumulative.actual_usd + s.totals.total_usd;
            let savings = c.cumulative.savings_usd + s.totals.savings_usd;
            let pct = if baseline > 0.0 {
                (savings / baseline) * 100.0
            } else {
                0.0
            };
            json!({
                "date":         "all sources",
                "calls":        calls,
                "baseline_usd": baseline,
                "actual_usd":   actual,
                "savings_usd":  savings,
                "savings_pct":  pct,
            })
        }
        _ => Value::Null,
    };

    Json(json!({
        "caveman":   caveman_v,
        "sovacount": sov_v,
        "combined":  combined_v,
    }))
}

fn tier_to_bucket(t: &sovacount::TierBucket, label: &str) -> Value {
    let pct = if t.baseline_opus_usd > 0.0 {
        (t.savings_usd / t.baseline_opus_usd) * 100.0
    } else {
        0.0
    };
    json!({
        "date":         label,
        "calls":        t.count,
        "baseline_usd": t.baseline_opus_usd,
        "actual_usd":   t.total_usd,
        "savings_usd":  t.savings_usd,
        "savings_pct":  pct,
    })
}
