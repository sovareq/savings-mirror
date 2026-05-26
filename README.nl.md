# savings-mirror — Nederlands

[![Licentie: MIT](https://img.shields.io/badge/licentie-MIT-green.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-edition_2024-orange.svg)](Cargo.toml)
[![Status](https://img.shields.io/badge/status-VERIFIED__GREEN-success.svg)](TEST-RESULTS.md)

Lokale, alleen-lezen USD-besparing-tracker voor de
[caveman](https://github.com/JuliusBrussee/caveman) Claude Code skill.

Leest je `~/.claude/projects/**/*.jsonl` transcripten, prijst de output-tokens
af tegen Anthropics publieke prijstabel, en toont wat je écht bespaard hebt
door (a) een goedkoper model-tier te kiezen en (b) caveman-compressie te
gebruiken. Per dag, 7 dagen, cumulatief, en per modus. Geen telemetrie, geen
netwerkverkeer, geen write-back — pure offline lezer.

![dashboard screenshot](assets/screenshot.png)

> *Screenshot: brutalist mono dashboard op `http://127.0.0.1:8991/`.*

---

## Waarom

Caveman claimt ~65% reductie van output-tokens in `full` modus. Je hoeft die
claim niet blind te vertrouwen — `savings-mirror` meet hem op je eigen
transcripten en toont het dollar-verschil, opgesplitst in twee eerlijke lagen:

1. **tier-winst** — `if_opus_usd − actual_usd`. Echt gemeten: je koos
   Haiku/Sonnet boven Opus en bespaarde het verschil uit de publieke prijstabel.
2. **caveman-winst** — `if_opus_no_caveman_usd − if_opus_usd`. Schatting op
   basis van de per-call compressie-factor uit de modus-tracker hook. Exact
   meten zou een parallelle controlerun zonder caveman vereisen.

`totaal = tier + caveman`. Beide kolommen zijn zichtbaar, zodat je elke claim
los kan beoordelen.

---

## Installatie

### macOS app (aanbevolen)

```sh
git clone https://codeberg.org/sovareq_bv/savings-mirror.git
cd savings-mirror
./scripts/build-app.sh           # bouwt ~/Desktop/SavingsMirror.app
open ~/Desktop/SavingsMirror.app # menubar-app, start runtime automatisch
```

### Vanuit broncode

```sh
cargo build --release
./target/release/savings-mirror  # luistert op 127.0.0.1:8991
open http://127.0.0.1:8991
```

---

## Naast Claude Code laten meedraaien

Voeg een `SessionStart`-hook toe aan `~/.claude/settings.json` zodat de runtime
automatisch start wanneer Claude Code opstart:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "pgrep -f 'SavingsMirror.app/Contents/MacOS' >/dev/null || SAVINGS_MIRROR_NO_DASHBOARD=1 open -gj /Users/<jij>/Desktop/SavingsMirror.app",
            "timeout": 5
          }
        ]
      }
    ]
  }
}
```

De `pgrep`-guard maakt de hook idempotent. `SAVINGS_MIRROR_NO_DASHBOARD=1`
voorkomt dat de launcher elke sessie een browser-tab opent — open het
dashboard handmatig via het menubar-icoon (`SavingsMirror → Open dashboard`).

---

## Per-modus uitsplitsing

Caveman heeft lite/full/ultra/wenyan modi. `savings-mirror` registreert elke
modus-overgang via een Claude Code `UserPromptSubmit`-hook
(`~/.claude/hooks/savings-mirror-mode-logger.js`), en koppelt elk
assistant-bericht aan de modus die actief was op het moment van de call.

De *"verdeling per modus"*-sectie toont oproepen + USD bespaard per modus.
Modi met nul oproepen worden verborgen.

---

## Licentie

MIT. Gebouwd als companion-tool bij [caveman](https://github.com/JuliusBrussee/caveman)
van @JuliusBrussee — niet gelieerd, geen garantie, geen support-contract.

Auteur: Bjorn Lambrechts ([Sovareq](https://sovareq.com)).
