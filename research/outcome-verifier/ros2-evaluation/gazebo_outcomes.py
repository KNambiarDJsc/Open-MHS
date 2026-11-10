"""The Gazebo experiment: native ROS 2 action status and odometry against the simulator's own physics pose.

    python gazebo_outcomes.py --run RUN_DIR --evidence EVIDENCE.jsonl --out OUT_DIR [--faults]

RUN_DIR is what gazebo/run_experiment.sh wrote (intent.jsonl, urdf.xml, gz_raw.jsonl). EVIDENCE is
`ros2_evidence.py convert RUN_DIR/bag --mapping mappings/gazebo_openamr.json`. Labels come from gz_raw.jsonl, Gazebo's
pose messages written straight to a file, not through ROS 2, the bag or the bridge. The plan is EVALUATION_GAZEBO.md.
Needs the `openmhs` binary (OPENMHS_BIN, or on PATH). SIMULATION ONLY.
"""

from __future__ import annotations

import argparse
import bisect
import json
import math
import random
import statistics
import xml.etree.ElementTree as ET
from pathlib import Path

from malaga_checkpoints import rfc, seconds, verify

LIFT_TOL_M = 0.010
LIFT_WINDOW_MS = (0, 200)
BASE_BOX_M = 0.050
BASE_WINDOW_MS = 50
MODEL, CARRIAGE = "openamrobot", "lift_carriage_link"
DOMAIN = "sim:gazebo-openamr"
FAULTS = ["self_confirming", "other_clock", "reference_gap", "other_attempt", "frozen_reference", "reordered"]
FAULT_KIND = {"self_confirming": "source", "other_clock": "values", "reference_gap": "availability",
              "other_attempt": "metadata", "frozen_reference": "values, undetectable by design", "reordered": "nothing"}
SIM = {"system": "experiment", "provenance": "simulator"}


# ---------------------------------------------------------------- inputs

def load_evidence(path: Path) -> dict[str, list[dict]]:
    by: dict[str, list[dict]] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        r = json.loads(line)
        by.setdefault(r["signal"], []).append(r)
    for rs in by.values():
        rs.sort(key=lambda r: (seconds(r["time"]["source"]) if "source" in r["time"] else -1.0, r["id"]))
    return by


def t_of(r: dict) -> float | None:
    return seconds(r["time"]["source"]) if "source" in r["time"] else None


def v_of(r: dict) -> float:
    v = r["value"]
    return v["value"] if isinstance(v, dict) else v


def carriage_offset(urdf_path: Path) -> tuple[float, float, float]:
    """lift_carriage_link's position in the URDF root frame at lift_joint = 0, from the run's own URDF. Refuses a chain
    with rotations, which this sum would get wrong."""
    text = urdf_path.read_text(encoding="utf-8")
    text = text[text.index("<robot"):text.rindex("</robot>") + len("</robot>")]
    robot = ET.fromstring(text)
    parent_of = {j.find("child").get("link"): j for j in robot.findall("joint")}
    x = y = z = 0.0
    link = CARRIAGE
    while link in parent_of:
        j = parent_of[link]
        o = j.find("origin")
        if o is not None:
            if any(abs(float(a)) > 1e-12 for a in o.get("rpy", "0 0 0").split()):
                raise SystemExit(f"joint {j.get('name')} has a rotation; the offset sum does not handle it")
            dx, dy, dz = (float(a) for a in o.get("xyz", "0 0 0").split())
            x, y, z = x + dx, y + dy, z + dz
        link = j.find("parent").get("link")
    return x, y, z


def raw_gz(path: Path) -> dict[str, list[tuple]]:
    """Label source: (t, carriage z) and (t, model x, model y) from Gazebo's own messages."""
    car, base = [], []
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            d = json.loads(line)
            s = d.get("header", {}).get("stamp", {})
            t = int(s.get("sec", 0)) + int(s.get("nsec", 0)) / 1e9
            if t == 0:
                continue
            for p in d.get("pose", []):
                pos = p.get("position", {})
                if p.get("name") == CARRIAGE:
                    car.append((t, float(pos.get("z", 0.0))))
                elif p.get("name") == MODEL:
                    base.append((t, float(pos.get("x", 0.0)), float(pos.get("y", 0.0))))
    car.sort()
    base.sort()
    return {"carriage": car, "base": base}


def first_after(samples: list[tuple], t: float, within: float):
    i = bisect.bisect_left(samples, (t,))
    return samples[i] if i < len(samples) and samples[i][0] - t <= within else None


