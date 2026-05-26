#!/usr/bin/env python3
"""
Kwaliteits-check: voor elke (gesuggereerde-tier, opus) paar uit real-test.json,
laat Sonnet-4.5 oordelen of de gesuggereerde response substantieel even goed is.

Rubric (strikt):
  - technical_correctness (0-3)
  - completeness          (0-3)
  - actionable_usability  (0-3)
Totaal max 9. >=7 = acceptabel (downgrade veilig).

Voor de classifier-instructie: we geven beide responses anoniem (A/B random)
en vragen om scores. Vermeldt expliciet dat dit zelf-evaluatie is door Sonnet.

Cost: 5 taken × 1 judge-call × ~600 tokens out = ~$0,05
"""

import json
import pathlib
import random
import sys
import time
import urllib.request
import urllib.error
from datetime import datetime

KEY_FILE  = pathlib.Path.home() / ".local" / "share" / "savings-mirror" / "anthropic-key.tmp"
SOURCE    = sorted(pathlib.Path.home().glob("Desktop/sovacount-real-*/real-test.json"))[-1]
OUT_DIR   = SOURCE.parent / "kwaliteits-check"
JUDGE_MODEL = "claude-sonnet-4-5-20250929"

JUDGE_SYSTEM = """You are an impartial code-review judge. You receive a USER prompt and two candidate answers labelled A and B. Score each on:

- technical_correctness: 0-3 (0=wrong/misleading, 3=fully correct)
- completeness:          0-3 (0=missing core, 3=fully covers prompt)
- actionable_usability:  0-3 (0=unusable, 3=ready to apply)

Output strict JSON: {"a": {"correct":N,"complete":N,"usable":N,"total":N}, "b": {...}, "winner": "a|b|tie", "verdict": "short reason"}.
No prose outside JSON. Be strict, ignore writing style — only score substance."""


def call_judge(api_key: str, user_prompt: str, ans_a: str, ans_b: str) -> dict:
    body = {
        "model": JUDGE_MODEL,
        "max_tokens": 700,
        "system": JUDGE_SYSTEM,
        "messages": [{
            "role": "user",
            "content": (
                f"USER prompt:\n```\n{user_prompt}\n```\n\n"
                f"Candidate A:\n```\n{ans_a}\n```\n\n"
                f"Candidate B:\n```\n{ans_b}\n```\n\n"
                "Score and pick winner."
            ),
        }],
    }
    req = urllib.request.Request(
        "https://api.anthropic.com/v1/messages",
        data=json.dumps(body).encode("utf-8"),
        headers={
            "x-api-key": api_key,
            "anthropic-version": "2023-06-01",
            "content-type": "application/json",
        },
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=120) as r:
        data = json.loads(r.read().decode("utf-8"))
    txt = ""
    for blk in data.get("content", []):
        if blk.get("type") == "text":
            txt += blk["text"]
    # strip ```json fences indien aanwezig
    txt = txt.strip()
    if txt.startswith("```"):
        txt = txt.strip("`")
        if txt.lower().startswith("json"):
            txt = txt[4:]
    return json.loads(txt.strip())


