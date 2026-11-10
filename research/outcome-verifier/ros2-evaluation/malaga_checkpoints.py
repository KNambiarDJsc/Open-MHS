"""The experiment in EVALUATION.md, on the Málaga 2009 parking_0L ROS 2 recording.

    python malaga_checkpoints.py --evidence EVIDENCE.jsonl --tum GT_TUM.txt --out OUTDIR

EVIDENCE.jsonl is `ros2_evidence.py convert` output for the bag with mappings/malaga2009_parking_0L.json. GT_TUM.txt is
the dataset's published ground-truth file, used only for labels. Needs the `openmhs` binary (OPENMHS_BIN, or on PATH).
Faults are injected into copies of real records and are named as injected in every output.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import random
import shutil
import statistics
import subprocess
import time
from pathlib import Path

ORIGIN = {"lat": 36.714459075, "lon": -4.4789588283333330}  # /rawlog_comment, [GPS_ORIGIN]
BOX_M = 3.0
WINDOW_MS = 50
CAL_SECONDS = 6.0  # the vehicle stands still for 6.4 s at the start
FAULTS = ["wrong_frame", "reference_gap", "other_clock", "calibration_expired", "other_attempt", "conflicting_reference",
          "reordered",
          # Added after the first run (RESULTS.md, Deviations): a rotation about the origin barely moves positions near
          # it, where most ARRIVED claims are. A plausible odom-to-map offset does.
          "wrong_frame_offset"]
# What each fault does to the physical answer. "metadata": the numbers are untouched, so the physical label still
# holds and a script that ignores metadata answers as on clean data. "values": the numbers a script would read change.
FAULT_KIND = {"wrong_frame": "values", "wrong_frame_offset": "values", "other_clock": "values",
              "conflicting_reference": "values", "reference_gap": "availability", "calibration_expired": "metadata",
              "other_attempt": "metadata", "reordered": "none"}


def binary() -> str:
    return os.environ.get("OPENMHS_BIN") or shutil.which("openmhs") or "openmhs"


def enu(lat: float, lon: float) -> tuple[float, float]:
    """WGS84 latitude/longitude to local east/north metres at ORIGIN. Small-area approximation: over 200 m its error is
    far below a millimetre, well under the GPS errors studied."""
    a, f = 6378137.0, 1 / 298.257223563
    e2 = f * (2 - f)
    lat0 = math.radians(ORIGIN["lat"])
    s = math.sin(lat0)
    m = a * (1 - e2) / (1 - e2 * s * s) ** 1.5
    n = a / math.sqrt(1 - e2 * s * s)
    return math.radians(lon - ORIGIN["lon"]) * n * math.cos(lat0), math.radians(lat - ORIGIN["lat"]) * m


def seconds(rfc: str) -> float:
    import datetime as dt
    whole, _, frac = rfc.rstrip("Z").partition(".")
    base = dt.datetime.strptime(whole, "%Y-%m-%dT%H:%M:%S").replace(tzinfo=dt.timezone.utc).timestamp()
    return base + (float("0." + frac) if frac else 0.0)


def rfc(t: float) -> str:
    import datetime as dt
    ms = int(round(t * 1000))
    s, rem = divmod(ms, 1000)
    return dt.datetime.fromtimestamp(s, dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S") + f".{rem:03d}Z"


def load(evidence: Path) -> dict[str, list[dict]]:
    by_signal: dict[str, list[dict]] = {}
    for line in evidence.read_text(encoding="utf-8").splitlines():
        r = json.loads(line)
        by_signal.setdefault(r["signal"], []).append(r)
    for rs in by_signal.values():
        rs.sort(key=lambda r: seconds(r["time"]["source"]))
    return by_signal


def pairs(rs_a: list[dict], rs_b: list[dict]) -> list[tuple[float, float, float, dict, dict]]:
    """(t, a, b, rec_a, rec_b) where two signals share a source time."""
    b = {r["time"]["source"]: r for r in rs_b}
    return [(seconds(r["time"]["source"]), r["value"]["value"], b[r["time"]["source"]]["value"]["value"], r,
             b[r["time"]["source"]]) for r in rs_a if r["time"]["source"] in b]


def tum(path: Path) -> list[tuple[float, float, float]]:
    out = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.strip() and not line.startswith("#"):
            p = line.split()
            out.append((float(p[0]), float(p[1]), float(p[2])))
    return out


def nearest(samples: list[tuple[float, float, float]], t: float, within: float):
    best = min(samples, key=lambda s: abs(s[0] - t))
    return best if abs(best[0] - t) <= within else None


def alignment(sig: dict[str, list[dict]]) -> dict:
    """Is the ground truth's `map` local east/north at the published origin? The rear RTK antenna sits above base_link
    (tf_static: 0, 0, 0.132), so its fix should match the ground truth's x, y."""
    gt = {r["time"]["source"]: r["value"]["value"] for r in sig["base.x"]}
    gty = {r["time"]["source"]: r["value"]["value"] for r in sig["base.y"]}
    res = []
    for t, lat, lon, rl, _ in pairs(sig["gps_rtk_rear.latitude"], sig["gps_rtk_rear.longitude"]):
        k = rl["time"]["source"]
        if k in gt:
            e, n = enu(lat, lon)
            res.append(math.hypot(e - gt[k], n - gty[k]))
    return {"pairs": len(res), "median_m": round(statistics.median(res), 3), "max_m": round(max(res), 3)}