def nearest(samples: list[tuple], t: float, within: float):
    i = bisect.bisect_left(samples, (t,))
    best = [s for s in samples[max(0, i - 1):i + 1] if abs(s[0] - t) <= within]
    return min(best, key=lambda s: abs(s[0] - t)) if best else None


def yaw(qz: float, qw: float) -> float:
    return 2.0 * math.atan2(qz, qw)


# ---------------------------------------------------------------- claims

def lift_claims(sig: dict, intent: list[dict], offset: tuple, t_end: float) -> tuple[list[dict], dict]:
    cal = {"id": "cal-lift-carriage-offset", "signal": "calibration", "value": "lift-carriage-offset",
           "source": {**SIM, "device": "lift_controller", "kind": "calibration"},
           "calibration": {"name": "lift-carriage-offset", "applies_to": "lift_controller",
                           "valid_from": rfc(0.0), "valid_until": rfc(t_end + 60)}}
    by_goal = {i["goal_id"]: i for i in intent if i.get("kind") == "lift" and i.get("goal_id")}
    out = []
    for r in sig.get("lift.action.status", []):
        if r["value"] != "SUCCEEDED" or r["refs"]["attempt"] not in by_goal:
            continue
        task = by_goal[r["refs"]["attempt"]]
        t = t_of(r)
        if t is None:
            continue
        out.append({"id": task["task"], "kind": "lift", "t": t, "target": task["target_m"],
                    "expect_z": task["target_m"] + offset[2], "goal": r["refs"]["attempt"], "record": {
                        "id": f"claim-{task['task']}", "signal": "claim.lift_at_target", "value": "AT_TARGET",
                        "source": {**SIM, "device": "lift_controller", "kind": "controller"},
                        "time": dict(r["time"]), "refs": {"attempt": r["refs"]["attempt"]},
                        "relations": [{"type": "wasDerivedFrom", "id": r["id"]},
                                      {"type": "wasDerivedFrom", "id": cal["id"]}]}})
    return out, cal


def base_claims(sig: dict, t_end: float) -> tuple[list[dict], dict, dict]:
    odom = {k: {r["time"]["source"]: v_of(r) for r in sig[f"odom.{k}"]} for k in ("x", "y", "qz", "qw")}
    gz = {k: {r["time"]["source"]: v_of(r) for r in sig[f"base.{k}"]} for k in ("x", "y", "qz", "qw")}
    stamps = sorted((s for s in odom["x"] if all(s in odom[k] for k in odom)), key=seconds)
    gz_stamps = sorted((s for s in gz["x"] if all(s in gz[k] for k in gz)), key=seconds)
    gz_t = [seconds(s) for s in gz_stamps]
    # Calibration: the first odometry message, and Gazebo's model pose at the same moment, before any task.
    s0 = stamps[0]
    i = min(range(len(gz_t)), key=lambda k: abs(gz_t[k] - seconds(s0)))
    g0 = gz_stamps[i]
    dyaw = yaw(gz["qz"][g0], gz["qw"][g0]) - yaw(odom["qz"][s0], odom["qw"][s0])
    c, s = math.cos(dyaw), math.sin(dyaw)
    tx = gz["x"][g0] - (c * odom["x"][s0] - s * odom["y"][s0])
    ty = gz["y"][g0] - (s * odom["x"][s0] + c * odom["y"][s0])
    cal = {"id": "cal-odom-origin", "signal": "calibration", "value": "odom-origin",
           "source": {**SIM, "device": "diff_drive_odometry", "kind": "calibration"},
           "calibration": {"name": "odom-origin", "applies_to": "diff_drive_odometry",
                           "valid_from": rfc(seconds(s0)), "valid_until": rfc(t_end + 60)}}
    stats = {"odom_first_stamp": s0, "gz_sample_gap_ms": round(abs(gz_t[i] - seconds(s0)) * 1000, 1),
             "yaw_offset_rad": round(dyaw, 6), "tx_m": round(tx, 6), "ty_m": round(ty, 6)}
    out, next_t = [], math.ceil(seconds(s0))
    for st in stamps:
        t = seconds(st)
        if t < next_t:
            continue
        next_t = math.floor(t) + 1
        ox, oy = odom["x"][st], odom["y"][st]
        x, y = c * ox - s * oy + tx, s * ox + c * oy + ty
        k = f"O{int(round(t)):04d}"
        out.append({"id": k, "kind": "base", "t": t, "x": x, "y": y, "odom": (ox, oy), "record": {
            "id": f"claim-{k}", "signal": "claim.base_at", "value": "AT_POSITION",
            "source": {**SIM, "device": "diff_drive_odometry", "kind": "controller"},
            "time": {"source": st, "clock_domain": DOMAIN, "uncertainty_ms": 0}, "refs": {"attempt": k},
            "frame": "gz:world",
            "relations": [{"type": "wasDerivedFrom", "id": f"odom@{st}"}, {"type": "wasDerivedFrom", "id": cal["id"]}]}})
    return out, cal, stats


