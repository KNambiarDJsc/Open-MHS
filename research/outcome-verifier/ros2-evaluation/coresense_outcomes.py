"""Real drone flights: action status against motion capture. The plan is EVALUATION_CORESENSE.md (committed first).

    python coresense_outcomes.py --bags BAGS_DIR --out OUT_DIR [--faults]

BAGS_DIR holds the CoreSense `rosbag2_real_paths_2_OK_*` directories. Needs the `openmhs` binary (OPENMHS_BIN or PATH).
Reads the bags read-only. REAL HARDWARE, replayed offline; the failures in these flights were induced by the experimenters.

The reference (`self_localization/pose`) comes from the motion-capture system that also feeds the controller: it is
independent of the action's claim, not of the sensor the behavior used.
"""

from __future__ import annotations

import argparse
import bisect
import datetime as dt
import json
import math
import random
import re
import statistics
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "python"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from malaga_checkpoints import verify  # noqa: E402
from openmhs.importers import cdr  # noqa: E402
from openmhs.importers import coresense_aerostack2 as imp  # noqa: E402

DOMAIN = "coresense-recorder"
XY_TOL_M = 0.20
TAKEOFF_Z_M, TAKEOFF_TOL_M = 1.5, 0.25
LAND_Z_MAX_M = 0.15
WINDOW_MS = 50
FAULTS = ["wrong_frame", "other_clock", "reference_gap", "other_attempt"]
FAULT_KIND = {"wrong_frame": "values (xy) or metadata (z)", "other_clock": "values", "reference_gap": "availability",
              "other_attempt": "metadata"}
SRC = {"system": "ros2-bag", "device": "mocap-localization", "kind": "independent_sensor", "provenance": "device"}


def rfc(ns: int) -> str:
    return imp.rfc(ns)


def parse_ms(text: str) -> int:
    whole, _, frac = text.rstrip("Z").partition(".")
    base = dt.datetime.strptime(whole, "%Y-%m-%dT%H:%M:%S").replace(tzinfo=dt.timezone.utc).timestamp()
    return int(round((base + (float("0." + frac) if frac else 0.0)) * 1000))


# ------------------------------------------------------------------------------------------------ reading the bag

def load_poses(bag: imp.Bag, drone: str) -> list[tuple[int, float, float, float]]:
    out = []
    for ts, data in bag.messages(f"/{drone}/self_localization/pose"):
        p = cdr.decode_pose_stamped(bytes(data))
        out.append((ts, p.position[0], p.position[1], p.position[2]))
    out.sort()
    return out


def waypoints(bag: imp.Bag) -> dict[str, dict[str, float]]:
    wp: dict[str, dict[str, float]] = {}
    for drone in bag.drones():
        for _ts, line in imp.facts(bag, drone):
            m = re.match(r'(wp_\d+) (x|y)Coord "([-\d.]+)"', line)
            if m:
                wp.setdefault(m.group(1), {})[m.group(2)] = float(m.group(3))
    return wp


def goto_links(bag: imp.Bag, drone: str) -> list[tuple[int, str]]:
    out = []
    for ts, line in imp.facts(bag, drone):
        m = re.match(r"goto_\d+ to (wp_\d+)", line)
        if m:
            out.append((ts, m.group(1)))
    return out


def takeoff_heights(bag: imp.Bag) -> set[float]:
    heights = set()
    for drone in bag.drones():
        for _ts, data in bag.messages(f"/{drone}/mission_status"):
            item = json.loads(cdr.decode_string(bytes(data))).get("current_item") or {}
            if item.get("behavior") == "takeoff" and "height" in item.get("args", {}):
                heights.add(float(item["args"]["height"]))
    return heights


# ------------------------------------------------------------------------------------------------ claims

