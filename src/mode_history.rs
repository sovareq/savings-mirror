//! Per-call caveman-mode resolution.
//!
//! Reads the NDJSON history written by `~/.claude/hooks/savings-mirror-mode-logger.js`
//! and answers "which caveman mode was active at timestamp T?" via binary search.
//! Combined with `compression_factor`, this lets the parser apply the right
//! savings ratio per call instead of a blanket 0.65 — calls made while caveman
//! was `off` correctly produce zero savings.
//!
//! All file operations are best-effort: missing/corrupt history yields an empty
//! `Vec<ModeEntry>` and every lookup returns `"off"`.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeEntry {
    pub ts: DateTime<Utc>,
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

fn home_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home)
}

pub fn history_path() -> PathBuf {
    home_dir()
        .join(".local")
        .join("share")
        .join("savings-mirror")
        .join("mode-history.ndjson")
}

pub fn caveman_flag_path() -> PathBuf {
    let dir = std::env::var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".claude"));
    dir.join(".caveman-active")
}

/// Reads the live caveman-active flag. Empty / missing / unreadable → "off".
pub fn read_current_mode() -> String {
    match std::fs::read_to_string(caveman_flag_path()) {
        Ok(s) => {
            let t = s.trim();
            if t.is_empty() {
                "off".to_string()
            } else {
                t.to_string()
            }
        }
        Err(_) => "off".to_string(),
    }
}

/// Returns entries sorted by timestamp ascending. Corrupt lines are skipped.
pub fn load_history() -> Vec<ModeEntry> {
    let path = history_path();
    let Ok(file) = File::open(&path) else {
        return Vec::new();
    };
    let mut out: Vec<ModeEntry> = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<ModeEntry>(&l).ok())
        .collect();
    out.sort_by_key(|e| e.ts);
    out
}

/// Seed the history file with a single entry at the given baseline timestamp
/// if no file exists yet. Subsequent transitions are appended by the JS hook.
pub fn seed_if_missing(baseline: DateTime<Utc>) -> Result<()> {
    let path = history_path();
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let entry = ModeEntry {
        ts: baseline,
        mode: read_current_mode(),
        session: None,
    };
    let mut f = File::create(&path)?;
    writeln!(f, "{}", serde_json::to_string(&entry)?)?;
    Ok(())
}

/// Truncate history to a single entry `{ts: now, mode: current}`. Mirrors the
/// behaviour of `caveman::reset_baseline` so `/api/reset` wipes both files.
pub fn reset(now: DateTime<Utc>) -> Result<()> {
    let path = history_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let entry = ModeEntry {
        ts: now,
        mode: read_current_mode(),
        session: None,
    };
    let mut f = File::create(&path)?;
    writeln!(f, "{}", serde_json::to_string(&entry)?)?;
    Ok(())
}

/// Returns the mode active at `ts`. Lookups before the first entry → "off",
/// at-or-after an entry → that entry's mode (until superseded by a later one).
pub fn mode_at(history: &[ModeEntry], ts: DateTime<Utc>) -> &str {
    if history.is_empty() {
        return "off";
    }
    // Rightmost entry with entry.ts <= ts.
    match history.binary_search_by(|e| e.ts.cmp(&ts)) {
        Ok(i) => history[i].mode.as_str(),
        Err(0) => "off",
        Err(i) => history[i - 1].mode.as_str(),
    }
}

/// Per-mode output-compression base factor. Only `full` is benchmarked (0.65
/// mean over 10 tasks on sonnet-4); the rest are conservative estimates pending
/// further measurement. `off` and unknown modes return 0 so they contribute
/// zero savings.
pub fn compression_factor(mode: &str) -> f64 {
    match mode {
        "full" => 0.65,
        "lite" => 0.25,
        "ultra" => 0.75,
        "wenyan-lite" | "wenyan" | "wenyan-full" | "wenyan-ultra" => 0.50,
        _ => 0.0,
    }
}

