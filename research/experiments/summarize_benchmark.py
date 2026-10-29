"""Merge the benchmark's output files and print the summary tables used in RELIABILITY_BENCHMARK.md.

    python summarize_benchmark.py main.json extra1.json extra2.json ... > ../results/reliability_benchmark.json
    (the markdown tables go to stderr)
"""

from __future__ import annotations

import json
import sys

ARMS = ["direct-naive", "direct-careful", "mhs-same-key", "mhs-new-key", "mhs-reconcile"]


def main() -> None:
    merged: dict = {"runs": {}}
    for path in sys.argv[1:]:
        d = json.load(open(path, encoding="utf-8"))
        merged.setdefault("_meta", d.get("_meta"))
        merged.setdefault("scenarios", {}).update(d.get("scenarios", {}))
        merged["runs"].update({k: v for k, v in d.get("runs", {}).items() if v})
        for k in ("overhead", "effort"):
            if k in d:
                merged[k] = d[k]
    scen = sorted({k.split("/")[0] for k in merged["runs"]})
    out = sys.stderr

    def cell(rows: list[dict], arm_is_mhs: bool) -> str:
        ok = [r for r in rows if "claim" in r]
        if not ok:
            return "-"
        n = len(ok)
        c = {k: sum(r["claim"] == k for r in ok) for k in ("success", "failure", "unknown")}
        dup = sum(r["duplicate"] for r in ok)
        fs = sum(r["false_success"] for r in ok)
        ff = sum(r["false_failure"] for r in ok)
        return f"S{c['success']} F{c['failure']} U{c['unknown']} d{dup} fs{fs} ff{ff}"

    print("| Scenario | " + " | ".join(ARMS) + " |", file=out)
    print("|---|" + "---|" * len(ARMS), file=out)
    for s in scen:
        row = [cell(merged["runs"].get(f"{s}/{a}", []), a.startswith("mhs")) for a in ARMS]
        print(f"| {s} | " + " | ".join(row) + " |", file=out)

    print("\nTotals per arm (all scenarios):", file=out)
    print("| arm | runs | completed | duplicate executions | false success | false failure | honest unknown | executed an out-of-limit target |", file=out)
    print("|---|---|---|---|---|---|---|---|", file=out)
    for a in ARMS:
        rows = [r for k, v in merged["runs"].items() if k.endswith("/" + a) for r in v if "claim" in r]
        if not rows:
            continue
        f7 = [r for k, v in merged["runs"].items() if k == f"F7/{a}" for r in v if "claim" in r]
        print(f"| {a} | {len(rows)} | {sum(r['completed'] for r in rows)} | {sum(r['duplicate'] for r in rows)} | "
              f"{sum(r['false_success'] for r in rows)} | {sum(r['false_failure'] for r in rows)} | "
              f"{sum(r['honest_unknown'] for r in rows)} | {sum(r['executions'] > 0 for r in f7)} of {len(f7)} |", file=out)

    mhs = [r for k, v in merged["runs"].items() if k.split("/")[1].startswith("mhs") for r in v if "claim" in r]
    if mhs:
        print(f"\nOpenMHS runs: {len(mhs)}; audit chain valid in {sum(bool(r.get('audit_chain_valid')) for r in mhs)}; "
              f"audit complete in {sum(bool(r.get('audit_complete')) for r in mhs)}; "
              f"device reference recorded in {sum(r.get('with_device_reference', 0) > 0 for r in mhs)}", file=out)
    errs = [(k, r) for k, v in merged["runs"].items() for r in v if "harness_error" in r]
    print(f"harness errors: {len(errs)}", file=out)
    print(json.dumps(merged, indent=1, default=str))


if __name__ == "__main__":
    main()
