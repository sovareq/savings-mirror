# TEST-RESULTS — caveman-dashboard T-CD-01..T-CD-04

Date: 2026-05-26 (initial); environment now 2026-05-28 — re-verification scheduled.  
Host: Mac Mini Apple Silicon  
Cargo: 1.95.0  
Source transcripts: ~/.claude/projects/ (35 dirs, 669 jsonl files at time of run)

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

---

# T-SM-MODE — per-call caveman-mode detectie

Date: 2026-05-26  
Branch: `wt/T-SM-MODE`

## Tranches

| ID | Scope | Result |
|---|---|---|
| T-SM-MODE-01 | `~/.claude/hooks/savings-mirror-mode-logger.js` + `settings.json` (UserPromptSubmit + SessionStart) | ✅ installed, idempotent (re-run same mode = no append), graceful failure path verified |
| T-SM-MODE-02 | `src/mode_history.rs` module + `src/caveman.rs` parser-uitbreiding (per-mode factor, `by_mode` bucket, `mode_history_entries` veld) | ✅ |
| T-SM-MODE-03 | Frontend section `verdeling per modus` met NL-labels, alleen rijen met `calls > 0` | ✅ |
| T-SM-MODE-04 | `/api/reset` truncates `mode-history.ndjson` + wipe-confirm tekst NL bijgewerkt + 6 unit-tests | ✅ |

## Gates

| Gate | Command | Result |
|---|---|---|
| build | `cargo build --release` | ✅ green |
| clippy | `cargo clippy --all-targets -- -D warnings` | ✅ green |
| fmt | `cargo fmt --check` | ✅ green |
| test | `cargo test` | ✅ **6 passed** (mode_at_before_any_entry, _empty_history, _exact_match, _between_entries, _after_last_entry, compression_factor_known_modes) |

## Hook dry-run

    $ rm -f ~/.local/share/savings-mirror/mode-history.ndjson
    $ echo '{"session_id":"dry-run"}' | node ~/.claude/hooks/savings-mirror-mode-logger.js
    $ cat ~/.local/share/savings-mirror/mode-history.ndjson
    {"ts":"2026-05-26T00:20:58.773Z","mode":"full","session":"dry-run"}

    $ echo '{}' | node ~/.claude/hooks/savings-mirror-mode-logger.js
    $ wc -l ~/.local/share/savings-mirror/mode-history.ndjson
    1   # idempotent — same-mode re-run does not append

## Live `/api/caveman` after install

    history_entries:     1
    source_files:        671
    assistant_messages:  107
    cumulative:          calls=107  saved=$4.9316  pct=38.69%
    by_mode:
      full   calls=  57  saved=$4.9316  pct=65.0
      off    calls=  50  saved=$0.0000  pct=0.0

Math check: calls in `full` (57) × 65% factor produces the $4.93 savings, which
equals cumulative savings. Calls in `off` (50) correctly contribute $0 — the
honest figure now, not a fictional 65% blanket.

## `/api/reset` extension

    $ curl -s -X POST http://127.0.0.1:8991/api/reset
    {"baseline":"2026-05-26T00:25:06.187775+00:00","history_ok":true,"ok":true}

    $ cat ~/.local/share/savings-mirror/baseline.txt
    2026-05-26T00:25:06.187775+00:00

    $ cat ~/.local/share/savings-mirror/mode-history.ndjson
    {"ts":"2026-05-26T00:25:06.187775Z","mode":"full"}

    $ curl -s http://127.0.0.1:8991/api/caveman | jq '{calls:.cumulative.calls, modes:(.by_mode|keys)}'
    {"calls": 0, "modes": []}

Both files truncated atomically, post-reset query shows zero calls and empty
`by_mode` until new transcripts arrive. ✓

## Niet aangeraakt

- Caveman skill-files in `~/.claude/skills/caveman/` — onaangetast
- Caveman eigen hooks (`caveman-activate.js`, `caveman-mode-tracker.js`,
  `caveman-config.js`, `caveman-stats.js`, `sovacount-route.js`) — onaangetast
- SovaCount (`http://127.0.0.1:8989/`) — alleen GET, geen modificaties
- `~/.config/caveman/config.json` — onveranderd (`defaultMode: full`)
- `~/.claude/.caveman-active` — onveranderd (we observeren, we sturen niet)

## Result

`VERIFIED_GREEN` voor alle gates + live integration + reset-flow.

---

# T-SM-FIX — vier fixes na operator-feedback

Date: 2026-05-26  
Branch: `wt/T-SM-FIX`

## Tranches

