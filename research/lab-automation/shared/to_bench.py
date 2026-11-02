"""One gap-suite result as the proposed OpenMHS Bench format (docs/bench/RESULT_FORMAT.md).

    python to_bench.py <gaps-result.json> GAP-05 --evidence <file> ... > result.json

A proof that the format fits evidence that already exists, not a benchmark. It only handles the
lost-answer scenario so far, and refuses others rather than guess at their fields.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from typing import Any

SCENARIOS = {
    "GAP-05": {
        "invariant": {"id": "no-blind-repeat-after-restart",
                      "statement": "After a lost answer and a hard restart, the command is reported uncertain, is not "
                                   "sent again, and recovery agrees with the device."},
        "fault": {"kind": "lost-answer-then-hard-restart", "at": "after the device applied the command"},
    },
}


def convert(run: dict[str, Any], sid: str, system_version: str, evidence: list[tuple[str, str]]) -> dict[str, Any]:
    if sid not in SCENARIOS:
        raise ValueError(f"{sid} has no conversion yet")
    scenario = run["scenarios"][sid]
    detail = scenario["detail"]
    known = SCENARIOS[sid]
    reconciled = detail.get("reconciled") or {}
    concluded = reconciled.get("state")
    return {
        "format": "openmhs-bench-result/0.1",
        "system": {"name": "OpenMHS", "version": system_version, "commit": run.get("source", {}).get("commit")},
        "scenario": {"id": sid, "version": 1, "title": scenario["title"]},
        "target": {"name": run["ecosystem"], "version": "", "kind": "simulator"},
        "operation": detail.get("operation", {}),
        "fault": known["fault"],
        "invariant": known["invariant"],
        "transitions": (detail.get("lost") or {}).get("transitions", []),
        "dispatch_count": detail.get("dispatch_count"),
        "recovery": {"concluded": concluded,
                     "matches_target": concluded == "RECONCILED_APPLIED" if scenario["outcome"] == "PASS" else None}
        if concluded else None,
        "evidence": [{"path": path, "sha256": digest} for path, digest in evidence],
        "outcome": scenario["outcome"],
        "environment": {k: v for k, v in run.get("environment", {}).items() if k != "packages"},
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("run", type=Path)
    ap.add_argument("scenario")
    ap.add_argument("--system-version", default="unknown")
    ap.add_argument("--evidence", nargs="*", type=Path, default=[])
    a = ap.parse_args()
    run = json.loads(a.run.read_text(encoding="utf-8"))
    files = [*a.evidence, a.run]
    evidence = [(p.name, hashlib.sha256(p.read_bytes()).hexdigest()) for p in files]
    json.dump(convert(run, a.scenario, a.system_version, evidence), sys.stdout, indent=1)
    print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
