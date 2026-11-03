"""Run the pre-registered study (Layer 0, model of ROS 2 action semantics).

python -I run_study.py --n 200 --base-seed 20261103 --out results/confirmatory
python -I run_study.py --n 30  --base-seed 1        --out D:/openmhs-tmp/pilot   (debug only)
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
import platform
import random
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import channel  # noqa: E402
import stats  # noqa: E402
from clients import ARMS  # noqa: E402
from trial import FAULTS, SCENARIOS, run_trial, trial_seed  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
PREREG_COMMIT = "9a29ea1d11108fb7f0d4ba1d333c7f01c8776564"
METRICS = ["D", "FC", "FD", "U", "V"]


def git(*args: str) -> str:
    try:
        return subprocess.run(["git", *args], cwd=HERE, capture_output=True, text=True,
                              check=True).stdout.strip()
    except Exception:
        return "unavailable"


def run_record(argv: list[str], base_seed: int, n: int) -> dict:
    return {
        "layer": "0 (model of ROS 2 action semantics; not ROS 2)",
        "preregistration_commit": PREREG_COMMIT,
        "code_commit": git("rev-parse", "HEAD"),
        "dirty": bool(git("status", "--porcelain", "--", ".")),
        "python": sys.version.split()[0],
        "platform": platform.platform(),
        "argv": argv,
        "base_seed": base_seed,
        "n_per_cell": n,
        "channel": {"eps_fp": channel.EPS_FP, "eps_fn": channel.EPS_FN,
                    "dropout": channel.DROPOUT, "prior": channel.PRIOR, "alpha": channel.ALPHA,
                    "readings": channel.N_READINGS},
        "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }


def run_all(n: int, base_seed: int) -> list[dict]:
    trials = []
    for fault in FAULTS:
        for scenario in SCENARIOS:
            for i in range(n):
                seed = trial_seed(base_seed, fault, scenario, i)
                for arm in ARMS:
                    t = run_trial(arm, fault, scenario, seed)
                    t["index"] = i
                    trials.append(t)
    return trials


def cell_summary(rows: list[dict]) -> dict:
    n = len(rows)
    out = {"n": n, "fault_triggered": sum(r["fault_triggered"] for r in rows)}
    for m in METRICS:
        k = sum(r[m] for r in rows)
        lo, hi = stats.clopper_pearson(k, n)
        out[m] = {"k": k, "rate": k / n, "ci95": [lo, hi]}
    out["outcomes"] = {o: sum(r["outcome"] == o for r in rows)
                       for o in ("APPLIED", "NOT_APPLIED", "UNKNOWN")}
    out["checker_disagreements"] = sum(not r["checker_agrees"] for r in rows)
    out["mean_sends"] = sum(r["sends"] for r in rows) / n
    return out


def wrong_resolution(rows: list[dict]) -> dict:
    """Among trials the client resolved (APPLIED/NOT_APPLIED), how many were wrong?"""
    resolved = [r for r in rows if r["outcome"] != "UNKNOWN"]
    wrong = [r for r in resolved if r["FC"] or r["FD"]]
    n = len(resolved)
    if n == 0:
        return {"resolved": 0, "wrong": 0}
    lo, hi = stats.clopper_pearson(len(wrong), n)
    return {"resolved": n, "wrong": len(wrong), "rate": len(wrong) / n, "ci95": [lo, hi]}


def index_trials(trials: list[dict]) -> dict:
    return {(t["arm"], t["fault"], t["scenario"], t["index"]): t for t in trials}


def paired(idx: dict, arm_x: str, arm_y: str, faults: list[str], scenario: str, metric: str,
           n: int) -> dict:
    """Paired comparison on shared seeds. b: x bad & y good. c: x good & y bad."""
    b = c = 0
    diffs = []
    for f in faults:
        for i in range(n):
            x = idx[(arm_x, f, scenario, i)][metric]
            y = idx[(arm_y, f, scenario, i)][metric]
            diffs.append(int(x) - int(y))
            if x and not y:
                b += 1
            if y and not x:
                c += 1
    rng = random.Random(12345)
    boots = []
    m = len(diffs)
    for _ in range(2000):
        boots.append(sum(diffs[rng.randrange(m)] for _ in range(m)) / m)
    boots.sort()
    return {
        "x": arm_x, "y": arm_y, "metric": metric, "faults": faults, "scenario": scenario,
        "pairs": m, "x_only_bad_b": b, "y_only_bad_c": c,
        "mean_difference_x_minus_y": sum(diffs) / m,
        "bootstrap95": [boots[int(0.025 * len(boots))], boots[int(0.975 * len(boots))]],
        "p_exact_mcnemar": stats.mcnemar_exact(b, c),
    }


def confirmatory_contrasts(idx: dict, n: int) -> list[dict]:
    ns = "non-idempotent"
    contrasts = [
        ("C1 identity", paired(idx, "A", "B-M0", ["F1", "F2", "F3", "F4"], ns, "V", n)),
        ("C2 durability", paired(idx, "B-M0", "C-M0", ["F6"], ns, "V", n)),
        ("C3 measurement", paired(idx, "C-M0", "C-M1", ["F3", "F4", "F6"], ns, "U", n)),
        ("C4 false confirmation", paired(idx, "A", "C-M1", ["F5"], ns, "FC", n)),
    ]
    adj = stats.holm([c[1]["p_exact_mcnemar"] for c in contrasts])
    out = []
    for (name, c), p_adj in zip(contrasts, adj):
        c["contrast"] = name
        c["p_holm"] = p_adj
        out.append(c)
    return out


def hypotheses(cells: dict, resolution: dict) -> dict:
    ns = "non-idempotent"

    def cell(arm, fault):
        return cells[f"{arm}|{fault}|{ns}"]

    h1_faults = ["F1", "F2", "F3", "F4", "F6"]
    h1_uppers = {f: cell("A", f)["D"]["ci95"][1] for f in h1_faults}
    h1_falsified = all(u < 0.02 for u in h1_uppers.values())

    h2 = {
        "B_D_by_fault": {f: cell("B-M0", f)["D"]["k"] for f in ("F1", "F2", "F6")},
        "C_D_by_fault": {f: cell("C-M0", f)["D"]["k"] for f in ("F1", "F2", "F6")},
    }
    h2_falsified = cell("B-M0", "F6")["D"]["k"] <= cell("C-M0", "F6")["D"]["k"]

    h3 = {}
    for arm0, arm1 in (("B-M0", "B-M1"), ("C-M0", "C-M1")):
        for f in ("F3", "F4", "F6"):
            u0 = cell(arm0, f)["U"]["rate"]
            u1 = cell(arm1, f)["U"]["rate"]
            h3[f"{arm1}|{f}"] = {"U_M0": u0, "U_M1": u1, "reduction_pp": 100 * (u0 - u1)}
    h3_lower = all(v["reduction_pp"] >= 5.0 for v in h3.values())

    h4 = {}
    for arm in ("A", "B-M0", "C-M0", "B-M1", "C-M1"):
        h4[arm] = {"FC_rate": cell(arm, "F5")["FC"]["rate"], "FC_ci95": cell(arm, "F5")["FC"]["ci95"]}
    predicted_miss = channel.decision_probabilities(False)[channel.APPLIED]
    return {
        "H1m": {"A_D_upper95_by_fault": h1_uppers, "falsified": h1_falsified},
        "H2m": {**h2, "falsified": h2_falsified},
        "H3m": {"unresolved_reduction": h3, "all_reductions_at_least_5pp": h3_lower,
                "wrong_resolution_M1": resolution,
                "alpha": channel.ALPHA},
        "H4m": {"F5_false_confirmation": h4,
                "model_predicted_miss_probability_per_verification": predicted_miss},
        "H5": "not testable in Layer 0 (arm D not built)",
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=200)
    ap.add_argument("--base-seed", type=int, default=20261103)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    os.makedirs(args.out, exist_ok=True)
    record = run_record(sys.argv, args.base_seed, args.n)
    t0 = time.time()
    trials = run_all(args.n, args.base_seed)
    record["wall_seconds"] = round(time.time() - t0, 2)
    record["trials_total"] = len(trials)

    raw_path = os.path.join(args.out, "trials.jsonl.gz")
    # mtime=0 keeps the gzip header constant so the file hash is reproducible across runs
    with gzip.GzipFile(raw_path, "wb", mtime=0) as raw:
        with io.TextIOWrapper(raw, encoding="utf-8", newline="\n") as fh:
            for t in trials:
                fh.write(json.dumps(t, sort_keys=True) + "\n")
    with open(raw_path, "rb") as fh:
        record["trials_sha256"] = hashlib.sha256(fh.read()).hexdigest()

    cells = {}
    for arm in ARMS:
        for f in FAULTS:
            for s in SCENARIOS:
                rows = [t for t in trials if t["arm"] == arm and t["fault"] == f
                        and t["scenario"] == s]
                cells[f"{arm}|{f}|{s}"] = cell_summary(rows)

    m1_rows = [t for t in trials if t["arm"] in ("B-M1", "C-M1") and t["scenario"] == "non-idempotent"
               and t["fault"] != "F0"]
    resolution = {
        "M1_all_faults": wrong_resolution(m1_rows),
        "by_fault": {f: wrong_resolution([t for t in m1_rows if t["fault"] == f])
                     for f in FAULTS if f != "F0"},
    }
    idx = index_trials(trials)
    contrasts = confirmatory_contrasts(idx, args.n)
    summary = {
        "note": "Layer 0: results describe client strategies against a model of ROS 2 action "
                "semantics, not ROS 2 and not hardware.",
        "cells": cells,
        "confirmatory_contrasts": contrasts,
        "hypotheses": hypotheses(cells, resolution),
        "checker_disagreements_total": sum(c["checker_disagreements"] for c in cells.values()),
    }
    with open(os.path.join(args.out, "summary.json"), "w", encoding="utf-8", newline="\n") as fh:
        json.dump(summary, fh, indent=2, sort_keys=True)
        fh.write("\n")
    with open(os.path.join(args.out, "run_record.json"), "w", encoding="utf-8", newline="\n") as fh:
        json.dump(record, fh, indent=2, sort_keys=True)
        fh.write("\n")
    print(f"{len(trials)} trials in {record['wall_seconds']} s -> {args.out}")
    print(f"checker disagreements: {summary['checker_disagreements_total']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
