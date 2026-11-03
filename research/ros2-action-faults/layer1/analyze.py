"""Evaluate Layer 1a observations against the premises in LAYER1_PLAN.md.

python -I analyze.py results/layer1a  ->  prints a table and writes summary.json next to the input.
Validity rule (fixed in Amendment 1): a trial is valid only if the first send was 'accepted'.
A premise is 'held' only if every valid trial agrees and at least 15 of 20 trials were valid.
"""

from __future__ import annotations

import collections
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
import stats  # noqa: E402

# premise -> predicate over one valid observation (True = agrees with the model)
PREMISES = {
    "dup_held": ("P1 duplicate goal id rejected while held; one effect",
                 lambda o: o["second_same_uuid"] == "rejected" and o["effects"] == 1),
    "expiry_10s": ("P2 after expiry: UNKNOWN; same id accepted again; two effects",
                   lambda o: o["late_status_after_expiry"] == "UNKNOWN"
                   and o["resend_same_uuid_fresh_client"] == "accepted" and o["effects"] == 2),
    "kill_after_effect": ("P3 after kill+restart: UNKNOWN; same id accepted again; two effects",
                          lambda o: o["status_old_goal_after_restart"] == "UNKNOWN"
                          and o["resend_same_uuid"] == "accepted" and o["effects"] == 2),
    "kill_before_effect": ("P4 kill before the effect: UNKNOWN; resend accepted; one effect",
                           lambda o: o["status_old_goal_after_restart"] == "UNKNOWN"
                           and o["resend_same_uuid"] == "accepted" and o["effects"] == 1),
    "kill_after_effect_same_client": (
        "P5 same-client resend: client exception recorded; server executes it (two effects)",
        lambda o: any("Two goals were accepted with the same ID" in e for e in o["client_exceptions"])
        and o["effects"] == 2),
}


def main(path: str) -> int:
    rows = [json.loads(line) for line in open(os.path.join(path, "observations.jsonl"), encoding="utf-8")]
    out = {"n_observations": len(rows), "harness_errors": sum("harness_error" in r for r in rows),
           "premises": {}}
    by = collections.defaultdict(list)
    for r in rows:
        by[r["scenario"]].append(r)
    for name, (label, agrees) in PREMISES.items():
        rs = [r for r in by.get(name, []) if "harness_error" not in r]
        valid = [r for r in rs if r.get("first") == "accepted"]
        invalid = [r for r in rs if r.get("first") != "accepted"]
        agree = [r for r in valid if agrees(r)]
        disagree = [r for r in valid if not agrees(r)]
        if len(valid) >= 15 and not disagree:
            verdict = "held"
        elif disagree:
            verdict = "contradicted in %d valid trial(s)" % len(disagree)
        else:
            verdict = "inconclusive"
        lo = stats.clopper_pearson(len(agree), len(valid))[0] if valid else None
        out["premises"][name] = {
            "premise": label, "trials": len(rs), "valid": len(valid), "invalid_first_send": len(invalid),
            "agree": len(agree), "disagree": len(disagree), "verdict": verdict,
            "agreement_lower95": lo,
            "disagreeing_observations": [{k: v for k, v in r.items() if k != "server_events"}
                                         for r in disagree[:5]],
            "invalid_timelines": [{"order": r["order"], "first": r["first"],
                                   "server_events": r.get("server_events")} for r in invalid[:10]],
        }
    with open(os.path.join(path, "summary.json"), "w", encoding="utf-8", newline="\n") as fh:
        json.dump(out, fh, indent=2, sort_keys=True)
        fh.write("\n")
    for name, p in out["premises"].items():
        print(f"{name:32} valid {p['valid']:2}/{p['trials']:2} agree {p['agree']:2} "
              f"disagree {p['disagree']} -> {p['verdict']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1]))
