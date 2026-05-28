//! Read-only proxy to the local SovaCount endpoint at `http://127.0.0.1:8989/cost`.
//! Never writes, never POSTs — just GETs JSON and reshapes it.
//!
//! Returns `Ok(None)` on every reachable failure (connection refused, timeout,
//! non-2xx, malformed body) so callers can gracefully fall back to caveman-only
//! mode instead of bubbling an error.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

use crate::caveman::OPUS_PRICE_PER_M;

const SOVACOUNT_URL: &str = "http://127.0.0.1:8989/cost";
const TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierBucket {
    #[serde(default)]
    pub count: u64,
    #[serde(default)]
    pub total_usd: f64,
    #[serde(default)]
    pub baseline_opus_usd: f64,
    #[serde(default)]
    pub savings_usd: f64,
    /// Tokens-equivalent. Indien sovacount sidecar dit niet exporteert,
    /// geschat als baseline_opus_usd / $75 per M output-tokens (estimator).
    #[serde(default)]
    pub out_tokens_total: u64,
}

impl Default for TierBucket {
    fn default() -> Self {
        Self {
            count: 0,
            total_usd: 0.0,
            baseline_opus_usd: 0.0,
            savings_usd: 0.0,
            out_tokens_total: 0,
        }
    }
}

/// Estimate output-tokens from the Opus baseline-USD value. Used as fallback
/// when the upstream sovacount sidecar does not export a tokens field — we
/// invert the public Opus output-token price ($75 per 1M tokens, see
/// `caveman::OPUS_PRICE_PER_M`) to get a tokens-equivalent for the dashboard.
pub(crate) fn estimate_tokens_from_opus_usd(baseline_opus_usd: f64) -> u64 {
    if baseline_opus_usd <= 0.0 {
        return 0;
    }
    (baseline_opus_usd / OPUS_PRICE_PER_M * 1_000_000.0) as u64
}

/// Apply the fallback estimator in-place: only fills `out_tokens_total` when
/// the sidecar didn't report it (value still 0) and there is a positive
/// baseline_opus_usd to derive it from.
fn fill_tokens_estimate(b: &mut TierBucket) {
    if b.out_tokens_total == 0 && b.baseline_opus_usd > 0.0 {
        b.out_tokens_total = estimate_tokens_from_opus_usd(b.baseline_opus_usd);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovaCountCost {
    pub totals: TierBucket,
    #[serde(default)]
    pub by_tier: BTreeMap<String, TierBucket>,
    #[serde(default)]
    pub by_day: BTreeMap<String, TierBucket>,
}

pub async fn fetch_cost() -> Result<Option<SovaCountCost>> {
    let client = match reqwest::Client::builder().timeout(TIMEOUT).build() {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    let resp = match client.get(SOVACOUNT_URL).send().await {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };
    if !resp.status().is_success() {
        return Ok(None);
    }
    let mut cost = match resp.json::<SovaCountCost>().await.ok() {
        Some(c) => c,
        None => return Ok(None),
    };
    // Pad 2 fallback: when the sidecar omits out_tokens_total, derive it from
    // baseline_opus_usd. Pad 1 (sidecar exports tokens) is already covered by
    // `#[serde(default)]` on the field — `fill_tokens_estimate` is a no-op
    // when the value is already > 0.
    fill_tokens_estimate(&mut cost.totals);
    for b in cost.by_tier.values_mut() {
        fill_tokens_estimate(b);
    }
    for b in cost.by_day.values_mut() {
        fill_tokens_estimate(b);
    }
    Ok(Some(cost))
}
