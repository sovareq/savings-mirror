//! Caveman transcript-parser. Ports the logic from `~/.claude/hooks/caveman-stats.js`
//! into Rust. Walks `~/.claude/projects/**/*.jsonl`, sums output-tokens per model,
//! applies the benchmark compression-factor (0.65 for 'full' mode), prices the
//! result against the public Anthropic output-token table, and returns per-day +
//! cumulative savings figures.
//!
//! Only output tokens are counted — caveman compresses the model's reply, not
//! the prompt. Input pricing is irrelevant for the savings claim.

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use walkdir::WalkDir;

use crate::mode_history::{self, ModeEntry};

/// USD per 1M output tokens, matched by model-id prefix.
const PRICE_TABLE: &[(&str, f64)] = &[
    ("claude-opus-4", 75.00),
    ("claude-sonnet-4", 15.00),
    ("claude-haiku-4", 4.00),
    ("claude-3-5-sonnet", 15.00),
    ("claude-3-5-haiku", 4.00),
    ("claude-3-opus", 75.00),
];

/// Anthropic public price per 1M output tokens for Opus. Used as the universal
/// "if the user had run this same prompt on Opus" baseline for the tier-savings
/// computation (real, measured savings from picking a cheaper tier).
pub const OPUS_PRICE_PER_M: f64 = 75.00;

fn price_per_million(model: &str) -> Option<f64> {
    for (prefix, price) in PRICE_TABLE {
        if model.starts_with(prefix) {
            return Some(*price);
        }
    }
    None
}

/// Savings broken into two honest layers:
///
/// 1. **tier_savings** — `if_opus_usd − actual_usd`. The user picked a cheaper
///    model (Haiku/Sonnet/etc.) than Opus and saved that delta. This is a
///    *real* measurement from public price tables, no estimation.
///
/// 2. **caveman_savings** — `if_opus_no_caveman_usd − if_opus_usd`. The model
///    output fewer tokens because caveman was active. This is an *estimate*
///    from `mode_history::compression_factor_for_call`, since the only way to
///    measure it precisely would be a parallel control-run without caveman.
///
/// `total_savings` is their sum. The struct also exposes a few backwards-
/// compatible alias fields (`baseline_usd`, `savings_usd`, `savings_pct`) so
/// older API consumers continue to work.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DailyBucket {
    pub date: String,
    pub calls: u64,
    /// Scenario 1: USD actually billed (used_model × out_tokens).
    pub actual_usd: f64,
    /// Scenario 2: same tokens, priced as if every call had run on Opus.
    pub if_opus_usd: f64,
    /// Scenario 3: scenario 2 expanded by the per-call caveman factor —
    /// i.e. the bigger Opus reply you would have got without compression.
    pub if_opus_no_caveman_usd: f64,
    /// `if_opus_usd − actual_usd` — real, measured (tier choice).
    pub tier_savings_usd: f64,
    /// `if_opus_no_caveman_usd − if_opus_usd` — estimated (compression).
    pub caveman_savings_usd: f64,
    pub total_savings_usd: f64,
    pub tier_savings_pct: f64,
    pub caveman_savings_pct: f64,
    pub total_savings_pct: f64,

    // -------- backwards-compatible aliases --------
    /// Alias for `if_opus_no_caveman_usd` (old "baseline" definition).
    pub baseline_usd: f64,
    /// Alias for `total_savings_usd`.
    pub savings_usd: f64,
    /// Alias for `total_savings_pct`.
    pub savings_pct: f64,
}

impl DailyBucket {
    /// Recompute every derived field (pct + aliases) from the raw accumulators.
    /// Call this after the per-call sums are final.
    fn finalise(&mut self) {
        self.total_savings_usd = self.tier_savings_usd + self.caveman_savings_usd;
        self.tier_savings_pct = if self.if_opus_usd > 0.0 {
            (self.tier_savings_usd / self.if_opus_usd) * 100.0
        } else {
            0.0
        };
        self.caveman_savings_pct = if self.if_opus_no_caveman_usd > 0.0 {
            (self.caveman_savings_usd / self.if_opus_no_caveman_usd) * 100.0
        } else {
            0.0
        };
        self.total_savings_pct = if self.if_opus_no_caveman_usd > 0.0 {
            (self.total_savings_usd / self.if_opus_no_caveman_usd) * 100.0
        } else {
            0.0
        };
        self.baseline_usd = self.if_opus_no_caveman_usd;
        self.savings_usd = self.total_savings_usd;
        self.savings_pct = self.total_savings_pct;
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CavemanReport {
    pub today: DailyBucket,
    pub last_7d: DailyBucket,
    pub cumulative: DailyBucket,
    pub per_day: Vec<DailyBucket>,
    /// Savings broken down by caveman-mode that was active at call-time. Keys
    /// are mode strings (`off`, `lite`, `full`, `ultra`, `wenyan-*`).
    pub by_mode: BTreeMap<String, DailyBucket>,
    pub mode_history_entries: u64,
    pub source_files: u64,
    pub assistant_messages: u64,
}

fn claude_projects_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".claude").join("projects")
}

