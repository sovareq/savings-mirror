#!/usr/bin/env python3
"""
Classifier-test op Bjorn's eigen prompts: sample 50 random user-messages uit
~/.claude/projects/*.jsonl (laatste 3 weken), stuur naar sovacount-real op 8990
(real Anthropic classifier), tel tier-verdeling, vergelijk met Julius-mix.

Doel: weten of de 20/70/10-aanname uit het rapport klopt voor BJORN's werk.

Cost: ~50 classifier-calls × ~300 tokens out × Sonnet $15/M = ~$0.10
"""

import json
import pathlib
import random
import sys
import time
import urllib.error
import urllib.request
from collections import defaultdict
from datetime import datetime, timezone, timedelta

OUT_DIR = pathlib.Path.home() / "Desktop" / f"bjorn-prompts-test-{datetime.utcnow().strftime('%Y%m%d-%H%M%S')}"
SOVACOUNT_URL = "http://127.0.0.1:8990"
SAMPLE_SIZE = 50
SEED = 42
MAX_PROMPT_CHARS = 8000   # cap zodat we niet hele bestanden meesturen
MIN_PROMPT_CHARS = 50     # filter triviale "ja" / "ok" prompts uit

CUTOFF = (datetime.now(timezone.utc) - timedelta(weeks=3)).isoformat()


def extract_user_text(msg_field):
    """Krijgt 'content' veld van een user message; kan string of list of dicts zijn."""
    if isinstance(msg_field, str):
        return msg_field
    if isinstance(msg_field, list):
        parts = []
        for blk in msg_field:
            if isinstance(blk, dict):
                # Sla tool_result blocks over — die zijn geen Bjorn-prompts
                if blk.get("type") == "tool_result":
                    continue
                if blk.get("type") == "text":
                    parts.append(blk.get("text", ""))
                elif "text" in blk:
                    parts.append(blk["text"])
        return "\n".join(parts)
    return ""


def collect_prompts():
    """Loop alle JSONL bestanden door, verzamel user-prompts uit laatste 3 weken."""
    root = pathlib.Path.home() / ".claude" / "projects"
    prompts = []
    for f in root.rglob("*.jsonl"):
        try:
            for line in f.open(encoding="utf-8"):
                try:
                    v = json.loads(line)
                except Exception:
                    continue
                if v.get("type") != "user":
                    continue
                ts = v.get("timestamp")
                if not ts or ts < CUTOFF:
                    continue
                msg = v.get("message", {}) or {}
                content = msg.get("content")
                text = extract_user_text(content)
                # Filter system-reminders + tool-results + hook-output
                if "system-reminder" in text.lower():
                    continue
                if "<command-message" in text:
                    continue
                text = text.strip()
                if not text or len(text) < MIN_PROMPT_CHARS:
                    continue
                prompts.append({
                    "ts":     ts,
                    "file":   str(f.name),
                    "text":   text[:MAX_PROMPT_CHARS],
                })
        except Exception:
            continue
    return prompts


def classify(prompt_text: str) -> dict:
    body = {
        "task_id": f"bjorn-{int(time.time()*1000)}",
        "scope_md": prompt_text,
    }
    req = urllib.request.Request(
        f"{SOVACOUNT_URL}/classify",
        data=json.dumps(body).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read().decode("utf-8"))