def claims_for(bag: imp.Bag, short: str, poses: dict, wp: dict) -> tuple[list[dict], dict]:
    claims, skipped = [], {"no_waypoint": 0}
    for drone in bag.drones():
        links = goto_links(bag, drone)
        for behavior in ("Takeoff", "GoTo", "Land"):
            for g in imp.goals(bag, drone, behavior):
                if g["status"] != "SUCCEEDED" or not g["end"]:
                    continue
                c = {"id": f"{short}/{drone}/{behavior}/{g['goal_id'].replace('-', '')[:12]}", "bag": short,
                     "drone": drone, "behavior": behavior, "goal": g["goal_id"], "t": g["end"], "target": None}
                if behavior == "GoTo":
                    best = min(links, key=lambda li: abs(li[0] - g["start"]), default=None)
                    if best is None or abs(best[0] - g["start"]) > 300_000_000 or best[1] not in wp or len(wp[best[1]]) < 2:
                        skipped["no_waypoint"] += 1
                        continue
                    c["target"] = {"wp": best[1], "x": wp[best[1]]["x"], "y": wp[best[1]]["y"]}
                claims.append(c)
    return claims, skipped


def box(v: float, tol: float) -> dict:
    return {"op": "between", "min": {"value": v - tol, "unit": "m"}, "max": {"value": v + tol, "unit": "m"}}


def contract(c: dict) -> dict:
    item = {"kind": "independent_sensor", "device": "mocap-localization", "frame": "earth", "bound": False}
    base = {"version": "0.1", "binding": {"attempt": c["goal"]},
            "timing": {"anchor": "claim", "default_window_ms": [-WINDOW_MS, WINDOW_MS]}}
    claim = {"signal": f"claim.{c['behavior'].lower()}_done", "kind": "controller", "device": c["drone"]}
    if c["behavior"] == "GoTo":
        t = c["target"]
        return {**base, "contract": "goto-arrived-at-waypoint",
                "evidence": {"claim": claim, "x": {**item, "signal": "pose.x"}, "y": {**item, "signal": "pose.y"}},
                "conditions": [{"name": "x within 0.20 m", "evidence": "x", **box(t["x"], XY_TOL_M)},
                               {"name": "y within 0.20 m", "evidence": "y", **box(t["y"], XY_TOL_M)}]}
    if c["behavior"] == "Takeoff":
        return {**base, "contract": "takeoff-at-height",
                "evidence": {"claim": claim, "z": {**item, "signal": "pose.z"}},
                "conditions": [{"name": "z within 0.25 m of 1.5 m", "evidence": "z", **box(TAKEOFF_Z_M, TAKEOFF_TOL_M)}]}
    return {**base, "contract": "land-on-ground",
            "evidence": {"claim": claim, "z": {**item, "signal": "pose.z"}},
            "conditions": [{"name": "z at most 0.15 m", "evidence": "z", "op": "le",
                            "value": {"value": LAND_Z_MAX_M, "unit": "m"}}]}


def signals(c: dict) -> list[tuple[str, int]]:
    return [("pose.x", 1), ("pose.y", 2)] if c["behavior"] == "GoTo" else [("pose.z", 3)]


def evidence_records(c: dict, poses: list, span_ns: int = 2_000_000_000) -> list[dict]:
    claim = {"id": f"claim-{c['id']}", "signal": f"claim.{c['behavior'].lower()}_done", "value": "ARRIVED",
             "source": {"system": "aerostack2", "device": c["drone"], "kind": "controller", "provenance": "device"},
             "time": {"source": rfc(c["t"]), "clock_domain": DOMAIN}, "refs": {"attempt": c["goal"]}}
    out = [claim]
    times = [p[0] for p in poses]
    lo, hi = bisect.bisect_left(times, c["t"] - span_ns), bisect.bisect_right(times, c["t"] + span_ns)
    for n, p in enumerate(poses[lo:hi], lo):
        for signal, idx in signals(c):
            out.append({"id": f"pose#{n}:{signal}", "signal": signal, "value": {"value": p[idx], "unit": "m"},
                        "source": dict(SRC), "time": {"source": rfc(p[0]), "clock_domain": DOMAIN}, "frame": "earth"})
    return out


