#!/usr/bin/env python3
"""
Controle-run: meet ECHTE caveman-compressie via dubbele API-call per taak.

Voor elke prompt uit Julius' benchmark (10 taken):
  1. Run met baseline system-prompt → meet output_tokens
  2. Run met caveman-full system-prompt → meet output_tokens
  3. Bereken reduction_pct = (baseline - caveman) / baseline * 100

Schrijft CSV + samenvattend report naar ~/Desktop/controle-run-<datum>/.

Veiligheid:
  - Key wordt gelezen uit ~/.local/share/savings-mirror/anthropic-key.tmp
    (0600-perms), nooit geprint, nooit gelogd
  - Key-file wordt na succesvolle run gewist (atomisch)
  - Geen retries op auth-error om credit-verlies te voorkomen
  - Max-tokens cap per call zodat een runaway response niet $5 budget vreet
"""

import csv
import json
import os
import pathlib
import sys
import time
from datetime import datetime

KEY_FILE   = pathlib.Path.home() / ".local" / "share" / "savings-mirror" / "anthropic-key.tmp"
BENCH_FILE = pathlib.Path("/tmp/caveman-bench/prompts.json")
OUT_DIR    = pathlib.Path.home() / "Desktop" / f"controle-run-{datetime.utcnow().strftime('%Y%m%d-%H%M%S')}"

MODEL       = "claude-sonnet-4-5-20250929"   # actuele Sonnet 4.5 — pas aan indien deprecated
MAX_TOKENS  = 1024                            # cap per response, voorkom runaway

BASELINE_SYSTEM = "You are a helpful coding assistant. Answer clearly and completely."

# Caveman-full system-prompt — letterlijk uit ~/.claude/skills/caveman/SKILL.md
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
    key = KEY_FILE.read_text(encoding="utf-8").strip()
    if not key.startswith("sk-ant-"):
        sys.exit("key-file bevat geen sk-ant- prefix")
    return key


def call_anthropic(api_key: str, system: str, user: str) -> dict:
    """Doet één Messages-API call. Returnt {input_tokens, output_tokens, content_len}."""
    import urllib.request
    import urllib.error

    body = json.dumps({
        "model": MODEL,
        "max_tokens": MAX_TOKENS,
        "system": system,
        "messages": [{"role": "user", "content": user}],
    }).encode("utf-8")

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
        with urllib.request.urlopen(req, timeout=120) as resp:
            data = json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        sys.exit(f"HTTP {e.code}: {e.read().decode('utf-8')[:200]}")

    usage = data.get("usage", {})
    content_text = ""
    for block in data.get("content", []):
        if block.get("type") == "text":
            content_text += block.get("text", "")
    return {
        "input_tokens":  usage.get("input_tokens", 0),
        "output_tokens": usage.get("output_tokens", 0),
        "content_len":   len(content_text),
        "stop_reason":   data.get("stop_reason"),
    }


def main():
    api_key = load_key()
    if not BENCH_FILE.exists():
        sys.exit(f"missende benchmark: {BENCH_FILE}")

    prompts = json.loads(BENCH_FILE.read_text())["prompts"]
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    print(f"output → {OUT_DIR}")
    print(f"model  → {MODEL}")
    print(f"taken  → {len(prompts)}")
    print()

    results = []
    for i, p in enumerate(prompts):
        task_id = p["id"]
        user    = p["prompt"]
        print(f"[{i+1:2d}/{len(prompts)}] {task_id}")

        b = call_anthropic(api_key, BASELINE_SYSTEM, user)
        time.sleep(0.5)
        c = call_anthropic(api_key, CAVEMAN_SYSTEM, user)
        time.sleep(0.5)

        b_out = b["output_tokens"]
        c_out = c["output_tokens"]
        reduction_pct = ((b_out - c_out) / b_out * 100) if b_out else 0.0

        # Sonnet 4.5 pricing — input $3/M, output $15/M
        b_usd = (b["input_tokens"] * 3 + b_out * 15) / 1_000_000
        c_usd = (c["input_tokens"] * 3 + c_out * 15) / 1_000_000

        row = {
            "task":            task_id,
            "baseline_in":     b["input_tokens"],
            "baseline_out":    b_out,
            "caveman_in":      c["input_tokens"],
            "caveman_out":     c_out,
            "reduction_pct":   round(reduction_pct, 1),
            "baseline_usd":    round(b_usd, 5),
            "caveman_usd":     round(c_usd, 5),
            "cost_total_usd":  round(b_usd + c_usd, 5),
        }
        results.append(row)
        print(f"     baseline: {b_out:4d} tok   caveman: {c_out:4d} tok   reductie: {reduction_pct:+5.1f}%")

    # CSV
    csv_path = OUT_DIR / "results.csv"
    with csv_path.open("w", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=list(results[0].keys()))
        writer.writeheader()
        writer.writerows(results)

    # Samenvatting
    total_baseline = sum(r["baseline_out"] for r in results)
    total_caveman  = sum(r["caveman_out"]  for r in results)
    avg_reduction  = sum(r["reduction_pct"] for r in results) / len(results)
    total_cost     = sum(r["cost_total_usd"] for r in results)

    summary = {
        "model":              MODEL,
        "datum_utc":          datetime.utcnow().isoformat(),
        "taken":              len(results),
        "total_baseline_out": total_baseline,
        "total_caveman_out":  total_caveman,
        "global_reduction_pct": round((total_baseline - total_caveman) / total_baseline * 100, 1) if total_baseline else 0.0,
        "avg_reduction_pct":  round(avg_reduction, 1),
        "total_cost_usd":     round(total_cost, 4),
    }
    (OUT_DIR / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")

    md = (OUT_DIR / "REPORT.md")
    md.write_text(
        f"# Controle-run caveman-compressie\n\n"
        f"**Datum**: {summary['datum_utc']}\n"
        f"**Model**: `{MODEL}`\n"
        f"**Taken**: {summary['taken']} uit Julius' benchmark\n\n"
        f"## Globaal\n\n"
        f"- baseline tokens totaal: {total_baseline}\n"
        f"- caveman tokens totaal: {total_caveman}\n"
        f"- **globale reductie**: {summary['global_reduction_pct']}%\n"
        f"- gemiddelde reductie (per-taak): {summary['avg_reduction_pct']}%\n"
        f"- total kost: ${summary['total_cost_usd']}\n\n"
        f"## Per taak\n\n"
        f"| taak | baseline-out | caveman-out | reductie |\n|---|---:|---:|---:|\n"
        + "".join(f"| {r['task']} | {r['baseline_out']} | {r['caveman_out']} | {r['reduction_pct']}% |\n" for r in results),
        encoding="utf-8",
    )

    # Wis key
    try:
        KEY_FILE.unlink()
        print(f"\nkey-file gewist: {KEY_FILE}")
    except FileNotFoundError:
        pass

    print(f"\n=== KLAAR ===")
    print(f"globale reductie: {summary['global_reduction_pct']}%")
    print(f"gemiddelde reductie: {summary['avg_reduction_pct']}%")
    print(f"total kost: ${summary['total_cost_usd']}")
    print(f"resultaten: {OUT_DIR}")


if __name__ == "__main__":
    main()