def main():
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    print(f"output → {OUT_DIR}")

    print("verzamel prompts uit transcripts (3w)...")
    all_prompts = collect_prompts()
    print(f"gevonden: {len(all_prompts)} user-prompts met >={MIN_PROMPT_CHARS} chars")

    random.seed(SEED)
    sample = random.sample(all_prompts, min(SAMPLE_SIZE, len(all_prompts)))
    print(f"sample: {len(sample)} (seed={SEED})\n")

    results = []
    tier_count = defaultdict(int)
    for i, p in enumerate(sample):
        try:
            c = classify(p["text"])
        except urllib.error.HTTPError as e:
            print(f"[{i+1}/{len(sample)}] HTTP {e.code}: {e.read().decode()[:120]}")
            continue
        except Exception as e:
            print(f"[{i+1}/{len(sample)}] FAIL: {e}")
            continue
        tier = c.get("tier")
        conf = c.get("confidence_pct") or c.get("confidence")
        ratio = (c.get("rationale", "") or "")[:80]
        snippet = p["text"][:60].replace("\n", " ")
        tier_count[tier] += 1
        results.append({
            "ts":         p["ts"],
            "snippet":    snippet,
            "chars":      len(p["text"]),
            "tier":       tier,
            "confidence": conf,
            "rationale":  ratio,
        })
        print(f"[{i+1:2d}/{len(sample)}] {tier:3s} conf={conf}%  {snippet}...")
        time.sleep(0.2)

    n = len(results)
    if not n:
        sys.exit("geen results")

    # Distributie
    hk = tier_count.get("hk", 0)
    so = tier_count.get("so", 0)
    op = tier_count.get("op", 0)

    # Bereken pay-per-token impact
    # Bjorn-baseline (alles Opus volgens werkpatroon-data):
    BJORN_OUT_TOKENS_3W = 53_597_047
    BJORN_IN_TOKENS_3W = 4_527_370
    BJORN_BASELINE_USD = 3818.22

    avg_out_per_call = 53_597_047 / 44302
    avg_in_per_call  =  4_527_370 / 44302

    pct_hk = hk / n
    pct_so = so / n
    pct_op = op / n

    # Sample-mix toegepast op 44302 calls
    PRICE = {"hk": (1.0, 5.0), "so": (3.0, 15.0), "op": (15.0, 75.0)}
    total_calls = 44302
    cost_after = 0.0
    breakdown = {}
    for tier, pct in [("hk", pct_hk), ("so", pct_so), ("op", pct_op)]:
        n_calls = int(total_calls * pct)
        in_tok  = n_calls * avg_in_per_call
        out_tok = n_calls * avg_out_per_call
        p_in, p_out = PRICE[tier]
        c = (in_tok * p_in + out_tok * p_out) / 1_000_000
        cost_after += c
        breakdown[tier] = {"calls": n_calls, "in_tok": int(in_tok), "out_tok": int(out_tok), "usd": round(c, 2)}

    savings = BJORN_BASELINE_USD - cost_after
    pct_savings = savings / BJORN_BASELINE_USD * 100

    # Plus combo met caveman 58.6% output-reductie
    combo_after = 0.0
    for tier, info in breakdown.items():
        n_calls = info["calls"]
        in_tok = n_calls * avg_in_per_call
        out_tok = n_calls * avg_out_per_call * (1 - 0.586)
        p_in, p_out = PRICE[tier]
        combo_after += (in_tok * p_in + out_tok * p_out) / 1_000_000
    combo_savings = BJORN_BASELINE_USD - combo_after
    combo_pct = combo_savings / BJORN_BASELINE_USD * 100

    summary = {
        "datum_utc":            datetime.utcnow().isoformat(),
        "sample_size":          n,
        "tier_count":           {"hk": hk, "so": so, "op": op},
        "tier_pct":             {"hk": round(pct_hk*100,1), "so": round(pct_so*100,1), "op": round(pct_op*100,1)},
        "vergeleken_met_julius_mix": {"julius_hk_pct": 20, "julius_so_pct": 70, "julius_op_pct": 10},
        "bjorn_baseline_usd_3w": BJORN_BASELINE_USD,
        "bjorn_mix_breakdown":   breakdown,
        "bjorn_mix_total_usd":   round(cost_after, 2),
        "bjorn_mix_savings_usd": round(savings, 2),
        "bjorn_mix_savings_pct": round(pct_savings, 1),
        "combo_caveman_total_usd":    round(combo_after, 2),
        "combo_caveman_savings_usd":  round(combo_savings, 2),
        "combo_caveman_savings_pct":  round(combo_pct, 1),
    }
    (OUT_DIR / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    (OUT_DIR / "results.json").write_text(json.dumps(results, indent=2), encoding="utf-8")

    # Wis key
    try:
        (pathlib.Path.home() / ".local" / "share" / "savings-mirror" / "anthropic-key.tmp").unlink()
    except FileNotFoundError:
        pass

    print(f"\n=== KLAAR ===")
    print(f"sample {n} prompts uit Bjorn-transcripts, classifier-test:")
    print(f"  Haiku:  {hk:3d} ({pct_hk*100:.1f}%)  [Julius-mix: 20%]")
    print(f"  Sonnet: {so:3d} ({pct_so*100:.1f}%)  [Julius-mix: 70%]")
    print(f"  Opus:   {op:3d} ({pct_op*100:.1f}%)  [Julius-mix: 10%]")
    print()
    print(f"Toegepast op Bjorn 3w baseline (${BJORN_BASELINE_USD}):")
    print(f"  SovaCount alleen:           ${summary['bjorn_mix_total_usd']}   bespaard ${summary['bjorn_mix_savings_usd']} ({summary['bjorn_mix_savings_pct']}%)")
    print(f"  Combo (+caveman 58.6%):     ${summary['combo_caveman_total_usd']}   bespaard ${summary['combo_caveman_savings_usd']} ({summary['combo_caveman_savings_pct']}%)")
    print()
    print(f"output: {OUT_DIR}")


if __name__ == "__main__":
    main()