# ------------------------------------------------------------------------------------------------ labels and script

def inside(c: dict, x: float, y: float, z: float, xy_tol: float = XY_TOL_M) -> bool:
    if c["behavior"] == "GoTo":
        return abs(x - c["target"]["x"]) <= xy_tol and abs(y - c["target"]["y"]) <= xy_tol
    if c["behavior"] == "Takeoff":
        return abs(z - TAKEOFF_Z_M) <= TAKEOFF_TOL_M
    return z <= LAND_Z_MAX_M


def label(c: dict, poses: list, xy_tol: float = XY_TOL_M) -> tuple[str, float | None]:
    """The pose sample nearest the claim, read straight from the bag. Separate path: no events, no verifier."""
    times = [p[0] for p in poses]
    i = bisect.bisect_left(times, c["t"])
    near = [poses[j] for j in (i - 1, i) if 0 <= j < len(poses)]
    best = min(near, key=lambda p: abs(p[0] - c["t"]), default=None)
    if best is None or abs(best[0] - c["t"]) > WINDOW_MS * 1_000_000:
        return "NO_REFERENCE", None
    _, x, y, z = best
    if c["behavior"] == "GoTo":
        err = math.hypot(x - c["target"]["x"], y - c["target"]["y"])
    elif c["behavior"] == "Takeoff":
        err = z - TAKEOFF_Z_M
    else:
        err = z
    return ("AT_TARGET" if inside(c, x, y, z, xy_tol) else "NOT_AT_TARGET"), err


def script(c: dict, records: list[dict], xy_tol: float = XY_TOL_M) -> str:
    """A reasonable script over the same evidence: nearest sample within the window, box check. It reads the signals by
    name and ignores frame, clock, device and goal id."""
    vals: dict[str, tuple[int, float]] = {}
    for r in records:
        if not r["signal"].startswith("pose.") or "source" not in r["time"]:
            continue
        d = abs(parse_ms(r["time"]["source"]) - round(c["t"] / 1e6))
        if d <= WINDOW_MS and (r["signal"] not in vals or d < vals[r["signal"]][0]):
            vals[r["signal"]] = (d, r["value"]["value"])
    need = [s for s, _ in signals(c)]
    if any(s not in vals for s in need):
        return "NO_DATA"
    x, y, z = (vals.get("pose.x", (0, 0.0))[1], vals.get("pose.y", (0, 0.0))[1], vals.get("pose.z", (0, 0.0))[1])
    return "SATISFIED" if inside(c, x, y, z, xy_tol) else "VIOLATED"


# ------------------------------------------------------------------------------------------------ faults