def claims(sig: dict[str, list[dict]], start: float) -> tuple[list[dict], dict, dict]:
    gps = pairs(sig["gps_normal.latitude"], sig["gps_normal.longitude"])
    gt = {round(seconds(r["time"]["source"]), 2): r["value"]["value"] for r in sig["base.x"]}
    gty = {round(seconds(r["time"]["source"]), 2): r["value"]["value"] for r in sig["base.y"]}
    offs = []
    for t, lat, lon, _, _ in gps:
        if t <= start + CAL_SECONDS and round(t, 2) in gt:
            e, n = enu(lat, lon)
            offs.append((e - gt[round(t, 2)], n - gty[round(t, 2)]))
    dx, dy = statistics.mean(o[0] for o in offs), statistics.mean(o[1] for o in offs)
    calibration = {
        "id": "cal-gps-normal-offset", "signal": "calibration", "value": "gps-offset",
        "source": {"system": "experiment", "device": "gps_normal", "kind": "calibration", "provenance": "device"},
        "calibration": {"name": "gps-offset", "applies_to": "gps_normal", "valid_from": rfc(start),
                        "valid_until": rfc(start + 600)},
    }
    out = []
    for k, (t, lat, lon, rlat, rlon) in enumerate(gps):
        if t <= start + CAL_SECONDS:
            continue
        e, n = enu(lat, lon)
        out.append({"k": k, "t": t, "x": e - dx, "y": n - dy, "record": {
            "id": f"claim-{k}", "signal": "claim.arrived", "value": "ARRIVED",
            "source": {"system": "experiment", "device": "gps_normal", "kind": "controller", "provenance": "device"},
            "time": {"source": rfc(t), "clock_domain": "malaga2009-logger"},
            "refs": {"attempt": f"cp-{k}"}, "frame": "map",
            "relations": [{"type": "wasDerivedFrom", "id": rlat["id"]}, {"type": "wasDerivedFrom", "id": rlon["id"]},
                          {"type": "wasDerivedFrom", "id": calibration["id"]}],
        }})
    stats = {"offset_east_m": round(dx, 3), "offset_north_m": round(dy, 3), "fixes_used": len(offs)}
    return out, calibration, stats


def contract(c: dict) -> dict:
    def box(v: float) -> dict:
        return {"op": "between", "min": {"value": v - BOX_M, "unit": "m"}, "max": {"value": v + BOX_M, "unit": "m"}}
    return {
        "contract": "arrived-at-believed-position", "version": "0.1",
        "binding": {"attempt": f"cp-{c['k']}"},
        "timing": {"anchor": "claim", "default_window_ms": [-WINDOW_MS, WINDOW_MS]},
        "evidence": {
            "claim": {"signal": "claim.arrived", "kind": "controller", "device": "gps_normal",
                      "calibration": ["gps-offset"]},
            "x": {"signal": "base.x", "kind": "independent_sensor", "frame": "map", "bound": False},
            "y": {"signal": "base.y", "kind": "independent_sensor", "frame": "map", "bound": False},
        },
        "conditions": [{"name": "x within 3 m", "evidence": "x", **box(c["x"])},
                       {"name": "y within 3 m", "evidence": "y", **box(c["y"])}],
    }


def near(rs: list[dict], t: float, span: float) -> list[dict]:
    return [r for r in rs if abs(seconds(r["time"]["source"]) - t) <= span]


