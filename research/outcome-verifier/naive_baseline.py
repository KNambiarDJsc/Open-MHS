"""The simple script the strategy's 60-day gate compares against: take the newest value of each signal and
check the contract's conditions on it. No identity, time window, clock, quality or calibration rules, and no
UNKNOWN. About fifteen lines of logic, which is the point: it is what someone would write in an afternoon.

    python research/outcome-verifier/naive_baseline.py

It runs over the corpus and compares with the expected verdicts. A script written to the verifier's rules
would match the verifier; this measures what those rules add, not whether the verifier is clever.
"""

from __future__ import annotations

import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCALE = {"V": 1.0, "mV": 1e-3, "m": 1.0, "mm": 1e-3}


def number(v):
    if isinstance(v, dict):
        return v["value"] * SCALE.get(v["unit"], float("nan"))
    return v


def naive(contract: dict, records: list[dict]) -> str:
    newest = {}
    for r in sorted(records, key=lambda r: (r.get("time") or {}).get("source", "")):
        newest[r["signal"]] = r["value"]
    for c in contract["conditions"]:
        signal = contract["evidence"][c["evidence"]]["signal"]
        if signal not in newest:
            return "VIOLATED"  # a script with no third answer has to pick one
        v = number(newest[signal])
        ok = {"between": lambda: number(c["min"]) <= v <= number(c["max"]),
              "eq": lambda: v == number(c["value"]),
              "ne": lambda: v != number(c["value"])}[c["op"]]()
        if not ok:
            return "VIOLATED"
    return "SATISFIED"


def main() -> int:
    rows = []
    for case in sorted((HERE / "corpus" / "cases").glob("*/*")):
        contract = json.loads((case / "contract.json").read_text(encoding="utf-8"))
        records = [json.loads(line) for line in (case / "evidence.jsonl").read_text(encoding="utf-8").splitlines()
                   if line.strip()]
        expected = json.loads((case / "expected.json").read_text(encoding="utf-8"))["verdict"]
        rows.append((f"{case.parent.name}/{case.name}", expected, naive(contract, records)))
    agree = sum(1 for _, e, n in rows if e == n)
    false_success = [name for name, e, n in rows if n == "SATISFIED" and e != "SATISFIED"]
    false_failure = [name for name, e, n in rows if n == "VIOLATED" and e != "VIOLATED"]
    print(f"cases {len(rows)}; the script agrees with the expected verdict in {agree}")
    print(f"false SATISFIED ({len(false_success)}): " + ", ".join(false_success))
    print(f"VIOLATED where the evidence could not establish it ({len(false_failure)}): " + ", ".join(false_failure))
    for name, e, n in rows:
        print(f"  {name:55} expected {e:10} script {n}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
