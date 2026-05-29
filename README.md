# savings-mirror

[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-edition_2024-orange.svg)](Cargo.toml)
[![Status](https://img.shields.io/badge/status-VERIFIED__GREEN-success.svg)](TEST-RESULTS.md)

Local, read-only savings tracker for the
[caveman](https://github.com/JuliusBrussee/caveman) Claude Code skill.

Parses your `~/.claude/projects/**/*.jsonl` transcripts, prices output
tokens against Anthropic's public table, and surfaces what caveman
compression + cheaper model tiers actually saved. Auto-detects whether
you pay per-token (Anthropic API) or run a flat-rate subscription
(Pro/Max/Team/Enterprise), and switches its primary metric:

- **API mode** → USD savings against the public price table.
- **Subscription mode** → tokens saved + 5-hour / 7-day cap utilization
  (USD shown as `≈ list-price` reference only).

Per-day, 7-day, cumulative, and per-mode breakdowns. No telemetry, no
remote calls beyond Anthropic's own `/api/oauth/usage` (subscription
mode only, requires your OAuth token), no write-back to your
transcripts.

![dashboard screenshot](assets/screenshot.png)

> *Screenshot: brutalist mono dashboard at `http://127.0.0.1:8991/`.*

---

## Two honest layers of savings

1. **tier-savings** — `if_opus_usd − actual_usd`. Real, measured: you
   picked Haiku/Sonnet over Opus and saved the public price-table
   delta.
2. **caveman-savings** — `if_opus_no_caveman_usd − if_opus_usd`.
   Estimated from the per-call compression factor recorded by the
   mode-tracker hook. Precise measurement would require a parallel
   control-run without caveman.

`total = tier + caveman`. In API mode both columns are visible so you
can judge each claim on its own. In subscription mode USD is hidden
and tokens-saved becomes the primary metric.

---

## Billing-mode detection

The dashboard's mode pill (top-right) shows the detected billing mode.
Click it to cycle `subscription → api → auto` and pin the override (the
pinned mode is persisted to
`~/.config/savings-mirror/billing-mode-override` and survives restarts).

Auto-detection precedence (`src/billing.rs::detect_mode`):

1. UI-override file (`~/.config/savings-mirror/billing-mode-override`).
2. Env `SAVINGS_MIRROR_BILLING_MODE` ∈ {`api`, `subscription`, `auto`}.
3. `ANTHROPIC_API_KEY` set → API.
4. `ANTHROPIC_AUTH_TOKEN` set → API.
5. `CLAUDE_CODE_OAUTH_TOKEN` env set → Subscription.
6. Local OAuth token reachable (macOS Keychain entry
   `Claude Code-credentials`, Linux/Windows
   `~/.claude/.credentials.json`) → Subscription.
7. Fallback → API.

In subscription mode the dashboard also calls Anthropic's
`/api/oauth/usage` for the 5-hour and 7-day utilization bars
(cached 4 min, honours 429 `retry-after` headers).

---

## Install

### macOS app (recommended)

```sh
git clone https://github.com/sovareq/savings-mirror.git
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
[releases page](https://github.com/sovareq/savings-mirror/releases),
extract, and run the binary.

---

## Run alongside Claude Code (auto-start)

Add a `SessionStart` hook to `~/.claude/settings.json` so the runtime
spawns whenever Claude Code starts:

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

The `pgrep` guard makes the hook idempotent. The
`SAVINGS_MIRROR_NO_DASHBOARD=1` env-var stops the launcher from
spawning a browser tab on every session — open the dashboard manually
via the menubar icon (`SavingsMirror → Open dashboard`).

---

## Per-mode breakdown

Caveman exposes lite/full/ultra/wenyan modes. `savings-mirror` records
every mode transition via a Claude Code `UserPromptSubmit` hook
(`~/.claude/hooks/savings-mirror-mode-logger.js`), then attributes each
assistant-message to whatever mode was active at call-time.

The dashboard's *"verdeling per modus"* section lists calls + (USD in
API mode / tokens-saved in subscription mode) per mode. Modes with
zero calls are hidden.

---

## Architecture

```
~/.claude/projects/**/*.jsonl
            │
            ▼
   ┌────────────────────┐
   │  caveman.rs        │  parses assistant-messages,
   │  build_report()    │  prices via PRICE_TABLE,
   └─────────┬──────────┘  applies per-mode factor
             │
             ▼
   ┌────────────────────┐  /api/caveman          full report
   │  axum HTTP server  │  /api/sovacount        sovacount totals (if :8989)
   │  127.0.0.1:8991    │  /api/combined         caveman + sovacount merged
   │                    │  /api/billing          mode + oauth usage
   │                    │  /api/billing/override pin or clear mode
   │                    │  /api/reset            wipe baseline + mode-history
   └─────────┬──────────┘  /health               "ok"
             │
             ▼
   ┌────────────────────┐
   │  dashboard.html    │  brutalist mono, single-file
   │                    │  polls /api/combined every 15s
   │                    │  polls /api/billing every 5 min
   └────────────────────┘  10s server-side cache on caveman report
```

- **Cache**: `build_report()` is memoized for 10 seconds via
  `Mutex<Option<...>>`. `/api/oauth/usage` results are cached 4
  minutes. `detect_mode()` is cached 5 minutes so the 15-second
  polling loop does not shell out to the macOS Keychain on every
  request.
- **Persistence**:
  - `~/.local/share/savings-mirror/baseline.txt` — "count from this
    instant" timestamp. `POST /api/reset` rewrites it to now.
  - `~/.local/share/savings-mirror/mode-history.ndjson` — append-only
    mode-transition log used to attribute calls.
  - `~/.config/savings-mirror/billing-mode-override` — single-line
    plain text (`api`/`subscription`); written by the dashboard pill
    click, absent file = no override.
- **Dependencies**: axum, tokio, serde, serde_json, walkdir, chrono,
  reqwest (for `/api/oauth/usage`), anyhow. Dev-only: mockito,
  tempfile.

---

## API

| Endpoint                  | Method | Response                                                          |
|---------------------------|--------|-------------------------------------------------------------------|
| `/`                       | GET    | `dashboard.html`                                                  |
| `/health`                 | GET    | `"ok"`                                                            |
| `/api/caveman`            | GET    | Full `CavemanReport` (today/7d/cum/by_mode)                       |
| `/api/sovacount`          | GET    | SovaCount totals if `:8989` reachable                             |
| `/api/combined`           | GET    | Caveman + sovacount merged + `billing_mode` field                 |
| `/api/billing`            | GET    | `{mode, detected_via, usage}` — usage only in subscription mode   |
| `/api/billing/override`   | POST   | Body `{mode: "api"\|"subscription"\|"auto"\|null}` — pin or clear |
| `/api/reset`              | POST   | Wipe baseline + mode-history                                      |

All endpoints return HTTP 200 even on failure, with `{"error": "..."}`
in the body — frontend degrades gracefully without 500 handling.

`BIND_ADDR=0.0.0.0:8991 ./savings-mirror` exposes the dashboard on the
LAN.

---

## What it doesn't do

- No write-back to caveman, sovacount, or your transcripts.
- No telemetry. No analytics. No tracking.
- Subscription-mode users: one remote call only, to
  `api.anthropic.com/api/oauth/usage`, with your own OAuth token, every
  4 minutes. API-mode users: zero remote calls.
- Disk writes are limited to the three files listed under
  *Persistence* above.
- No model invocations — savings figures come from transcript parsing,
  not from running a parallel control LLM.

---

## Companion tool: sovacount

[sovacount](https://github.com/sovareq/sovacount) is a separate process
that exposes a `/cost` endpoint summarising tier-routing savings
(Haiku/Sonnet vs Opus baseline) for prompts you route through it.
`savings-mirror` polls that endpoint post-hoc and folds the totals
into the same dashboard.

**Neither tool intercepts LLM traffic.** sovacount classifies and
records; savings-mirror reads. Both are pure consumers of work that
already happened — no proxy, no man-in-the-middle, no live mutation of
your API calls.

---

## License

MIT. Built as a companion tool to
[caveman](https://github.com/JuliusBrussee/caveman) by @JuliusBrussee —
not affiliated, no warranty, no support contract.

Author: Bjorn Lambrechts.