def contract(c: dict) -> dict:
    if c["kind"] == "lift":
        return {
            "contract": "lift-at-target-when-succeeded", "version": "0.1", "binding": {"attempt": c["goal"]},
            "timing": {"anchor": "claim", "default_window_ms": list(LIFT_WINDOW_MS)},
            "evidence": {
                "claim": {"signal": "claim.lift_at_target", "kind": "controller", "device": "lift_controller",
                          "calibration": ["lift-carriage-offset"]},
                "z": {"signal": "lift_carriage.z", "kind": "independent_sensor", "device": "gz-physics",
                      "frame": "gz:openamrobot", "bound": False}},
            "conditions": [{"name": "carriage within 10 mm of the target", "evidence": "z", "op": "between",
                            "min": {"value": c["expect_z"] - LIFT_TOL_M, "unit": "m"},
                            "max": {"value": c["expect_z"] + LIFT_TOL_M, "unit": "m"}}]}

    def box(v: float) -> dict:
        return {"op": "between", "min": {"value": v - BASE_BOX_M, "unit": "m"},
                "max": {"value": v + BASE_BOX_M, "unit": "m"}}
    return {
        "contract": "base-where-odometry-says", "version": "0.1", "binding": {"attempt": c["id"]},
        "timing": {"anchor": "claim", "default_window_ms": [-BASE_WINDOW_MS, BASE_WINDOW_MS]},
        "evidence": {
            "claim": {"signal": "claim.base_at", "kind": "controller", "device": "diff_drive_odometry",
                      "calibration": ["odom-origin"]},
            "x": {"signal": "base.x", "kind": "independent_sensor", "device": "gz-physics", "frame": "gz:world",
                  "bound": False},
            "y": {"signal": "base.y", "kind": "independent_sensor", "device": "gz-physics", "frame": "gz:world",
                  "bound": False}},
        "conditions": [{"name": "x within 50 mm", "evidence": "x", **box(c["x"])},
                       {"name": "y within 50 mm", "evidence": "y", **box(c["y"])}]}


REF = {"lift": ["lift_carriage.z"], "base": ["base.x", "base.y"]}
CONTEXT = {"lift": ["lift_joint.position"], "base": ["odom.x", "odom.y", "odom.qz", "odom.qw"]}


def evidence_for(c: dict, sig: dict, cal: dict, span: float = 2.0) -> list[dict]:
    out = [c["record"], cal]
    for name in REF[c["kind"]] + CONTEXT[c["kind"]]:
        rs = sig.get(name, [])
        ts = [t_of(r) for r in rs]
        lo = bisect.bisect_left(ts, c["t"] - span)
        hi = bisect.bisect_right(ts, c["t"] + span)
        out.extend(rs[lo:hi])
    return out


# ---------------------------------------------------------------- labels and baselines

def label(c: dict, raw: dict) -> tuple[str, float | None]:
    if c["kind"] == "lift":
        s = first_after(raw["carriage"], c["t"], LIFT_WINDOW_MS[1] / 1000)
        if s is None:
            return "NO_REFERENCE", None
        err = s[1] - c["expect_z"]
        return ("AT_TARGET" if abs(err) <= LIFT_TOL_M else "NOT_AT_TARGET"), err
    s = nearest(raw["base"], c["t"], BASE_WINDOW_MS / 1000)
    if s is None:
        return "NO_REFERENCE", None
    ex, ey = s[1] - c["x"], s[2] - c["y"]
    inside = abs(ex) <= BASE_BOX_M and abs(ey) <= BASE_BOX_M
    return ("AT_TARGET" if inside else "NOT_AT_TARGET"), math.hypot(ex, ey)


def _pick(records: list[dict], signal: str, t: float, after_within: float | None = None, within: float | None = None):
    rs = [r for r in records if r["signal"] == signal and "source" in r["time"]]
    if after_within is not None:
        rs = [r for r in rs if 0 <= t_of(r) - t <= after_within]
        return min(rs, key=t_of) if rs else None
    rs = [r for r in rs if abs(t_of(r) - t) <= within]
    return min(rs, key=lambda r: abs(t_of(r) - t)) if rs else None