def main():
    if not KEY_FILE.exists():
        sys.exit("missende key")
    api_key = KEY_FILE.read_text(encoding="utf-8").strip()
    if not SOURCE.exists():
        sys.exit(f"missende source: {SOURCE}")

    tests = json.loads(SOURCE.read_text(encoding="utf-8"))
    print(f"source: {SOURCE}")
    print(f"taken: {len(tests)}")
    OUT_DIR.mkdir(exist_ok=True)

    judge_results = []
    random.seed(42)

    for i, t in enumerate(tests):
        # Origineel prompt herstellen uit Julius' benchmark
        prompts_path = pathlib.Path("/tmp/caveman-bench/prompts.json")
        prompts = json.loads(prompts_path.read_text())["prompts"]
        original_prompt = next(p["prompt"] for p in prompts if p["id"] == t["task"])

        sug_content  = t["sug_content"]
        opus_content = t["opus_content"]

        # Anonimiseer: A/B random
        if random.random() < 0.5:
            a, b = sug_content, opus_content
            mapping = {"a": "suggested", "b": "opus"}
        else:
            a, b = opus_content, sug_content
            mapping = {"a": "opus", "b": "suggested"}

        print(f"\n[{i+1}/{len(tests)}] {t['task']} (sug={t['suggested_tier']})")
        try:
            j = call_judge(api_key, original_prompt, a, b)
        except urllib.error.HTTPError as e:
            print(f"   ! judge-error: HTTP {e.code} {e.read().decode()[:120]}")
            continue
        except Exception as e:
            print(f"   ! judge-error: {e}")
            continue

        # Decode mapping → suggested-score vs opus-score
        suggested_label = "a" if mapping["a"] == "suggested" else "b"
        opus_label      = "a" if mapping["a"] == "opus"      else "b"
        sug_score  = j[suggested_label]
        opus_score = j[opus_label]
        winner_label = j.get("winner", "")
        winner_role = mapping.get(winner_label, "tie")
        verdict = j.get("verdict", "")[:200]

        row = {
            "task":             t["task"],
            "suggested_tier":   t["suggested_tier"],
            "sug_total":        sug_score.get("total"),
            "sug_correct":      sug_score.get("correct"),
            "sug_complete":     sug_score.get("complete"),
            "sug_usable":       sug_score.get("usable"),
            "opus_total":       opus_score.get("total"),
            "opus_correct":     opus_score.get("correct"),
            "opus_complete":    opus_score.get("complete"),
            "opus_usable":      opus_score.get("usable"),
            "winner":           winner_role,
            "verdict":          verdict,
            "savings_pct":      t["savings_pct"],
        }
        judge_results.append(row)
        print(f"   suggested ({t['suggested_tier']}): {row['sug_total']}/9  opus: {row['opus_total']}/9  winner: {winner_role}")
        print(f"   verdict: {verdict}")
        time.sleep(0.4)

    # Samenvatting
    n = len(judge_results)
    if not n:
        sys.exit("geen resultaten")

    acceptable = sum(1 for r in judge_results if r["sug_total"] is not None and r["sug_total"] >= 7)
    sug_wins   = sum(1 for r in judge_results if r["winner"] == "suggested")
    opus_wins  = sum(1 for r in judge_results if r["winner"] == "opus")
    ties       = sum(1 for r in judge_results if r["winner"] == "tie")
    avg_sug    = sum(r["sug_total"]  or 0 for r in judge_results) / n
    avg_opus   = sum(r["opus_total"] or 0 for r in judge_results) / n

    summary = {
        "datum_utc":       datetime.utcnow().isoformat(),
        "judge_model":     JUDGE_MODEL,
        "taken":           n,
        "acceptable_count":   acceptable,
        "acceptable_pct":     round(acceptable / n * 100, 1),
        "winners":            {"suggested": sug_wins, "opus": opus_wins, "tie": ties},
        "avg_sug_total":      round(avg_sug, 2),
        "avg_opus_total":     round(avg_opus, 2),
        "results":            judge_results,
    }
    (OUT_DIR / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")

    # Markdown
    md = [
        f"# Kwaliteits-check — gesuggereerde tier vs Opus referentie",
        f"",
        f"**Datum**: {summary['datum_utc']}",
        f"**Judge**: `{JUDGE_MODEL}`",
        f"**Taken**: {n}",
        f"",
        f"## Aggregaat",
        f"",
        f"| Metric | Waarde |",
        f"|---|---:|",
        f"| Gemiddelde score suggested | {avg_sug:.2f}/9 |",
        f"| Gemiddelde score opus | {avg_opus:.2f}/9 |",
        f"| Suggested ≥7 (acceptabel) | {acceptable}/{n} ({summary['acceptable_pct']}%) |",
        f"| Winnaar = suggested | {sug_wins} |",
        f"| Winnaar = opus | {opus_wins} |",
        f"| Gelijkspel | {ties} |",
        f"",
        f"## Per taak",
        f"",
        f"| taak | tier | sug-score | opus-score | winner | bespaard% |",
        f"|---|---|---:|---:|---|---:|",
    ]
    for r in judge_results:
        md.append(
            f"| {r['task']} | {r['suggested_tier']} | {r['sug_total']}/9 | "
            f"{r['opus_total']}/9 | {r['winner']} | {r['savings_pct']}% |"
        )
    md.append("")
    md.append("## Verdicten")
    md.append("")
    for r in judge_results:
        md.append(f"### {r['task']} (tier={r['suggested_tier']}, winner={r['winner']})")
        md.append(f"- suggested: correct {r['sug_correct']}/3, complete {r['sug_complete']}/3, usable {r['sug_usable']}/3 = **{r['sug_total']}/9**")
        md.append(f"- opus:      correct {r['opus_correct']}/3, complete {r['opus_complete']}/3, usable {r['opus_usable']}/3 = **{r['opus_total']}/9**")
        md.append(f"- *verdict*: {r['verdict']}")
        md.append("")

    md.append("## Methodologie")
    md.append("")
    md.append("- Beide responses zijn anoniem (A/B random) aan judge gegeven")
    md.append("- Judge model is Sonnet 4.5 — niet Opus zelf (om judge-bias te vermijden)")
    md.append("- Rubric: technical_correctness + completeness + actionable_usability, elk 0-3")
    md.append(f"- Acceptabel-drempel: total >= 7/9. Onder die drempel = downgrade NIET veilig voor die taak.")
    md.append("- LLM-as-judge heeft inherent variance. Voor productie-besluit: combineer met menselijke review op de gevallen waar winner=opus.")
    md.append("")

    (OUT_DIR / "REPORT.md").write_text("\n".join(md), encoding="utf-8")

    # Wis key
    try:
        KEY_FILE.unlink()
    except FileNotFoundError:
        pass

    print(f"\n=== KLAAR ===")
    print(f"acceptabel (sug ≥7/9): {acceptable}/{n} ({summary['acceptable_pct']}%)")
    print(f"avg suggested: {avg_sug:.2f}/9  avg opus: {avg_opus:.2f}/9")
    print(f"winners: suggested={sug_wins}, opus={opus_wins}, tie={ties}")
    print(f"output: {OUT_DIR}")


if __name__ == "__main__":
    main()
