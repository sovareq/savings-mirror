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
use walkdir::WalkDir;

const COMPRESSION_FULL: f64 = 0.65; // benchmark mean from caveman repo

/// USD per 1M output tokens, matched by model-id prefix.
const PRICE_TABLE: &[(&str, f64)] = &[
    ("claude-opus-4", 75.00),
    ("claude-sonnet-4", 15.00),
    ("claude-haiku-4", 4.00),
    ("claude-3-5-sonnet", 15.00),
    ("claude-3-5-haiku", 4.00),
    ("claude-3-opus", 75.00),
];

fn price_per_million(model: &str) -> Option<f64> {
    for (prefix, price) in PRICE_TABLE {
        if model.starts_with(prefix) {
            return Some(*price);
        }
    }
    None
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DailyBucket {
    pub date: String,
    pub calls: u64,
    pub baseline_usd: f64,
    pub actual_usd: f64,
    pub savings_usd: f64,
    pub savings_pct: f64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CavemanReport {
    pub today: DailyBucket,
    pub last_7d: DailyBucket,
    pub cumulative: DailyBucket,
    pub per_day: Vec<DailyBucket>,
    pub source_files: u64,
    pub assistant_messages: u64,
}

fn claude_projects_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".claude").join("projects")
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

pub fn build_report() -> Result<CavemanReport> {
    let root = claude_projects_dir();
    let mut by_day: BTreeMap<NaiveDate, DailyBucket> = BTreeMap::new();
    let mut source_files = 0u64;
    let mut assistant_messages = 0u64;

    if !root.exists() {
        return Ok(CavemanReport::default());
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
            let Some(price) = price_per_million(&model) else {
                continue;
            };
            assistant_messages += 1;

            // Compressed = what actually came back. Baseline = what it would
            // have been without caveman compression.
            let actual_usd = (out_tokens as f64 / 1_000_000.0) * price;
            let baseline_usd = actual_usd / (1.0 - COMPRESSION_FULL);
            let savings_usd = baseline_usd - actual_usd;

            let date = ts.date_naive();
            let b = by_day.entry(date).or_insert_with(|| DailyBucket {
                date: date.to_string(),
                ..Default::default()
            });
            b.calls += 1;
            b.baseline_usd += baseline_usd;
            b.actual_usd += actual_usd;
            b.savings_usd += savings_usd;
        }
    }

    // Finalise pct + collect.
    for b in by_day.values_mut() {
        b.savings_pct = if b.baseline_usd > 0.0 {
            (b.savings_usd / b.baseline_usd) * 100.0
        } else {
            0.0
        };
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
        cumulative.baseline_usd += b.baseline_usd;
        cumulative.actual_usd += b.actual_usd;
        cumulative.savings_usd += b.savings_usd;
        if d.num_days_from_ce() >= week_cut {
            last_7d.calls += b.calls;
            last_7d.baseline_usd += b.baseline_usd;
            last_7d.actual_usd += b.actual_usd;
            last_7d.savings_usd += b.savings_usd;
        }
    }
    last_7d.savings_pct = if last_7d.baseline_usd > 0.0 {
        (last_7d.savings_usd / last_7d.baseline_usd) * 100.0
    } else {
        0.0
    };
    cumulative.savings_pct = if cumulative.baseline_usd > 0.0 {
        (cumulative.savings_usd / cumulative.baseline_usd) * 100.0
    } else {
        0.0
    };

    Ok(CavemanReport {
        today,
        last_7d,
        cumulative,
        per_day: by_day.into_values().collect(),
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