/// Per-call factor combining `compression_factor(mode)` with a length-based
/// modifier. Short answers compress more aggressively relative to the
/// uncompressed baseline (caveman drops fluff that a normal reply would have
/// added); long answers compress less because their substance dominates.
///
/// Modifier sits in [-0.10, +0.10] around the base factor; the final value is
/// clamped to [0.20, 0.85] so no call escapes to absurd savings. Pivot is
/// 500 output tokens — the rough median of a Claude-Code assistant message.
///
/// Honest signal, not jitter: the input is `output_tokens`, a real measured
/// quantity from the transcript. Two calls with identical `out_tokens` produce
/// identical factors (deterministic).
pub fn compression_factor_for_call(mode: &str, out_tokens: u64) -> f64 {
    let base = compression_factor(mode);
    if base == 0.0 {
        return 0.0;
    }
    let len_factor = (500.0 / (out_tokens as f64).max(50.0)).clamp(0.0, 2.0);
    let modifier = (len_factor - 1.0) * 0.10; // ∈ [-0.10, +0.10]
    (base + modifier).clamp(0.20, 0.85)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn entries() -> Vec<ModeEntry> {
        vec![
            ModeEntry {
                ts: Utc.with_ymd_and_hms(2026, 5, 26, 1, 0, 0).unwrap(),
                mode: "off".into(),
                session: None,
            },
            ModeEntry {
                ts: Utc.with_ymd_and_hms(2026, 5, 26, 1, 10, 0).unwrap(),
                mode: "full".into(),
                session: None,
            },
            ModeEntry {
                ts: Utc.with_ymd_and_hms(2026, 5, 26, 1, 20, 0).unwrap(),
                mode: "off".into(),
                session: None,
            },
        ]
    }

    #[test]
    fn mode_at_before_any_entry() {
        let h = entries();
        let ts = Utc.with_ymd_and_hms(2026, 5, 26, 0, 30, 0).unwrap();
        assert_eq!(mode_at(&h, ts), "off");
    }

    #[test]
    fn mode_at_empty_history() {
        let h: Vec<ModeEntry> = Vec::new();
        let ts = Utc.with_ymd_and_hms(2026, 5, 26, 1, 0, 0).unwrap();
        assert_eq!(mode_at(&h, ts), "off");
    }

    #[test]
    fn mode_at_exact_match() {
        let h = entries();
        let ts = Utc.with_ymd_and_hms(2026, 5, 26, 1, 10, 0).unwrap();
        assert_eq!(mode_at(&h, ts), "full");
    }

    #[test]
    fn mode_at_between_entries() {
        let h = entries();
        let ts = Utc.with_ymd_and_hms(2026, 5, 26, 1, 15, 0).unwrap();
        assert_eq!(mode_at(&h, ts), "full");
    }

    #[test]
    fn mode_at_after_last_entry() {
        let h = entries();
        let ts = Utc.with_ymd_and_hms(2026, 5, 26, 5, 0, 0).unwrap();
        assert_eq!(mode_at(&h, ts), "off");
    }

    #[test]
    fn compression_factor_known_modes() {
        assert_eq!(compression_factor("off"), 0.0);
        assert_eq!(compression_factor("lite"), 0.25);
        assert_eq!(compression_factor("full"), 0.65);
        assert_eq!(compression_factor("ultra"), 0.75);
        assert_eq!(compression_factor("wenyan-full"), 0.50);
        assert_eq!(compression_factor("unknown-future-mode"), 0.0);
    }

    #[test]
    fn factor_for_call_within_clamp_range() {
        for tokens in [10u64, 50, 100, 500, 5000, 50_000] {
            let f = compression_factor_for_call("full", tokens);
            assert!((0.20..=0.85).contains(&f), "tokens={tokens}, factor={f}");
        }
    }

    #[test]
    fn factor_for_call_zero_for_off() {
        for tokens in [10u64, 500, 50_000] {
            assert_eq!(compression_factor_for_call("off", tokens), 0.0);
            assert_eq!(compression_factor_for_call("unknown", tokens), 0.0);
        }
    }

    #[test]
    fn factor_for_call_short_response_higher() {
        // Short response (50 tokens) ≥ pivot (500) → higher factor than long.
        let short = compression_factor_for_call("full", 50);
        let pivot = compression_factor_for_call("full", 500);
        let long = compression_factor_for_call("full", 5000);
        assert!(short > pivot, "short {short} should exceed pivot {pivot}");
        assert!(pivot > long, "pivot {pivot} should exceed long {long}");
    }

    #[test]
    fn factor_for_call_deterministic() {
        let a = compression_factor_for_call("full", 1234);
        let b = compression_factor_for_call("full", 1234);
        assert_eq!(a, b);
    }
}