fn baseline_file() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("savings-mirror")
        .join("baseline.txt")
}

/// Read the active baseline timestamp. If the file does not exist, write
/// "now" so future calls start counting from this moment (auto-zero on first
/// run).
pub fn load_or_init_baseline() -> DateTime<Utc> {
    let path = baseline_file();
    if let Ok(content) = std::fs::read_to_string(&path)
        && let Ok(ts) = DateTime::parse_from_rfc3339(content.trim())
    {
        return ts.with_timezone(&Utc);
    }
    let now = Utc::now();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&path, now.to_rfc3339()).ok();
    now
}

/// Reset the baseline to "now" — wipes the cache so future calls are counted
/// from this instant onwards. Does NOT touch sovacount.
pub fn reset_baseline() -> Result<DateTime<Utc>> {
    let path = baseline_file();
    let now = Utc::now();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&path, now.to_rfc3339()).context("writing baseline")?;
    if let Ok(mut guard) = REPORT_CACHE.lock() {
        *guard = None;
    }
    Ok(now)
}

/// Parse one transcript line (a JSONL record from Claude Code). We only care
/// about `assistant`-type messages with usage stats.
fn extract_usage(line: &str) -> Option<(DateTime<Utc>, String, u64)> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "assistant" {
        return None;
    }
    let ts = v.get("timestamp")?.as_str()?;
    let ts = DateTime::parse_from_rfc3339(ts).ok()?.with_timezone(&Utc);
    let msg = v.get("message")?;
    let model = msg.get("model")?.as_str()?.to_string();
    let usage = msg.get("usage")?;
    let out = usage.get("output_tokens")?.as_u64()?;
    Some((ts, model, out))
}

/// Cache the heavy `build_report_uncached` result so a runaway client (or a
/// stale browser tab still polling at 2s) cannot trigger a full
/// `~/.claude/projects/**/*.jsonl` walk on every request.
static REPORT_CACHE: Mutex<Option<(Instant, CavemanReport)>> = Mutex::new(None);
const REPORT_TTL: Duration = Duration::from_secs(10);

pub fn build_report() -> Result<CavemanReport> {
    if let Ok(guard) = REPORT_CACHE.lock()
        && let Some((t, r)) = guard.as_ref()
        && t.elapsed() < REPORT_TTL
    {
        return Ok(r.clone());
    }
    let fresh = build_report_uncached()?;
    if let Ok(mut guard) = REPORT_CACHE.lock() {
        *guard = Some((Instant::now(), fresh.clone()));
    }
    Ok(fresh)
}

