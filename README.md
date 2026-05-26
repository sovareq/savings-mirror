# savings-mirror

[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-edition_2024-orange.svg)](Cargo.toml)
[![Status](https://img.shields.io/badge/status-VERIFIED__GREEN-success.svg)](TEST-RESULTS.md)

Local, read-only USD savings tracker for the
[caveman](https://github.com/JuliusBrussee/caveman) Claude Code skill.

Parses your `~/.claude/projects/**/*.jsonl` transcripts, prices the output
tokens against Anthropic's public table, and shows what you actually saved by
(a) picking a cheaper model tier and (b) running caveman compression. Per-day,
7-day, cumulative, and per-mode breakdowns. No telemetry, no remote calls, no
write-back — pure offline consumer.

![dashboard screenshot](assets/screenshot.png)

> *Screenshot: brutalist mono dashboard at `http://127.0.0.1:8991/`.*

---

## Why

Caveman claims ~65% output-token reduction in `full` mode. You shouldn't have
to trust the claim — `savings-mirror` measures it on your real transcripts and
shows the dollar delta, broken into two honest layers:

1. **tier-savings** — `if_opus_usd − actual_usd`. Real, measured: you picked
   Haiku/Sonnet over Opus and saved the public price-table delta.
2. **caveman-savings** — `if_opus_no_caveman_usd − if_opus_usd`. Estimated from
   the per-call compression factor recorded by the mode-tracker hook. Only way
   to measure precisely would be a parallel control-run without caveman.

`total = tier + caveman`. Both columns are visible so you can judge each
claim on its own.

---

## Install

### macOS app (recommended)

```sh
git clone https://codeberg.org/sovareq_bv/savings-mirror.git
cd savings-mirror
./scripts/build-app.sh           # produces ~/Desktop/SavingsMirror.app
open ~/Desktop/SavingsMirror.app # menubar app, auto-starts the runtime
```

### From source

```sh
cargo build --release
./target/release/savings-mirror  # listens on 127.0.0.1:8991
open http://127.0.0.1:8991
```

### From a release tarball

Grab the latest `savings-mirror-<version>-<arch>.tar.gz` from the
[releases page](https://codeberg.org/sovareq_bv/savings-mirror/releases),
extract, and run the binary.

---

## Run alongside Claude Code (auto-start)

Add a `SessionStart` hook to `~/.claude/settings.json` so the runtime spawns
whenever Claude Code starts:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "pgrep -f 'SavingsMirror.app/Contents/MacOS' >/dev/null || SAVINGS_MIRROR_NO_DASHBOARD=1 open -gj /Users/<you>/Desktop/SavingsMirror.app",
            "timeout": 5
          }
        ]
      }
    ]
  }
}
```

The `pgrep` guard makes the hook idempotent. The `SAVINGS_MIRROR_NO_DASHBOARD=1`
env-var stops the launcher from spawning a browser tab on every session — open
the dashboard manually via the menubar icon (`SavingsMirror → Open dashboard`).

---

## Per-mode breakdown

Caveman exposes lite/full/ultra/wenyan modes. `savings-mirror` records every
mode transition via a Claude Code `UserPromptSubmit` hook
(`~/.claude/hooks/savings-mirror-mode-logger.js`), then attributes each
assistant-message's savings to whatever mode was active at call-time.

The dashboard's *"verdeling per modus"* section lists calls + USD saved per
mode. Modes with zero calls are hidden.

---

## Architecture

```
~/.claude/projects/**/*.jsonl
            │
            ▼
   ┌────────────────────┐
   │  caveman.rs        │   parses assistant-messages,
   │  build_report()    │   prices via PRICE_TABLE,
   └─────────┬──────────┘   applies per-mode factor
             │
             ▼
   ┌────────────────────┐
   │  axum HTTP server  │   /api/caveman    full report
   │  127.0.0.1:8991    │   /api/combined   + sovacount
   └─────────┬──────────┘   /api/reset      wipe baseline
             │
             ▼
   ┌────────────────────┐
   │  dashboard.html    │   brutalist mono, polls /api/combined
   │  (single file)     │   every 15s, 10s server-side cache
   └────────────────────┘
```

- **Cache**: `build_report()` is memoized for 10 seconds via `Mutex<Option<...>>`.
  Stops a runaway client (or a stale 2s-polling tab) from triggering a full
  transcript walk on every request.
- **Baseline**: `~/.local/share/savings-mirror/baseline.txt` holds the
  "count from this instant" timestamp. `POST /api/reset` rewrites it to now and
  wipes the mode-history file in lockstep.
- **Cost**: zero third-party crates other than axum/tokio/chrono/walkdir/serde.

---

## API

| Endpoint        | Method | Response                                    |
|-----------------|--------|---------------------------------------------|
| `/`             | GET    | `dashboard.html`                            |
| `/health`       | GET    | `"ok"`                                      |
| `/api/caveman`  | GET    | Full `CavemanReport` (today/7d/cum/by_mode) |
| `/api/sovacount`| GET    | SovaCount totals if `:8989` reachable       |
| `/api/combined` | GET    | Caveman + sovacount merged                  |
| `/api/reset`    | POST   | Wipe baseline + mode-history                |

All endpoints return HTTP 200 even on failure, with `{"error": "..."}` in the
body — frontend degrades gracefully without 500 handling.

`BIND_ADDR=0.0.0.0:8991 ./savings-mirror` exposes the dashboard on the LAN.

---

## What it doesn't do

- No write-back to caveman, sovacount, or your transcripts.
- No telemetry. No remote calls. No analytics.
- No mutations on disk other than `baseline.txt` and `mode-history.ndjson`
  under `~/.local/share/savings-mirror/`.
- No model invocations — 100% offline transcript parsing.

---

## Companion tool: sovacount

[sovacount](https://codeberg.org/sovareq_bv/sovacount) is a tier-router that
sits in front of the Anthropic API and downgrades calls to Haiku/Sonnet where
the user-facing quality budget allows. `savings-mirror` reads sovacount's
`/cost` endpoint and folds those savings into the same dashboard.

---

## License

MIT. Built as a companion tool to [caveman](https://github.com/JuliusBrussee/caveman)
by @JuliusBrussee — not affiliated, no warranty, no support contract.

Author: Bjorn Lambrechts ([Sovareq](https://sovareq.com)).