def inject(fault: str, c: dict, records: list[dict]) -> list[dict]:
    rs = json.loads(json.dumps(records))
    if fault == "wrong_frame":
        a = math.radians(30)
        xs = {r["time"]["source"]: r for r in rs if r["signal"] == "pose.x"}
        ys = {r["time"]["source"]: r for r in rs if r["signal"] == "pose.y"}
        for k in xs.keys() & ys.keys():
            x, y = xs[k]["value"]["value"], ys[k]["value"]["value"]
            xs[k]["value"]["value"] = math.cos(a) * x - math.sin(a) * y + 5.0
            ys[k]["value"]["value"] = math.sin(a) * x + math.cos(a) * y - 8.0
        for r in rs:
            if r["signal"].startswith("pose."):
                r["frame"] = "odom"
    elif fault == "other_clock":
        for r in rs:
            if r["signal"].startswith("pose."):
                r["time"]["source"] = rfc(parse_ms(r["time"]["source"]) * 1_000_000 + 1_500_000_000)
                r["time"]["clock_domain"] = "other-logger"
    elif fault == "reference_gap":
        rs = [r for r in rs if not (r["signal"].startswith("pose.") and abs(parse_ms(r["time"]["source"]) - c["t"] // 1_000_000) <= 1000)]
    elif fault == "other_attempt":
        rs[0]["refs"]["attempt"] = "goal-somebody-else"
    return rs


# ------------------------------------------------------------------------------------------------ run

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bags", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--faults", action="store_true")
    args = ap.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    work = args.out / "work"
    work.mkdir(exist_ok=True)

    rows, all_claims, ctx = [], [], {}
    summary: dict = {"takeoff_heights_in_mission_items": {}, "skipped": {}}
    for bagdir in sorted(args.bags.glob("rosbag2_real_paths_2_OK_*")):
        short = imp.condition_of(bagdir)
        bag = imp.Bag(bagdir)
        try:
            poses = {d: load_poses(bag, d) for d in bag.drones()}
            claims, skipped = claims_for(bag, short, poses, waypoints(bag))
            summary["takeoff_heights_in_mission_items"][short] = sorted(takeoff_heights(bag))
        finally:
            bag.con.close()
        summary["skipped"][short] = skipped
        for c in claims:
            ctx[c["id"]] = poses[c["drone"]]
        all_claims.extend(claims)

    for c in all_claims:
        records = evidence_records(c, ctx[c["id"]])
        v = verify(contract(c), records, work, c["id"].replace("/", "_"))
        lab, err = label(c, ctx[c["id"]])
        rows.append({"id": c["id"], "bag": c["bag"], "drone": c["drone"], "behavior": c["behavior"], "t_ns": c["t"],
                     "label": lab, "error_m": None if err is None else round(err, 4), "native": "SATISFIED",
                     "script": script(c, records), "verifier": v["verdict"], "reasons": v["reasons"],
                     "verify_seconds": round(v["seconds"], 3),
                     "label_at_0.10": label(c, ctx[c["id"]], 0.10)[0] if c["behavior"] == "GoTo" else None,
                     "label_at_0.30": label(c, ctx[c["id"]], 0.30)[0] if c["behavior"] == "GoTo" else None})
    (args.out / "claims.json").write_text(json.dumps(rows, indent=1) + "\n", encoding="utf-8", newline="\n")

    sample = all_claims[:: max(1, len(all_claims) // 12)]
    summary["determinism_checked"] = len(sample)
    summary["determinism_identical"] = all(
        verify(contract(c), evidence_records(c, ctx[c["id"]]), work, f"a-{i}")["report"]
        == verify(contract(c), evidence_records(c, ctx[c["id"]]), work, f"b-{i}")["report"]
        for i, c in enumerate(sample))

    if args.faults:
        faults = []
        want = {"AT_TARGET": "SATISFIED", "NOT_AT_TARGET": "VIOLATED"}
        for c in all_claims:
            clean = next(r for r in rows if r["id"] == c["id"])
            if clean["label"] not in want:
                continue
            records = evidence_records(c, ctx[c["id"]])
            for f in FAULTS:
                rs = inject(f, c, records)
                v = verify(contract(c), rs, work, f"f-{f}-{c['id'].replace('/', '_')}")
                answer = script(c, rs)
                faults.append({"id": c["id"], "behavior": c["behavior"], "fault": f, "fault_kind": FAULT_KIND[f],
                               "injected": True, "label": clean["label"], "verifier": v["verdict"],
                               "reasons": v["reasons"], "script": answer,
                               "script_matches_label": answer == want[clean["label"]],
                               "script_false_satisfied": answer == "SATISFIED" and clean["label"] != "AT_TARGET",
                               "verifier_false_satisfied": v["verdict"] == "SATISFIED" and clean["label"] != "AT_TARGET"})
        (args.out / "faults.json").write_text(json.dumps(faults, indent=1) + "\n", encoding="utf-8", newline="\n")
        summary["fault_trials"] = len(faults)

    summary["claims"] = len(rows)
    summary["verify_seconds_median"] = round(statistics.median(r["verify_seconds"] for r in rows), 3)
    (args.out / "summary.json").write_text(json.dumps(summary, indent=1) + "\n", encoding="utf-8", newline="\n")
    print(json.dumps(summary, indent=1))
    random.seed(0)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
