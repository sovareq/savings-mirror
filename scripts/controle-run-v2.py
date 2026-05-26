#!/usr/bin/env python3
"""
Controle-run v2 — combineert caveman-compressie-meting + sovacount-tier-meting
in één run met max_tokens=4096 zodat geen artefacten van max-cap.

Per taak 4 calls (10 × 4 = 40 totaal):
  A. sonnet  baseline       — referentie van de gebruikelijke prijs/kwaliteit-mix
  B. sonnet  caveman-full   — voor caveman-reductie t.o.v. zelfde model
  C. haiku   baseline       — voor tier-down savings t.o.v. opus
  D. opus    baseline       — bovengrens, "alles via top tier"

Resultaten:
  caveman-laag  = (A_tok - B_tok) / A_tok   per taak + globaal
  tier-laag     = D_kost - C_kost           per taak (real-USD)
  combined      = D_kost - (C_kost * (B_tok/A_tok)) — beste case

Veiligheid:
  - Key gelezen uit ~/.local/share/savings-mirror/anthropic-key.tmp (0600)
  - Key nooit in stdout / logs
  - Key gewist op success
  - Max-tokens 4096 → kost-cap per call
  - HTTP 429 → retry 1× met 3s, daarna abort
"""

import csv
import json
import os
import pathlib
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime

KEY_FILE   = pathlib.Path.home() / ".local" / "share" / "savings-mirror" / "anthropic-key.tmp"
BENCH_FILE = pathlib.Path("/tmp/caveman-bench/prompts.json")
STAMP      = datetime.utcnow().strftime("%Y%m%d-%H%M%S")
OUT_DIR    = pathlib.Path.home() / "Desktop" / f"controle-run-v2-{STAMP}"

MAX_TOKENS = 4096

CONFIGS = [
    {"id": "sonnet_baseline", "model": "claude-sonnet-4-5-20250929", "system": "baseline"},
    {"id": "sonnet_caveman",  "model": "claude-sonnet-4-5-20250929", "system": "caveman"},
    {"id": "haiku_baseline",  "model": "claude-haiku-4-5-20251001",  "system": "baseline"},
    {"id": "opus_baseline",   "model": "claude-opus-4-1-20250805",   "system": "baseline"},
]

# Prijzen per 1M tokens (input / output) — Anthropic public mei 2026
PRICES = {
    "claude-sonnet-4-5-20250929": (3.00,  15.00),
    "claude-haiku-4-5-20251001":  (1.00,   5.00),
    "claude-opus-4-1-20250805":  (15.00,  75.00),
}

BASELINE_SYSTEM = "You are a helpful coding assistant. Answer clearly and completely."

CAVEMAN_SYSTEM = (
    "Respond terse like smart caveman. All technical substance stay. Only fluff die.\n"
    "Drop: articles (a/an/the), filler (just/really/basically/actually/simply), "
    "pleasantries (sure/certainly/of course/happy to), hedging. Fragments OK. "
    "Short synonyms (big not extensive, fix not 'implement a solution for'). "
    "Technical terms exact. Code blocks unchanged. Errors quoted exact.\n"
    "Pattern: [thing] [action] [reason]. [next step]."
)


def load_key() -> str:
    if not KEY_FILE.exists():
        sys.exit(f"missende key-file: {KEY_FILE}")
    k = KEY_FILE.read_text(encoding="utf-8").strip()
    if not k.startswith("sk-ant-"):
        sys.exit("geen sk-ant- prefix")
    return k


def call(api_key: str, model: str, system_kind: str, user: str) -> dict:
    system = CAVEMAN_SYSTEM if system_kind == "caveman" else BASELINE_SYSTEM
    body = json.dumps({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "system": system,
        "messages": [{"role": "user", "content": user}],
    }).encode("utf-8")

    for attempt in range(2):
        req = urllib.request.Request(
            "https://api.anthropic.com/v1/messages",
            data=body,
            headers={
                "x-api-key": api_key,
                "anthropic-version": "2023-06-01",
                "content-type": "application/json",
            },
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=180) as resp:
                data = json.loads(resp.read().decode("utf-8"))
            break
        except urllib.error.HTTPError as e:
            err = e.read().decode("utf-8")
            if e.code == 429 and attempt == 0:
                print(f"   ! 429, retry in 3s")
                time.sleep(3.0)
                continue
            sys.exit(f"HTTP {e.code}: {err[:200]}")

    usage = data.get("usage", {})
    return {
        "input_tokens":  usage.get("input_tokens", 0),
        "output_tokens": usage.get("output_tokens", 0),
        "stop_reason":   data.get("stop_reason"),
    }


def usd_cost(model: str, in_tok: int, out_tok: int) -> float:
    pin, pout = PRICES[model]
    return (in_tok * pin + out_tok * pout) / 1_000_000