def evidence_for(c: dict, sig: dict, cal: dict, span: float = 2.0) -> list[dict]:
    return [c["record"], cal, *near(sig["base.x"], c["t"], span), *near(sig["base.y"], c["t"], span)]


def verify(contract_obj: dict, records: list[dict], work: Path, name: str) -> dict:
    cpath, epath = work / f"{name}.contract.json", work / f"{name}.evidence.jsonl"
    cpath.write_text(json.dumps(contract_obj, sort_keys=True), encoding="utf-8")
    epath.write_text("".join(json.dumps(r, sort_keys=True) + "\n" for r in records), encoding="utf-8")
    t0 = time.perf_counter()
    p = subprocess.run([binary(), "--json", "outcome", "verify", "--contract", str(cpath), "--evidence", str(epath)],
                       capture_output=True, text=True)
    elapsed = time.perf_counter() - t0
    if p.returncode not in (0, 3, 4):
        raise SystemExit(f"verifier failed on {name}: {p.stderr}")
    report = json.loads(p.stdout)
    return {"verdict": report["verdict"], "reasons": sorted({r["code"] for r in report["reasons"]}),
            "seconds": elapsed, "report": p.stdout}


def script(c: dict, records: list[dict]) -> str:
    """A reasonable script: the nearest x and y within ±50 ms, box check. It knows nothing of frames, clocks,
    attempts or calibration, because most scripts don't."""
    def pick(signal: str):
        rs = [r for r in records if r["signal"] == signal]
        if not rs:
            return None
        best = min(rs, key=lambda r: abs(seconds(r["time"]["source"]) - c["t"]))
        return best if abs(seconds(best["time"]["source"]) - c["t"]) <= WINDOW_MS / 1000 else None
    x, y = pick("base.x"), pick("base.y")
    if x is None or y is None:
        return "NO_DATA"
    inside = abs(x["value"]["value"] - c["x"]) <= BOX_M and abs(y["value"]["value"] - c["y"]) <= BOX_M
    return "SATISFIED" if inside else "VIOLATED"