def gt_script(c: dict, records: list[dict]) -> str:
    """The strongest few-line baseline: reads the reference signal by name, ignores device, kind, frame, clock,
    attempt and calibration, as most scripts do."""
    if c["kind"] == "lift":
        r = _pick(records, "lift_carriage.z", c["t"], after_within=LIFT_WINDOW_MS[1] / 1000)
        if r is None:
            return "NO_DATA"
        return "SATISFIED" if abs(v_of(r) - c["expect_z"]) <= LIFT_TOL_M else "VIOLATED"
    x = _pick(records, "base.x", c["t"], within=BASE_WINDOW_MS / 1000)
    y = _pick(records, "base.y", c["t"], within=BASE_WINDOW_MS / 1000)
    if x is None or y is None:
        return "NO_DATA"
    inside = abs(v_of(x) - c["x"]) <= BASE_BOX_M and abs(v_of(y) - c["y"]) <= BASE_BOX_M
    return "SATISFIED" if inside else "VIOLATED"


def joint_script(c: dict, records: list[dict]) -> str | None:
    """What a roboticist without simulator access would check: the robot's own joint state."""
    if c["kind"] != "lift":
        return None
    r = _pick(records, "lift_joint.position", c["t"], after_within=LIFT_WINDOW_MS[1] / 1000)
    if r is None:
        return "NO_DATA"
    return "SATISFIED" if abs(v_of(r) - c["target"]) <= LIFT_TOL_M else "VIOLATED"


# ---------------------------------------------------------------- faults

def inject(fault: str, c: dict, records: list[dict], offset: tuple, base_cal: dict | None) -> tuple[dict, list[dict]]:
    """A copy of the claim's evidence with one fault. Every fault is ours, not the run's."""
    rs = json.loads(json.dumps(records))
    con = contract(c)
    ref = set(REF[c["kind"]])
    if fault == "self_confirming":
        rs = [r for r in rs if r["signal"] not in ref]
        if c["kind"] == "lift":
            for r in [r for r in records if r["signal"] == "lift_joint.position" and "source" in r["time"]]:
                rs.append({**json.loads(json.dumps(r)), "id": f"selfconf:{r['id']}", "signal": "lift_carriage.z",
                           "value": {"value": v_of(r) + offset[2], "unit": "m"}, "frame": "gz:openamrobot"})
        else:
            cs, sn = math.cos(base_cal["yaw_offset_rad"]), math.sin(base_cal["yaw_offset_rad"])
            ox = {r["time"]["source"]: r for r in records if r["signal"] == "odom.x"}
            oy = {r["time"]["source"]: v_of(r) for r in records if r["signal"] == "odom.y"}
            for st, rx in ox.items():
                if st not in oy:
                    continue
                x, y = v_of(rx), oy[st]
                for sig_name, val in (("base.x", cs * x - sn * y + base_cal["tx_m"]),
                                      ("base.y", sn * x + cs * y + base_cal["ty_m"])):
                    rs.append({**json.loads(json.dumps(rx)), "id": f"selfconf:{rx['id']}:{sig_name}", "signal": sig_name,
                               "value": {"value": val, "unit": "m"}, "frame": "gz:world"})
    elif fault == "other_clock":
        for r in rs:
            if r["signal"] in ref and "source" in r["time"]:
                r["time"]["source"] = rfc(t_of(r) + 1.5)
                r["time"]["clock_domain"] = "sim:another-simulator"
                r["time"].pop("uncertainty_ms", None)
    elif fault == "reference_gap":
        rs = [r for r in rs if not (r["signal"] in ref and "source" in r["time"] and abs(t_of(r) - c["t"]) <= 1.0)]
    elif fault == "other_attempt":
        rs[0]["refs"]["attempt"] = "goal-somebody-else"
    elif fault == "frozen_reference":
        for name in ref:
            mine = [r for r in rs if r["signal"] == name and "source" in r["time"]]
            if not mine:
                continue
            old = min(mine, key=lambda r: abs(t_of(r) - (c["t"] - 2.0)))
            for r in mine:
                if abs(t_of(r) - c["t"]) <= 0.5:
                    r["value"] = json.loads(json.dumps(old["value"]))
    elif fault == "reordered":
        random.Random(c["id"]).shuffle(rs)
    return con, rs


