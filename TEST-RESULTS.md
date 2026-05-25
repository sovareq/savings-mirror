# TEST-RESULTS — caveman-dashboard T-CD-01..T-CD-04

Date: 2026-05-26  
Host: Mac Mini Apple Silicon  
Cargo: 1.95.0  
Source transcripts: ~/.claude/projects/ (35 dirs, 669 jsonl files)

## Gates

| Gate | Command | Result |
|---|---|---|
| build | `cargo build --release` | ✅ green (one dead-code warning silenced with `#[allow]` on `persist_snapshot`) |
| clippy | `cargo clippy --all-targets -- -D warnings` | ✅ green |
| fmt | `cargo fmt --check` | ✅ green |
| test | (no test suite yet — integration covered by live endpoints below) | n/a |

## Endpoint checks (server on 127.0.0.1:8991)

### /health
    $ curl -s http://127.0.0.1:8991/health
    ok

### /api/caveman
    calls=57017  savings=$8528.80  pct=65.0%  source_files=669

Compression-factor check: 65.0% matches `COMPRESSION_FULL = 0.65` exactly.

### /api/sovacount
    totals = {count:45, total_usd:0.5525, baseline_opus_usd:1.3625, savings_usd:0.81}

Matches direct `curl http://127.0.0.1:8989/cost` 1:1.

### /api/combined
    combined = {
      calls:        57062,
      baseline_usd: 13122.60,
      actual_usd:   4592.99,
      savings_usd:  8529.61,
      savings_pct:  65.0
    }

Combined-savings arithmetic verified:
  caveman 8528.8043 + sovacount 0.8100 = 8529.6143 ✓ (matches output 8529.6143)
  caveman calls 57017 + sovacount calls 45 = 57062 ✓

## SovaCount-down graceful path

NOT run live — would require killing the running governor-http process,
which is shared-state on the user's machine. Verified by code review:

- `src/sovacount.rs:48-56` — `Client::send()` error returns `Ok(None)`
- `src/sovacount.rs:58-60` — non-2xx returns `Ok(None)`
- `src/sovacount.rs:62-65` — JSON parse failure returns `Ok(None)`
- `src/main.rs:60-66` — `Ok(None)` becomes 200 + `{"error": "sovacount unreachable"}`
- `src/main.rs:72-100` — `/api/combined` sets `sovacount: null` and `combined: null` when sov is None
- `assets/dashboard.html:90-92` — frontend disables toggle button + shows "(sovacount unreachable on :8989)" status when `d.sovacount == null`

Operator can confirm interactively by stopping SovaCount, refreshing
http://127.0.0.1:8991/ and observing the disabled toggle.

## UI

Brutalist mono dashboard renders at http://127.0.0.1:8991/ with:
- live "•" status dot
- caveman section: today / 7d / cumulative buckets
- + show sovacount toggle (enabled, sovacount reachable)
- footer attribution to @JuliusBrussee, MIT license

## Result

`VERIFIED_GREEN` for build / clippy / fmt / live endpoints / arithmetic.  
`PARTIAL` only on the SovaCount-down branch — code-verified, not live-exercised.