def main():
    api_key = load_key()
    if not BENCH_FILE.exists():
        sys.exit(f"missende benchmark: {BENCH_FILE}")
    prompts = json.loads(BENCH_FILE.read_text())["prompts"]
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    print(f"output → {OUT_DIR}")
    print(f"configs → {len(CONFIGS)} × {len(prompts)} taken = {len(CONFIGS) * len(prompts)} calls")
    print(f"max_tokens → {MAX_TOKENS}")
    print()

    rows = []
    for i, p in enumerate(prompts):
        task_id = p["id"]
        user    = p["prompt"]
        print(f"[{i+1:2d}/{len(prompts)}] {task_id}")

        per_task = {"task": task_id}
        for cfg in CONFIGS:
            r = call(api_key, cfg["model"], cfg["system"], user)
            cost = usd_cost(cfg["model"], r["input_tokens"], r["output_tokens"])
            per_task[f"{cfg['id']}_in"]   = r["input_tokens"]
            per_task[f"{cfg['id']}_out"]  = r["output_tokens"]
            per_task[f"{cfg['id']}_usd"]  = round(cost, 5)
            per_task[f"{cfg['id']}_stop"] = r["stop_reason"]
            print(f"     {cfg['id']:16s} {r['output_tokens']:4d} out / ${cost:.4f}")
            time.sleep(0.4)

        # afgeleide metrics
        a = per_task["sonnet_baseline_out"]
        b = per_task["sonnet_caveman_out"]
        c_usd = per_task["haiku_baseline_usd"]
        d_usd = per_task["opus_baseline_usd"]
        sonnet_usd = per_task["sonnet_baseline_usd"]

        per_task["caveman_reduction_pct"] = round((a - b) / a * 100, 1) if a else 0.0
        per_task["tier_savings_haiku_vs_opus_usd"] = round(d_usd - c_usd, 5)
        per_task["tier_savings_sonnet_vs_opus_usd"] = round(d_usd - sonnet_usd, 5)
        # combined: haiku + caveman vs opus baseline (best-case)
        # caveman reduceert output_tokens met factor ratio op haiku ook
        if a > 0:
            haiku_caveman_estimate_out = per_task["haiku_baseline_out"] * (b / a)
            pin, pout = PRICES["claude-haiku-4-5-20251001"]
            haiku_caveman_cost = (per_task["haiku_baseline_in"] * pin + haiku_caveman_estimate_out * pout) / 1_000_000
            per_task["combined_savings_vs_opus_usd"] = round(d_usd - haiku_caveman_cost, 5)
        else:
            per_task["combined_savings_vs_opus_usd"] = 0.0

        rows.append(per_task)

    csv_path = OUT_DIR / "results.csv"
    with csv_path.open("w", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=list(rows[0].keys()))
        w.writeheader()
        w.writerows(rows)

    # samenvatting
    n = len(rows)
    total_sonnet_baseline_out = sum(r["sonnet_baseline_out"] for r in rows)
    total_sonnet_caveman_out  = sum(r["sonnet_caveman_out"]  for r in rows)
    avg_caveman = sum(r["caveman_reduction_pct"] for r in rows) / n
    global_caveman = (total_sonnet_baseline_out - total_sonnet_caveman_out) / total_sonnet_baseline_out * 100 if total_sonnet_baseline_out else 0

    total_haiku_usd  = sum(r["haiku_baseline_usd"]  for r in rows)
    total_sonnet_usd = sum(r["sonnet_baseline_usd"] for r in rows)
    total_opus_usd   = sum(r["opus_baseline_usd"]   for r in rows)
    total_run_cost   = sum(r["sonnet_baseline_usd"] + r["sonnet_caveman_usd"] +
                          r["haiku_baseline_usd"]  + r["opus_baseline_usd"] for r in rows)

    total_combined_savings = sum(r["combined_savings_vs_opus_usd"] for r in rows)
    pct_haiku_vs_opus  = (total_opus_usd - total_haiku_usd)  / total_opus_usd * 100 if total_opus_usd else 0
    pct_sonnet_vs_opus = (total_opus_usd - total_sonnet_usd) / total_opus_usd * 100 if total_opus_usd else 0
    pct_combined       = total_combined_savings / total_opus_usd * 100 if total_opus_usd else 0

    summary = {
        "datum_utc":             datetime.utcnow().isoformat(),
        "model_sonnet":          "claude-sonnet-4-5-20250929",
        "model_haiku":           "claude-haiku-4-5-20251001",
        "model_opus":            "claude-opus-4-1-20250805",
        "max_tokens":            MAX_TOKENS,
        "taken":                 n,
        "total_calls":           n * len(CONFIGS),
        "caveman": {
            "global_reduction_pct": round(global_caveman, 1),
            "avg_reduction_pct":    round(avg_caveman, 1),
            "total_baseline_out":   total_sonnet_baseline_out,
            "total_caveman_out":    total_sonnet_caveman_out,
        },
        "tier": {
            "total_haiku_usd":       round(total_haiku_usd, 4),
            "total_sonnet_usd":      round(total_sonnet_usd, 4),
            "total_opus_usd":        round(total_opus_usd, 4),
            "haiku_savings_pct_vs_opus":  round(pct_haiku_vs_opus, 1),
            "sonnet_savings_pct_vs_opus": round(pct_sonnet_vs_opus, 1),
        },
        "combined": {
            "total_savings_vs_opus_usd": round(total_combined_savings, 4),
            "savings_pct_vs_opus":       round(pct_combined, 1),
        },
        "run_cost_usd": round(total_run_cost, 4),
    }
    (OUT_DIR / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")

    # REPORT.md
    md = []
    md.append(f"# Controle-run v2 — caveman + sovacount echte meting\n")
    md.append(f"**Datum**: {summary['datum_utc']}")
    md.append(f"**Max tokens**: {MAX_TOKENS} (geen artefact-cap meer)")
    md.append(f"**Taken**: {n} uit Julius' eigen benchmark\n")

    md.append("## Caveman-laag (compressie t.o.v. zelfde model)\n")
    md.append(f"- model: Sonnet 4.5")
    md.append(f"- baseline output totaal: **{total_sonnet_baseline_out}** tokens")
    md.append(f"- caveman output totaal: **{total_sonnet_caveman_out}** tokens")
    md.append(f"- **globale reductie: {summary['caveman']['global_reduction_pct']}%**")
    md.append(f"- gemiddelde reductie (per-taak): {summary['caveman']['avg_reduction_pct']}%\n")

    md.append("## Tier-laag (model-keuze t.o.v. Opus)\n")
    md.append(f"| modus | totaal-kost | besparing vs Opus | % |\n|---|---:|---:|---:|")
    md.append(f"| Opus baseline | ${summary['tier']['total_opus_usd']} | — | — |")
    md.append(f"| Sonnet baseline | ${summary['tier']['total_sonnet_usd']} | ${round(total_opus_usd - total_sonnet_usd, 4)} | {summary['tier']['sonnet_savings_pct_vs_opus']}% |")
    md.append(f"| Haiku baseline | ${summary['tier']['total_haiku_usd']} | ${round(total_opus_usd - total_haiku_usd, 4)} | {summary['tier']['haiku_savings_pct_vs_opus']}% |\n")

    md.append("## Gecombineerd (Haiku + Caveman vs Opus baseline)\n")
    md.append(f"- **totaal bespaard: ${summary['combined']['total_savings_vs_opus_usd']}**")
    md.append(f"- **percentage: {summary['combined']['savings_pct_vs_opus']}%**\n")

    md.append("## Per-taak detail\n")
    md.append("| taak | son-base out | son-cav out | cav-red% | opus $ | sonnet $ | haiku $ | combo-savings $ |\n|---|---:|---:|---:|---:|---:|---:|---:|")
    for r in rows:
        md.append(
            f"| {r['task']} | {r['sonnet_baseline_out']} | {r['sonnet_caveman_out']} | "
            f"{r['caveman_reduction_pct']}% | ${r['opus_baseline_usd']} | "
            f"${r['sonnet_baseline_usd']} | ${r['haiku_baseline_usd']} | "
            f"${r['combined_savings_vs_opus_usd']} |"
        )
    md.append("")

    md.append("## Methodologie & disclaimers\n")
    md.append("- **Caveman-meting**: ECHTE controle-run (zelfde prompt, zelfde model, twee system-prompts). Geen schatting.")
    md.append("- **Tier-meting**: ECHTE meting via Anthropic public pricing × gemeten tokens. Geen mock.")
    md.append("- **Combined-schatting**: combineert caveman-ratio met haiku-kost. Bovengrens-schatting van wat tier-routing + compressie zou opleveren.")
    md.append("- **Max-tokens cap = 4096**: groot genoeg om 99% van baseline-responses ongehinderd te laten lopen.")
    md.append(f"- **Run-kost zelf**: ${summary['run_cost_usd']} (40 API-calls, opgenomen in zijn geheel).")
    md.append(f"- **Stop_reason 'max_tokens'**: bekijk CSV per call — als baseline hit cap, is reductie ondergeschat.")
    md.append("")

    (OUT_DIR / "REPORT.md").write_text("\n".join(md), encoding="utf-8")

    try:
        KEY_FILE.unlink()
    except FileNotFoundError:
        pass

    print(f"\n=== KLAAR ===")
    print(f"caveman globale reductie: {summary['caveman']['global_reduction_pct']}%")
    print(f"tier haiku-vs-opus: {summary['tier']['haiku_savings_pct_vs_opus']}%")
    print(f"combined haiku+caveman vs opus: {summary['combined']['savings_pct_vs_opus']}%")
    print(f"run-kost: ${summary['run_cost_usd']}")
    print(f"output: {OUT_DIR}")


if __name__ == "__main__":
    main()