def inject(fault: str, c: dict, records: list[dict], cal: dict) -> tuple[dict, list[dict]]:
    """A copy of the claim's evidence with one fault. Every fault is ours, not the data's."""
    rs = json.loads(json.dumps(records))
    con = contract(c)
    if fault == "wrong_frame":
        xs = {r["time"]["source"]: r for r in rs if r["signal"] == "base.x"}
        ys = {r["time"]["source"]: r for r in rs if r["signal"] == "base.y"}
        for k in xs.keys() & ys.keys():
            x, y = xs[k]["value"]["value"], ys[k]["value"]["value"]
            xs[k]["value"]["value"], ys[k]["value"]["value"] = -y, x
            xs[k]["frame"] = ys[k]["frame"] = "odom"
    elif fault == "wrong_frame_offset":
        xs = {r["time"]["source"]: r for r in rs if r["signal"] == "base.x"}
        ys = {r["time"]["source"]: r for r in rs if r["signal"] == "base.y"}
        a = math.radians(30)
        for k in xs.keys() & ys.keys():
            x, y = xs[k]["value"]["value"], ys[k]["value"]["value"]
            xs[k]["value"]["value"] = math.cos(a) * x - math.sin(a) * y + 5.0
            ys[k]["value"]["value"] = math.sin(a) * x + math.cos(a) * y - 8.0
            xs[k]["frame"] = ys[k]["frame"] = "odom"
    elif fault == "reference_gap":
        rs = [r for r in rs if not (r["signal"].startswith("base.") and abs(seconds(r["time"]["source"]) - c["t"]) <= 1.0)]
    elif fault == "other_clock":
        for r in rs:
            if r["signal"].startswith("base."):
                r["time"]["source"] = rfc(seconds(r["time"]["source"]) + 1.5)
                r["time"]["clock_domain"] = "other-logger"
    elif fault == "calibration_expired":
        for r in rs:
            if r["id"] == cal["id"]:
                r["calibration"]["valid_until"] = rfc(c["t"] - 1)
    elif fault == "other_attempt":
        rs[0]["refs"]["attempt"] = "cp-somebody-else"
    elif fault == "conflicting_reference":
        nearest_x = min((r for r in rs if r["signal"] == "base.x"), key=lambda r: abs(seconds(r["time"]["source"]) - c["t"]))
        bad = json.loads(json.dumps(nearest_x))
        bad["id"] = "injected-conflict"
        bad["value"]["value"] += 5.0
        bad["time"]["source"] = rfc(c["t"])
        rs.append(bad)
    elif fault == "reordered":
        random.Random(c["k"]).shuffle(rs)
    return con, rs


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--evidence", type=Path, required=True)
    ap.add_argument("--tum", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    work = args.out / "work"
    work.mkdir(exist_ok=True)
    sig = load(args.evidence)
    labels_src = tum(args.tum)
    start = labels_src[0][0]
    summary: dict = {"alignment": alignment(sig)}
    cs, cal, summary["calibration"] = claims(sig, start)
    rows = []
    for c in cs:
        records = evidence_for(c, sig, cal)
        v = verify(contract(c), records, work, f"cp-{c['k']}")
        ref = nearest(labels_src, c["t"], WINDOW_MS / 1000)
        if ref is None:
            label, err = "NO_REFERENCE", None
        else:
            err = math.hypot(ref[1] - c["x"], ref[2] - c["y"])
            label = "ARRIVED" if abs(ref[1] - c["x"]) <= BOX_M and abs(ref[2] - c["y"]) <= BOX_M else "NOT_ARRIVED"
        rows.append({"k": c["k"], "t": round(c["t"], 2), "label": label,
                     "belief_error_m": None if err is None else round(err, 3), "native": "SATISFIED",
                     "script": script(c, records), "verifier": v["verdict"], "reasons": v["reasons"],
                     "verify_seconds": round(v["seconds"], 3)})
    (args.out / "claims.json").write_text(json.dumps(rows, indent=1) + "\n", encoding="utf-8")

    # Determinism: the same inputs twice give the same bytes.
    sample = cs[:: max(1, len(cs) // 20)]
    summary["determinism_checked"] = len(sample)
    summary["determinism_identical"] = all(
        verify(contract(c), evidence_for(c, sig, cal), work, f"det-a-{c['k']}")["report"]
        == verify(contract(c), evidence_for(c, sig, cal), work, f"det-b-{c['k']}")["report"] for c in sample)
    # The ±2 s selection cannot change a verdict whose window is ±50 ms: checked against the whole file.
    full = [json.loads(line) for line in args.evidence.read_text(encoding="utf-8").splitlines()]
    sub = cs[:: max(1, len(cs) // 8)]
    summary["selection_checked"] = len(sub)
    summary["selection_same_verdict"] = all(
        verify(contract(c), [c["record"], cal, *full], work, f"full-{c['k']}")["verdict"]
        == next(r["verifier"] for r in rows if r["k"] == c["k"]) for c in sub)

    # Injected faults: every ARRIVED claim, and ten NOT_ARRIVED claims spread over the run.
    label_of = {r["k"]: r["label"] for r in rows}
    arrived = [c for c in cs if label_of[c["k"]] == "ARRIVED"]
    not_arrived = [c for c in cs if label_of[c["k"]] == "NOT_ARRIVED"]
    targets = arrived + not_arrived[:: max(1, len(not_arrived) // 10)][:10]
    faults = []
    for c in targets:
        records = evidence_for(c, sig, cal)
        clean = next(r for r in rows if r["k"] == c["k"])
        for f in FAULTS:
            con, rs = inject(f, c, records, cal)
            v = verify(con, rs, work, f"fault-{f}-{c['k']}")
            answer = script(c, rs)
            truth = {"ARRIVED": "SATISFIED", "NOT_ARRIVED": "VIOLATED"}[clean["label"]]
            faults.append({"k": c["k"], "fault": f, "kind": FAULT_KIND[f], "injected": True, "label": clean["label"],
                           "clean_verifier": clean["verifier"], "verifier": v["verdict"], "reasons": v["reasons"],
                           "script": answer, "script_matches_label": answer == truth,
                           "script_false_satisfied": answer == "SATISFIED" and truth != "SATISFIED",
                           "verifier_false_satisfied": v["verdict"] == "SATISFIED" and truth != "SATISFIED"})
    (args.out / "faults.json").write_text(json.dumps(faults, indent=1) + "\n", encoding="utf-8")
    summary["claims"] = len(rows)
    summary["verify_seconds_median"] = round(statistics.median(r["verify_seconds"] for r in rows), 3)
    (args.out / "summary.json").write_text(json.dumps(summary, indent=1) + "\n", encoding="utf-8")
    print(json.dumps(summary, indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