fn build_report_uncached() -> Result<CavemanReport> {
    let root = claude_projects_dir();
    let baseline = load_or_init_baseline();
    // Make sure the history file has at least one entry, otherwise every call
    // would be classified as "off" until the JS hook records a transition.
    mode_history::seed_if_missing(baseline).ok();
    let history: Vec<ModeEntry> = mode_history::load_history();
    let mode_history_entries = history.len() as u64;

    let mut by_day: BTreeMap<NaiveDate, DailyBucket> = BTreeMap::new();
    let mut by_mode: BTreeMap<String, DailyBucket> = BTreeMap::new();
    let mut source_files = 0u64;
    let mut assistant_messages = 0u64;

    if !root.exists() {
        return Ok(CavemanReport {
            mode_history_entries,
            ..Default::default()
        });
    }

    for entry in WalkDir::new(&root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_type().is_file() && e.path().extension().is_some_and(|ext| ext == "jsonl")
        })
    {
        source_files += 1;
        let f = match File::open(entry.path()) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let reader = BufReader::new(f);
        for line in reader.lines().map_while(Result::ok) {
            let Some((ts, model, out_tokens)) = extract_usage(&line) else {
                continue;
            };
            if ts < baseline {
                continue; // pre-reset → skip
            }
            let Some(price) = price_per_million(&model) else {
                continue;
            };
            assistant_messages += 1;

            let tokens_m = out_tokens as f64 / 1_000_000.0;
            // Scenario 1: what was actually billed (cheaper-tier model × tokens).
            let actual_usd = tokens_m * price;
            // Scenario 2: same tokens at Opus rate. tier_savings = (2 − 1).
            let if_opus_usd = tokens_m * OPUS_PRICE_PER_M;
            // Scenario 3: Opus rate expanded by the inverse compression factor —
            // the longer reply we would have got without caveman, also on Opus.
            let mode = mode_history::mode_at(&history, ts);
            let factor = mode_history::compression_factor_for_call(mode, out_tokens);
            let if_opus_no_caveman_usd = if factor > 0.0 {
                if_opus_usd / (1.0 - factor)
            } else {
                if_opus_usd
            };
            let tier_savings_usd = if_opus_usd - actual_usd;
            let caveman_savings_usd = if_opus_no_caveman_usd - if_opus_usd;

            let date = ts.date_naive();
            let day = by_day.entry(date).or_insert_with(|| DailyBucket {
                date: date.to_string(),
                ..Default::default()
            });
            day.calls += 1;
            day.actual_usd += actual_usd;
            day.if_opus_usd += if_opus_usd;
            day.if_opus_no_caveman_usd += if_opus_no_caveman_usd;
            day.tier_savings_usd += tier_savings_usd;
            day.caveman_savings_usd += caveman_savings_usd;

            let mb = by_mode
                .entry(mode.to_string())
                .or_insert_with(|| DailyBucket {
                    date: mode.to_string(),
                    ..Default::default()
                });
            mb.calls += 1;
            mb.actual_usd += actual_usd;
            mb.if_opus_usd += if_opus_usd;
            mb.if_opus_no_caveman_usd += if_opus_no_caveman_usd;
            mb.tier_savings_usd += tier_savings_usd;
            mb.caveman_savings_usd += caveman_savings_usd;
        }
    }

    for mb in by_mode.values_mut() {
        mb.finalise();
    }

    // Finalise per-day pct + aliases.
    for b in by_day.values_mut() {
        b.finalise();
    }

    let today_date = Utc::now().date_naive();
    let today = by_day
        .get(&today_date)
        .cloned()
        .unwrap_or_else(|| DailyBucket {
            date: today_date.to_string(),
            ..Default::default()
        });

    let week_cut = today_date.num_days_from_ce() - 6;
    let mut last_7d = DailyBucket {
        date: format!("7d ending {today_date}"),
        ..Default::default()
    };
    let mut cumulative = DailyBucket {
        date: "all-time".into(),
        ..Default::default()
    };
    for (d, b) in &by_day {
        cumulative.calls += b.calls;
        cumulative.actual_usd += b.actual_usd;
        cumulative.if_opus_usd += b.if_opus_usd;
        cumulative.if_opus_no_caveman_usd += b.if_opus_no_caveman_usd;
        cumulative.tier_savings_usd += b.tier_savings_usd;
        cumulative.caveman_savings_usd += b.caveman_savings_usd;
        if d.num_days_from_ce() >= week_cut {
            last_7d.calls += b.calls;
            last_7d.actual_usd += b.actual_usd;
            last_7d.if_opus_usd += b.if_opus_usd;
            last_7d.if_opus_no_caveman_usd += b.if_opus_no_caveman_usd;
            last_7d.tier_savings_usd += b.tier_savings_usd;
            last_7d.caveman_savings_usd += b.caveman_savings_usd;
        }
    }
    last_7d.finalise();
    cumulative.finalise();

    Ok(CavemanReport {
        today,
        last_7d,
        cumulative,
        per_day: by_day.into_values().collect(),
        by_mode,
        mode_history_entries,
        source_files,
        assistant_messages,
    })
}