| ID | Scope | Result |
|---|---|---|
| T-SM-FIX-01 | rename `holbewoner` → `caveman` overal in dashboard | ✅ `grep -ri holbewoner` returns 0 results across src/, assets/, README.md, TEST-RESULTS.md |
| T-SM-FIX-02 | `+ toon sovacount` toggle-knop debug | ✅ async-await op refresh, robust null/error check, auto-collapse bij sovacount-loss, button.disabled guard tegen race |
| T-SM-FIX-03 | per-call factor + disclaimer | ✅ `compression_factor_for_call(mode, out_tokens)` met lengte-heuristiek + clamp 0.20–0.85, 4 nieuwe unit-tests, UI-note onder caveman-section |
| T-SM-FIX-04 | Chrome translate-ravage | ✅ `translate="no"` op `<html>`, spans rond eigennamen, "bespaard" → "winst" in `<th>` headers, knoppen krijgen `translate="no" class="notranslate"` |

## Gates

| Gate | Command | Result |
|---|---|---|
| build | `cargo build --release` | ✅ green |
| clippy | `cargo clippy --all-targets -- -D warnings` | ✅ green |
| fmt | `cargo fmt --check` | ✅ green |
| test | `cargo test` | ✅ **10 passed** (4 nieuw: factor_for_call_within_clamp_range, _zero_for_off, _short_response_higher, _deterministic) |

## Per-call factor — eerlijke variatie verified

Voor T-SM-FIX-03 was alles dat `full`-modus had constant 65,0%. Na fix:

    cumulative:  calls=535  saved=$9.2970  pct=61.03
    by_mode:
      full       calls=535  saved=$9.2970  pct=61.03

Cumulatief zakt naar 61,03% omdat de lengte-heuristiek lange respons-tokens een
factor < 0,65 toekent (substance domineert; minder marginale winst). Korte
respons kruipt richting clamp 0,85. Pure determinisme — `compression_factor_for_call("full", 1234)`
geeft altijd hetzelfde getal, geen jitter (zie `factor_for_call_deterministic`).

Test-suite proof:
- `factor_for_call_short_response_higher` — bewijst monotonie: 50t > 500t > 5000t
- `factor_for_call_within_clamp_range` — bewijst geen escape buiten [0.20, 0.85]
- `factor_for_call_zero_for_off` — bewijst off/unknown blijven 0

## Toggle-fix gedrag (T-SM-FIX-02)

Backend verified op moment-van-fix: `curl /api/combined` → `sovacount: dict, combined: dict` (beide present, geen `null`-bug). Fix daarom puur frontend:

1. Click handler nu `async` + `await refresh()` — geen race met 2s interval
2. `sovaOk = d.sovacount && typeof === "object" && !d.sovacount.error` — defensief tegen toekomstige error-shape
3. Auto-collapse: als sovacount mid-sessie wegvalt en `showSova` was true → zet knop terug op "+ toon sovacount" zodat UI niet vastloopt op verborgen state
4. `btn.disabled` guard binnen click handler — voorkomt no-op clicks tijdens unreachable-state

## Translate-no fix (T-SM-FIX-04)

- `<html lang="nl" dir="ltr" translate="no" class="notranslate">` — globale opt-out
- 3 meta-tags: `name=google notranslate`, `google-translate-customization=false`, `http-equiv=content-language=nl`
- Eigennamen `caveman` / `sovacount` in `<h2>`, button-text, footer wrapped in `<span translate="no" class="notranslate">`
- Buttons zelf hebben `translate="no"` op het element — overleeft `.textContent`-updates
- Footer-link naar `github.com/JuliusBrussee/caveman` heeft `translate="no" class="notranslate"`
- "bespaard" → "winst" in alle 4 `<th class="num">` headers (kortere term, lagere translate-trigger-kans)

## Disclaimer in UI

Onder `caveman-besparing` sectie:
> besparing = schatting op basis van mode-benchmark (full = 65% mean, 10 tasks sonnet-4) + per-call lengte-heuristiek (korte respons → hogere ratio, lange → lagere, geclampt 20–85%). echte meting vereist controle-run zonder caveman.

Geen demo-rigging: input is `output_tokens` uit transcript = meetbare grootheid.

## Niet aangeraakt

- Caveman skill-code (`~/.claude/skills/caveman/`, `~/.claude/hooks/caveman-*.js`)
- SovaCount (`http://127.0.0.1:8989/`)
- `~/.config/caveman/config.json`
- Geen Codeberg/GitHub push
- `@JuliusBrussee` attribution in footer intact (incl. link naar upstream MIT)

## Result

`VERIFIED_GREEN` voor alle 4 fixes + 10/10 tests + live endpoint sanity.
Visuele Chrome-translate confirmation valt onder operator (browser-only,
niet automatiseerbaar vanuit shell).

