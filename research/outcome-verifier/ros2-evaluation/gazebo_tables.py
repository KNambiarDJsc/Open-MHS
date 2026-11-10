"""Tables for RESULTS_GAZEBO.md from the analysis directories gazebo_outcomes.py wrote.

    python gazebo_tables.py RESULTS_DIR/bullet1 RESULTS_DIR/bullet2 ...

Prints Markdown. Reads claims.json, summary.json and, where present, faults.json.
"""

from __future__ import annotations

import json
import sys
from collections import Counter
from pathlib import Path


def load(d: Path, name: str):
    p = d / name
    return json.loads(p.read_text(encoding="utf-8")) if p.exists() else None


def main() -> int:
    dirs = [Path(a) for a in sys.argv[1:]]
    print("## Claims per run\n")
    print("| Run | Kind | Label | Claims | Native | Joint-state script right | Ground-truth script right | Verifier |")
    print("|---|---|---|---|---|---|---|---|")
    want = {"AT_TARGET": "SATISFIED", "NOT_AT_TARGET": "VIOLATED"}
    for d in dirs:
        rows = load(d, "claims.json")
        for kind in ("lift", "base"):
            for lab in ("AT_TARGET", "NOT_AT_TARGET", "NO_REFERENCE"):
                rs = [r for r in rows if r["kind"] == kind and r["label"] == lab]
                if not rs:
                    continue
                truth = want.get(lab)
                js = "n/a" if kind == "base" else sum(r["joint_script"] == truth for r in rs)
                gs = sum(r["gt_script"] == truth for r in rs) if truth else Counter(r["gt_script"] for r in rs)
                ver = ", ".join(f"`{k}` {v}" for k, v in sorted(Counter(r["verifier"] for r in rs).items()))
                native = "says there (wrong)" if lab == "NOT_AT_TARGET" else "says there"
                print(f"| {d.name} | {kind} | `{lab}` | {len(rs)} | {native} | {js} | {gs} | {ver} |")
    print("\n## False SATISFIED\n")
    for d in dirs:
        rows = load(d, "claims.json")
        fs = [r["id"] for r in rows if r["verifier"] == "SATISFIED" and r["label"] != "AT_TARGET"]
        disagree = [r["id"] for r in rows if r["label"] in want and r["verifier"] != want[r["label"]]]
        print(f"- {d.name}: verifier false SATISFIED {len(fs)} {fs}; verifier differs from label on {len(disagree)} "
              f"{disagree[:10]}")

    print("\n## Lift goals: carriage error at the reported success (m), by run\n")
    head = [d.name for d in dirs]
    print("| Goal | Target (m) | " + " | ".join(head) + " |")
    print("|---|---|" + "---|" * len(head))
    per = {d.name: {r["id"]: r for r in load(d, "claims.json") if r["kind"] == "lift"} for d in dirs}
    ids = sorted({i for v in per.values() for i in v})
    for i in ids:
        target = next(v[i]["target_m"] for v in per.values() if i in v)
        cells = []
        for d in dirs:
            r = per[d.name].get(i)
            cells.append("no claim" if r is None else f"{r['error_m']:+.4f} {r['verifier']}" if r["error_m"] is not None
                         else f"none {r['verifier']}")
        print(f"| {i} | {target} | " + " | ".join(cells) + " |")

    print("\n## Drive segments: odometry travel vs Gazebo travel (m)\n")
    print("| Run | Task | Command | Odometry | Gazebo | Ratio |")
    print("|---|---|---|---|---|---|")
    for d in dirs:
        for s in load(d, "summary.json")["drive_segments"]:
            g, o = s.get("gazebo_travel_m"), s.get("odom_travel_m")
            ratio = f"{o / g:.1f}x" if g and g > 0.01 else "-"
            print(f"| {d.name} | {s['task']} | {s['vx']} m/s, {s['wz']} rad/s, {s['duration_s']} s | {o} | {g} | {ratio} |")

    for d in dirs:
        faults = load(d, "faults.json")
        if not faults:
            continue
        print(f"\n## Injected faults ({d.name}, {len(faults)} trials)\n")
        print("| Fault | Changes | Trials | Verifier | Reasons | Script right | Script false \"there\" | Verifier false `SATISFIED` |")
        print("|---|---|---|---|---|---|---|---|")
        for f in dict.fromkeys(x["fault"] for x in faults):
            xs = [x for x in faults if x["fault"] == f]
            ver = ", ".join(f"`{k}` {v}" for k, v in sorted(Counter(x["verifier"] for x in xs).items()))
            reasons = ", ".join(sorted({c for x in xs for c in x["reasons"] if c != "condition.false"})) or "-"
            print(f"| {f} | {xs[0]['fault_kind']} | {len(xs)} | {ver} | {reasons} | "
                  f"{sum(x['gt_script_matches_label'] for x in xs)} | {sum(x['gt_script_false_satisfied'] for x in xs)} | "
                  f"{sum(x['verifier_false_satisfied'] for x in xs)} |")

    print("\n## Other measurements\n")
    for d in dirs:
        s = load(d, "summary.json")
        print(f"- {d.name}: claims {s['claims']}, determinism {s['determinism_identical']} ({s['determinism_checked']}), "
              f"selection {s['selection_same_verdict']} ({s['selection_checked']}), status placement uncertainty "
              f"{s['status_uncertainty_ms']} ms, median verify {s['verify_seconds_median']} s")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
