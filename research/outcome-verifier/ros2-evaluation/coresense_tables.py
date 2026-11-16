"""Markdown tables for RESULTS_CORESENSE.md from coresense_outcomes.py's claims.json and faults.json.

    python coresense_tables.py RESULTS_DIR
"""

from __future__ import annotations

import collections
import json
import statistics
import sys
from pathlib import Path


def main() -> int:
    d = Path(sys.argv[1])
    rows = json.loads((d / "claims.json").read_text(encoding="utf-8"))
    want = {"AT_TARGET": "SATISFIED", "NOT_AT_TARGET": "VIOLATED"}
    print("| Behavior | Claims | At target (label) | Script right | Verifier `SATISFIED` | `VIOLATED` | `UNKNOWN` | Error at claim (m): median / max |")
    print("|---|---|---|---|---|---|---|---|")
    for beh in ("GoTo", "Takeoff", "Land"):
        rs = [r for r in rows if r["behavior"] == beh]
        v = collections.Counter(r["verifier"] for r in rs)
        errs = [abs(r["error_m"]) for r in rs if r["error_m"] is not None]
        right = sum(r["script"] == want.get(r["label"]) for r in rs)
        print(f"| {beh} | {len(rs)} | {sum(r['label'] == 'AT_TARGET' for r in rs)} | {right} | {v['SATISFIED']} | {v['VIOLATED']} | "
              f"{v['UNKNOWN']} | {statistics.median(errs):.3f} / {max(errs):.3f} |")
    print("\n| Flight | Claims | `SATISFIED` | `VIOLATED` | `UNKNOWN` |")
    print("|---|---|---|---|---|")
    for bag in ("no_failure", "panel_failure", "double_failure"):
        rs = [r for r in rows if r["bag"] == bag]
        v = collections.Counter(r["verifier"] for r in rs)
        print(f"| {bag} | {len(rs)} | {v['SATISFIED']} | {v['VIOLATED']} | {v['UNKNOWN']} |")
    g = [r for r in rows if r["behavior"] == "GoTo"]
    print("\n| GoTo tolerance (xy) | Within tolerance at the claim |")
    print("|---|---|")
    for key, tol in (("label_at_0.10", "0.10 m"), ("label", "0.20 m (planned)"), ("label_at_0.30", "0.30 m")):
        print(f"| {tol} | {sum(r[key] == 'AT_TARGET' for r in g)} of {len(g)} |")
    false = sum(r["verifier"] == "SATISFIED" and r["label"] != "AT_TARGET" for r in rows)
    mism = [r for r in rows if r["label"] in want and r["verifier"] != want[r["label"]]]
    print(f"\nVerifier false `SATISFIED`: {false}. Verifier differs from the label on {len(mism)} claims "
          f"({collections.Counter(tuple(r['reasons']) for r in mism)}). "
          f"Script differs from the label on {sum(r['script'] != want.get(r['label']) for r in rows if r['label'] in want)}.")
    fp = d / "faults.json"
    if fp.exists():
        f = json.loads(fp.read_text(encoding="utf-8"))
        print("\n| Injected fault | Trials | Verifier | Reasons | Script right | Script false `SATISFIED` | Verifier false `SATISFIED` |")
        print("|---|---|---|---|---|---|---|")
        for name in dict.fromkeys(x["fault"] for x in f):
            xs = [x for x in f if x["fault"] == name]
            ver = ", ".join(f"`{k}` {v}" for k, v in sorted(collections.Counter(x["verifier"] for x in xs).items()))
            reasons = ", ".join(sorted({c for x in xs for c in x["reasons"] if c != "condition.false"}))
            print(f"| {name} | {len(xs)} | {ver} | {reasons} | {sum(x['script_matches_label'] for x in xs)} | "
                  f"{sum(x['script_false_satisfied'] for x in xs)} | {sum(x['verifier_false_satisfied'] for x in xs)} |")
    s = json.loads((d / "summary.json").read_text(encoding="utf-8"))
    print(f"\nDeterminism: {s['determinism_identical']} ({s['determinism_checked']} claims verified twice). "
          f"Median verification {s['verify_seconds_median']} s per claim.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