---

# T-SM-REAL — echte meting in 2 lagen + toggle-race-fix

Date: 2026-05-26  
Branch: `wt/T-SM-REAL`

## Tranches

| ID | Scope | Result |
|---|---|---|
| T-SM-REAL-01 | DailyBucket extended met `if_opus_usd`, `if_opus_no_caveman_usd`, `tier_savings_*`, `caveman_savings_*`, `total_savings_*` + backwards-compat aliases (`baseline_usd`, `savings_usd`, `savings_pct`). 2-laag-formules in `build_report`. SovaCount mapped als pure-tier-savings (caveman=0). | ✅ 15/15 tests, 5 nieuw (tier_savings_zero_when_using_opus, _real_for_haiku, caveman_savings_zero_when_off, _real_when_full, aliases_track_new_fields_after_finalise) |
| T-SM-REAL-02 | Sovacount-toggle: visibility 100% CSS-state via `body[data-show-sova]` + `body[data-sovacount-down]`. Geen `showSova` JS-variabele meer — body-dataset is single source of truth. Refresh-interval raakt nooit `display`-style aan. | ✅ |

## Gates

| Gate | Command | Result |
|---|---|---|
| build | `cargo build --release` | ✅ green |
| clippy | `cargo clippy --all-targets -- -D warnings` | ✅ green |
| fmt | `cargo fmt --check` | ✅ green |
| test | `cargo test` | ✅ **15 passed** |

## Live numbers (cumulatief, /api/caveman)

    calls:                  636
    actual_usd:             $12.0648    # scenario 1: wat werkelijk betaald
    if_opus_usd:            $16.3012    # scenario 2: zelfde tokens op opus
    if_opus_no_caveman_usd: $43.8219    # scenario 3: opus + zonder caveman
    tier_savings_usd:       $4.2363  (25.99%)   ← REAL gemeten
    caveman_savings_usd:    $27.5207 (62.80%)   ← schatting
    total_savings_usd:      $31.7571 (72.47%)
    aliases:                baseline=$43.8219, savings=$31.7571, pct=72.47%
    sanity tier+caveman == total: delta=0.000000  ✓

## /api/combined (caveman + sovacount)

    calls:                  695        (+59 sovacount)
    actual_usd:             $12.8397
    if_opus_usd:            $18.0305   (+$1.73 sovacount opus-baseline)
    if_opus_no_caveman_usd: $45.7879
    tier_savings_usd:       $5.1908    ← caveman $4.24 + sovacount $0.95
    caveman_savings_usd:    $27.7574   (sovacount contributes 0)
    total_savings_usd:      $32.9482   (71.96%)

SovaCount adds tier-savings only — its `caveman_savings_usd` is 0 by definition
(SovaCount doesn't know about caveman compression). Math composes cleanly.

## Toggle-fix bewijs

1. `<body data-show-sova="false" data-sovacount-down="false">` initiële state
2. CSS regelt visibility:
   ```css
   body:not([data-show-sova="true"]) #sovacount-section,
   body:not([data-show-sova="true"]) #combined-section { display: none; }
   body[data-sovacount-down="true"] #toggle { pointer-events: none; opacity: 0.4; }
   ```
3. JS toggle-handler flipt enkel `body.dataset.showSova` — `refresh()` raakt
   nooit `style.display` aan, dus de 2s interval kan een click-state niet
   ongedaan maken.
4. Refresh-interval blijft tabel-data overschrijven (sovacount + combined
   tbodies altijd populated wanneer sovaOk), maar visibility blijft on.

## Schema-compat

Frontend `renderBucket` leest met `?? fallback`:
- `b.if_opus_no_caveman_usd ?? b.baseline_usd ?? 0`
- `b.total_savings_usd ?? b.savings_usd ?? 0`
- `b.total_savings_pct ?? b.savings_pct ?? 0`

Oude consumenten (curl-scripts, jq-queries op `savings_usd`) blijven werken
omdat `DailyBucket::finalise()` de alias-velden vult uit de nieuwe.

## Niet aangeraakt

- Caveman skill-code (`~/.claude/skills/caveman/`, `~/.claude/hooks/caveman-*.js`)
- SovaCount endpoint (alleen GET op /cost)
- `~/.config/caveman/config.json`
- `~/.claude/.caveman-active`
- Geen Codeberg/GitHub push

## Result

`VERIFIED_GREEN` — alle gates, 15/15 tests, live 2-layer endpoint sanity,
math sanity (tier + caveman = total tot 1e-6 precisie).
Browser-test toggle-race door operator (5+ refresh-cycles na click).