/// Append a NDJSON snapshot of the cumulative bucket. Idempotent — overwrites
/// the day's record if it already exists.
#[allow(dead_code)] // reserved for later persistence wiring; see data/
pub fn persist_snapshot(report: &CavemanReport) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let path = PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("caveman-dashboard")
        .join("savings.ndjson");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .context("opening NDJSON")?;
    let line = serde_json::to_string(&report.cumulative)?;
    writeln!(f, "{line}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode_history::compression_factor_for_call;

    /// Helper: build a finalised single-call DailyBucket using the same maths
    /// `build_report` runs in its inner loop.
    fn single_call(model: &str, out_tokens: u64, mode: &str) -> DailyBucket {
        let price = price_per_million(model).expect("known model");
        let tokens_m = out_tokens as f64 / 1_000_000.0;
        let actual_usd = tokens_m * price;
        let if_opus_usd = tokens_m * OPUS_PRICE_PER_M;
        let factor = compression_factor_for_call(mode, out_tokens);
        let if_opus_no_caveman_usd = if factor > 0.0 {
            if_opus_usd / (1.0 - factor)
        } else {
            if_opus_usd
        };
        let mut b = DailyBucket {
            calls: 1,
            actual_usd,
            if_opus_usd,
            if_opus_no_caveman_usd,
            tier_savings_usd: if_opus_usd - actual_usd,
            caveman_savings_usd: if_opus_no_caveman_usd - if_opus_usd,
            ..Default::default()
        };
        b.finalise();
        b
    }

    fn approx(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn tier_savings_zero_when_using_opus() {
        // 1M Opus output tokens → actual = $75, if_opus = $75 → tier delta = 0.
        let b = single_call("claude-3-opus", 1_000_000, "off");
        assert!(approx(b.actual_usd, 75.0, 1e-9));
        assert!(approx(b.if_opus_usd, 75.0, 1e-9));
        assert!(approx(b.tier_savings_usd, 0.0, 1e-9));
        // mode=off → no caveman component.
        assert!(approx(b.caveman_savings_usd, 0.0, 1e-9));
        assert!(approx(b.total_savings_usd, 0.0, 1e-9));
    }

    #[test]
    fn tier_savings_real_for_haiku() {
        // 1M Haiku output tokens → actual = $4, if_opus = $75 → tier = $71.
        // Mode is `off` so caveman_savings = 0 and total = tier.
        let b = single_call("claude-3-5-haiku", 1_000_000, "off");
        assert!(approx(b.actual_usd, 4.0, 1e-9));
        assert!(approx(b.if_opus_usd, 75.0, 1e-9));
        assert!(approx(b.tier_savings_usd, 71.0, 1e-9));
        assert!(approx(b.caveman_savings_usd, 0.0, 1e-9));
        assert!(approx(b.total_savings_usd, 71.0, 1e-9));
        // tier_pct = 71/75 ≈ 94.67%.
        assert!(approx(b.tier_savings_pct, 71.0 / 75.0 * 100.0, 1e-6));
    }

    #[test]
    fn caveman_savings_zero_when_off() {
        // Any model, mode=off → cf=0 → if_opus_no_caveman == if_opus.
        let b = single_call("claude-sonnet-4", 5_000, "off");
        assert!(approx(b.caveman_savings_usd, 0.0, 1e-9));
        assert!(approx(b.if_opus_no_caveman_usd, b.if_opus_usd, 1e-9));
        // total = tier only.
        assert!(approx(b.total_savings_usd, b.tier_savings_usd, 1e-9));
    }

    #[test]
    fn caveman_savings_real_when_full() {
        // 1M Sonnet output tokens, mode=full, 1M tokens → length-modifier near
        // its lower bound so cf ≈ 0.55 (base 0.65 minus 0.10 long-response).
        // if_opus = $75, if_opus_no_caveman = 75 / (1 − cf), caveman delta > 0.
        let b = single_call("claude-sonnet-4", 1_000_000, "full");
        assert!(b.caveman_savings_usd > 0.0);
        // Sanity: total = tier + caveman.
        assert!(approx(
            b.total_savings_usd,
            b.tier_savings_usd + b.caveman_savings_usd,
            1e-9,
        ));
        // Backwards-compat alias matches `total_savings_usd`.
        assert!(approx(b.savings_usd, b.total_savings_usd, 1e-9));
        assert!(approx(b.baseline_usd, b.if_opus_no_caveman_usd, 1e-9));
    }

    #[test]
    fn aliases_track_new_fields_after_finalise() {
        let b = single_call("claude-haiku-4", 100_000, "full");
        assert!(approx(b.baseline_usd, b.if_opus_no_caveman_usd, 1e-12));
        assert!(approx(b.savings_usd, b.total_savings_usd, 1e-12));
        assert!(approx(b.savings_pct, b.total_savings_pct, 1e-12));
    }
}
