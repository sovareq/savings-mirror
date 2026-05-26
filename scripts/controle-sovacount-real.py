#!/usr/bin/env python3
"""
SovaCount-real controle-run.

Voor elk van de 10 Julius-prompts:
  1. POST naar sovacount-real /classify (port 8990, GOVERNOR_PROVIDER=anthropic)
  2. Krijg tier-suggestie + confidence + rationale
  3. Log

Daarna: voor de top-3 prompts waar SovaCount Haiku/Sonnet voorstelt (downgrade van Opus):
  - Doe een ECHTE call op DIE tier
  - Doe een ECHTE call op Opus (referentie)
  - Vergelijk output_tokens + kost + (voor mens-evaluatie) toon beide responses
  - Real classification-accuracy = of SovaCount's keuze acceptabel was

Schrijft alles naar ~/Desktop/sovacount-real-<datum>/.
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

KEY_FILE      = pathlib.Path.home() / ".local" / "share" / "savings-mirror" / "anthropic-key.tmp"
BENCH_FILE    = pathlib.Path("/tmp/caveman-bench/prompts.json")
STAMP         = datetime.utcnow().strftime("%Y%m%d-%H%M%S")
OUT_DIR       = pathlib.Path.home() / "Desktop" / f"sovacount-real-{STAMP}"
SOVACOUNT_URL = "http://127.0.0.1:8990"

TIER_MODEL = {
    "hk": "claude-haiku-4-5-20251001",
    "so": "claude-sonnet-4-5-20250929",
    "op": "claude-opus-4-1-20250805",
}
TIER_PRICE = {
    "claude-haiku-4-5-20251001":  (1.00,  5.00),
    "claude-sonnet-4-5-20250929": (3.00, 15.00),
    "claude-opus-4-1-20250805": (15.00, 75.00),
}

BASELINE_SYSTEM = "You are a helpful coding assistant. Answer clearly and completely."
MAX_TOKENS = 4096


def load_key() -> str:
    if not KEY_FILE.exists():
        sys.exit(f"missende key: {KEY_FILE}")
    k = KEY_FILE.read_text(encoding="utf-8").strip()
    if not k.startswith("sk-ant-"):
        sys.exit("key zonder sk-ant- prefix")
    return k


def http_json_post(url: str, body: dict, headers: dict, timeout: int = 60) -> dict:
    req = urllib.request.Request(
        url,
        data=json.dumps(body).encode("utf-8"),
        headers={"content-type": "application/json", **headers},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read().decode("utf-8"))


def classify(prompt: str) -> dict:
    body = {
        "task_id": f"real-{int(time.time()*1000)}",
        "scope_md": prompt,
    }
    return http_json_post(f"{SOVACOUNT_URL}/classify", body, headers={})


def anthropic_call(api_key: str, model: str, system: str, user: str) -> dict:
    body = {
        "model": model,
        "max_tokens": MAX_TOKENS,
        "system": system,
        "messages": [{"role": "user", "content": user}],
    }
    headers = {
        "x-api-key": api_key,
        "anthropic-version": "2023-06-01",
    }
    data = http_json_post("https://api.anthropic.com/v1/messages", body, headers, timeout=180)
    usage = data.get("usage", {})
    content = ""
    for b in data.get("content", []):
        if b.get("type") == "text":
            content += b.get("text", "")
    return {
        "input_tokens":  usage.get("input_tokens", 0),
        "output_tokens": usage.get("output_tokens", 0),
        "content":       content,
        "stop_reason":   data.get("stop_reason"),
    }


def usd(model: str, in_tok: int, out_tok: int) -> float:
    pin, pout = TIER_PRICE[model]
    return (in_tok * pin + out_tok * pout) / 1_000_000


def main():
    api_key = load_key()
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    print(f"output → {OUT_DIR}")

    # Health-check sovacount-real
    try:
        with urllib.request.urlopen(f"{SOVACOUNT_URL}/health", timeout=3) as r:
            assert r.status == 200
        print("sovacount-real op 8990: ok")
    except Exception as e:
        sys.exit(f"sovacount-real (8990) niet bereikbaar: {e}")

    prompts = json.loads(BENCH_FILE.read_text())["prompts"]
    print(f"taken: {len(prompts)}")
    print()

    classifications = []
    for i, p in enumerate(prompts):
        print(f"[{i+1:2d}/{len(prompts)}] {p['id']}")
        try:
            c = classify(p["prompt"])
        except Exception as e:
            print(f"   ! classify-error: {e}")
            classifications.append({"task": p["id"], "error": str(e)})
            continue
        tier = c.get("tier")
        conf = c.get("confidence_pct") or c.get("confidence")
        ratio = c.get("rationale", "")[:80]
        model_hint = c.get("model_hint", "")
        classifications.append({
            "task":        p["id"],
            "prompt":      p["prompt"][:120],
            "tier":        tier,
            "confidence":  conf,
            "model_hint":  model_hint,
            "rationale":   ratio,
            "raw":         c,
        })
        print(f"     tier={tier:3s} conf={conf}% model_hint={model_hint}")
        print(f"     rationale: {ratio}")
        time.sleep(0.2)

    # opslag classify-resultaten
    (OUT_DIR / "classifications.json").write_text(
        json.dumps(classifications, indent=2), encoding="utf-8")

    # Real test: voor maximaal 5 taken, doe ECHTE call op gesuggereerde tier + opus referentie
    real_test = []
    test_targets = [c for c in classifications if c.get("tier") in ("hk", "so")][:5]
    print(f"\nReal-call op {len(test_targets)} taken (geselecteerde tier + opus):")

    for c in test_targets:
        prompt = next(p["prompt"] for p in prompts if p["id"] == c["task"])
        tier = c["tier"]
        suggested_model = TIER_MODEL[tier]
        print(f"  {c['task']} → {tier} ({suggested_model})")

        # Call op suggested tier
        try:
            r_sug = anthropic_call(api_key, suggested_model, BASELINE_SYSTEM, prompt)
        except Exception as e:
            print(f"     ! suggested error: {e}")
            continue

        # Call op Opus (referentie)
        try:
            r_opus = anthropic_call(api_key, TIER_MODEL["op"], BASELINE_SYSTEM, prompt)
        except Exception as e:
            print(f"     ! opus error: {e}")
            continue

        cost_sug  = usd(suggested_model,           r_sug["input_tokens"],  r_sug["output_tokens"])
        cost_opus = usd(TIER_MODEL["op"],          r_opus["input_tokens"], r_opus["output_tokens"])
        savings   = cost_opus - cost_sug

        real_test.append({
            "task":          c["task"],
            "suggested_tier": tier,
            "suggested_model": suggested_model,
            "sug_in":        r_sug["input_tokens"],
            "sug_out":       r_sug["output_tokens"],
            "sug_usd":       round(cost_sug, 5),
            "opus_in":       r_opus["input_tokens"],
            "opus_out":      r_opus["output_tokens"],
            "opus_usd":      round(cost_opus, 5),
            "savings_usd":   round(savings, 5),
            "savings_pct":   round(savings / cost_opus * 100, 1) if cost_opus else 0.0,
            "sug_content":   r_sug["content"][:1500],
            "opus_content":  r_opus["content"][:1500],
        })
        print(f"     {tier:2s}: ${cost_sug:.4f}  opus: ${cost_opus:.4f}  bespaard: ${savings:.4f} ({savings/cost_opus*100:.1f}%)")
        time.sleep(0.5)

    (OUT_DIR / "real-test.json").write_text(
        json.dumps(real_test, indent=2), encoding="utf-8")

    # Samenvatting
    n = len(classifications)
    valid = [c for c in classifications if "tier" in c]
    tier_counts = {"hk": 0, "so": 0, "op": 0}
    for c in valid:
        tier_counts[c["tier"]] = tier_counts.get(c["tier"], 0) + 1

    total_sug_cost  = sum(r["sug_usd"]  for r in real_test)
    total_opus_cost = sum(r["opus_usd"] for r in real_test)
    total_savings   = total_opus_cost - total_sug_cost

    summary = {
        "datum_utc":      datetime.utcnow().isoformat(),
        "sovacount_url":  SOVACOUNT_URL,
        "provider":       "anthropic",
        "taken":          n,
        "tier_distribution": tier_counts,
        "real_test_taken": len(real_test),
        "real_test_total_suggested_usd": round(total_sug_cost, 4),
        "real_test_total_opus_usd":      round(total_opus_cost, 4),
        "real_test_savings_usd":         round(total_savings, 4),
        "real_test_savings_pct":         round(total_savings / total_opus_cost * 100, 1) if total_opus_cost else 0.0,
    }
    (OUT_DIR / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")

    # Wis key
    try:
        KEY_FILE.unlink()
    except FileNotFoundError:
        pass

    print(f"\n=== KLAAR ===")
    print(f"tier-verdeling (10 taken): hk={tier_counts['hk']} so={tier_counts['so']} op={tier_counts['op']}")
    if real_test:
        print(f"real-test ({len(real_test)} taken):")
        print(f"  totaal op gesuggereerde tier: ${summary['real_test_total_suggested_usd']}")
        print(f"  totaal op opus referentie:    ${summary['real_test_total_opus_usd']}")
        print(f"  bespaard:                     ${summary['real_test_savings_usd']} ({summary['real_test_savings_pct']}%)")
    print(f"output: {OUT_DIR}")


if __name__ == "__main__":
    main()