# ---------------------------------------------------------------- run

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", type=Path, required=True)
    ap.add_argument("--evidence", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--faults", action="store_true")
    args = ap.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    work = args.out / "work"
    work.mkdir(exist_ok=True)

    sig = load_evidence(args.evidence)
    intent = [json.loads(line) for line in (args.run / "intent.jsonl").read_text(encoding="utf-8").splitlines()]
    t_end = max(t_of(r) for rs in sig.values() for r in rs if t_of(r) is not None)
    offset = carriage_offset(args.run / "urdf.xml")
    raw = raw_gz(args.run / "gz_raw.jsonl")
    lc, lift_cal = lift_claims(sig, intent, offset, t_end)
    bc, base_cal, base_stats = base_claims(sig, t_end)
    summary: dict = {"run": args.run.name, "carriage_offset_m": offset, "odom_calibration": base_stats,
                     "lift_goals_sent": sum(1 for i in intent if i.get("kind") == "lift"),
                     "lift_results": {str(i["task"]): i.get("result_status") for i in intent if i.get("kind") == "lift"},
                     "status_uncertainty_ms": sorted({r["record"]["time"].get("uncertainty_ms") for r in lc})}

    rows = []
    for c in lc + bc:
        cal = lift_cal if c["kind"] == "lift" else base_cal
        records = evidence_for(c, sig, cal)
        v = verify(contract(c), records, work, c["id"])
        lab, err = label(c, raw)
        row = {"id": c["id"], "kind": c["kind"], "t": round(c["t"], 3), "label": lab,
               "error_m": None if err is None else round(err, 4), "native": "SATISFIED",
               "gt_script": gt_script(c, records), "joint_script": joint_script(c, records),
               "verifier": v["verdict"], "reasons": v["reasons"], "verify_seconds": round(v["seconds"], 3)}
        if c["kind"] == "lift":
            row.update(target_m=c["target"], goal=c["goal"])
        rows.append(row)
    (args.out / "claims.json").write_text(json.dumps(rows, indent=1) + "\n", encoding="utf-8")

    sample = (lc + bc)[:: max(1, len(lc + bc) // 12)]
    summary["determinism_checked"] = len(sample)
    summary["determinism_identical"] = all(
        verify(contract(c), evidence_for(c, sig, lift_cal if c["kind"] == "lift" else base_cal), work, f"det-a-{c['id']}")["report"]
        == verify(contract(c), evidence_for(c, sig, lift_cal if c["kind"] == "lift" else base_cal), work, f"det-b-{c['id']}")["report"]
        for c in sample)
    full = [json.loads(line) for line in args.evidence.read_text(encoding="utf-8").splitlines()]
    sub = lc[:2] + bc[:: max(1, len(bc) // 2)][:2]
    summary["selection_checked"] = len(sub)
    summary["selection_same_verdict"] = all(
        verify(contract(c), [c["record"], lift_cal if c["kind"] == "lift" else base_cal, *full], work, f"full-{c['id']}")["verdict"]
        == next(r["verifier"] for r in rows if r["id"] == c["id"]) for c in sub)

    if args.faults:
        drive = [i for i in intent if i.get("kind") == "base"]
        lo, hi = min(i["start_sim"] for i in drive), max(i["stop_sim"] for i in drive) + 3.0
        in_drive = [c for c in bc if lo <= c["t"] <= hi]
        targets = lc + in_drive[:: max(1, len(in_drive) // 10)][:10]
        label_of = {r["id"]: r["label"] for r in rows}
        faults = []
        for c in targets:
            if label_of[c["id"]] == "NO_REFERENCE":
                continue
            cal = lift_cal if c["kind"] == "lift" else base_cal
            records = evidence_for(c, sig, cal)
            truth = {"AT_TARGET": "SATISFIED", "NOT_AT_TARGET": "VIOLATED"}[label_of[c["id"]]]
            clean = next(r for r in rows if r["id"] == c["id"])
            for f in FAULTS:
                con, rs = inject(f, c, records, offset, base_stats)
                v = verify(con, rs, work, f"fault-{f}-{c['id']}")
                answer = gt_script(c, rs)
                faults.append({"id": c["id"], "kind": c["kind"], "fault": f, "fault_kind": FAULT_KIND[f],
                               "injected": True, "label": label_of[c["id"]], "clean_verifier": clean["verifier"],
                               "verifier": v["verdict"], "reasons": v["reasons"], "gt_script": answer,
                               "gt_script_matches_label": answer == truth,
                               "gt_script_false_satisfied": answer == "SATISFIED" and truth != "SATISFIED",
                               "verifier_false_satisfied": v["verdict"] == "SATISFIED" and truth != "SATISFIED"})
        (args.out / "faults.json").write_text(json.dumps(faults, indent=1) + "\n", encoding="utf-8")
        summary["fault_trials"] = len(faults)

    summary["claims"] = {k: sum(1 for r in rows if r["kind"] == k) for k in ("lift", "base")}
    summary["verify_seconds_median"] = round(statistics.median(r["verify_seconds"] for r in rows), 3)
    (args.out / "summary.json").write_text(json.dumps(summary, indent=1) + "\n", encoding="utf-8")
    print(json.dumps(summary, indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
