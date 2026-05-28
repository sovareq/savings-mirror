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
}

impl Default for TierBucket {
    fn default() -> Self {
        Self {
            count: 0,
            total_usd: 0.0,
            baseline_opus_usd: 0.0,
            savings_usd: 0.0,
        }
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
    Ok(resp.json::<SovaCountCost>().await.ok())
}
