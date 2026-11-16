"""Check what a real drone flight's action status claimed against motion capture, offline (RFC 0004), at import time.

Used by ``python -m openmhs.importers.coresense_aerostack2 --verify``. Needs the ``openmhs`` binary (``OPENMHS_BIN`` or
``PATH``) and reads the bags read-only. Standard library otherwise.

What is verified: every ``Takeoff``, ``GoTo`` and ``Land`` goal that reached ``SUCCEEDED``. The claim is the action's
own status, at the time of that status. The reference is the motion-capture pose (``/droneN/self_localization/pose``),
which also feeds the drone's controller: independent of the action's claim, not of the sensor the behavior used.

**The tolerances are not constants of this module.** They are the *behavior thresholds* a deployment declares in its
manifest (a ``config`` component named ``behavior_thresholds``), and ``contract`` reads them from there. Change the
threshold and the manifest, therefore the release, changes with it, so a verdict can always be traced to the number it
was judged against and a different assumption is a different release instead of a silent re-run.
"""

from __future__ import annotations

import bisect
import json
import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path
from typing import Any

from . import cdr
from . import coresense_aerostack2 as imp

CLOCK = "coresense-recorder"
SOURCE = {"system": "ros2-bag", "device": "mocap-localization", "kind": "independent_sensor", "provenance": "device"}

# What the CoreSense manifest declares when verification is on. `basis` says where each number comes from.
DEFAULT_THRESHOLDS: dict[str, Any] = {
    "go_to_xy_m": 0.20,
    "takeoff_height_m": 1.5,
    "takeoff_z_m": 0.25,
    "land_z_max_m": 0.15,
    "window_ms": 50,
    "basis": ("go_to_xy_m is the Aerostack2 go_to_threshold default (config_default.yaml on main); the flights' own "
              "configuration is not published, so it is an assumption. takeoff_height_m is the mission items' height "
              "argument (checked against every mission_status message). takeoff_z_m, land_z_max_m and window_ms were "
              "chosen in the evaluation plan and are not read from the flights."),
}
THRESHOLD_KEYS = ("go_to_xy_m", "takeoff_height_m", "takeoff_z_m", "land_z_max_m", "window_ms")


def binary() -> str:
    return os.environ.get("OPENMHS_BIN") or shutil.which("openmhs") or "openmhs"


# ---------------------------------------------------------------------------------------------- reading the bag

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


def claims_for(bag: imp.Bag, short: str, poses: dict, wp: dict) -> tuple[list[dict], dict]:
    """One claim per goal that reached SUCCEEDED. A GoTo needs its waypoint (joined from the knowledge base by the
    ``goto_N to wp_K`` fact nearest the goal's start, within 300 ms); one without is skipped and counted."""
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


# ---------------------------------------------------------------------------------------------- contract, evidence

def box(v: float, tol: float) -> dict:
    return {"op": "between", "min": {"value": v - tol, "unit": "m"}, "max": {"value": v + tol, "unit": "m"}}


def contract(c: dict, th: dict[str, Any], max_unchanged_ms: int | None = None) -> dict:
    """The outcome contract (RFC 0004) of one claim, judged against the declared thresholds ``th``."""
    item: dict[str, Any] = {"kind": "independent_sensor", "device": "mocap-localization", "frame": "earth", "bound": False}
    if max_unchanged_ms is not None:
        item["max_unchanged_ms"] = max_unchanged_ms
    w = th["window_ms"]
    base = {"version": "0.1", "binding": {"attempt": c["goal"]},
            "timing": {"anchor": "claim", "default_window_ms": [-w, w]}}
    claim = {"signal": f"claim.{c['behavior'].lower()}_done", "kind": "controller", "device": c["drone"]}
    if c["behavior"] == "GoTo":
        t, tol = c["target"], th["go_to_xy_m"]
        return {**base, "contract": "goto-arrived-at-waypoint",
                "evidence": {"claim": claim, "x": {**item, "signal": "pose.x"}, "y": {**item, "signal": "pose.y"}},
                "conditions": [{"name": f"x within {tol:.2f} m", "evidence": "x", **box(t["x"], tol)},
                               {"name": f"y within {tol:.2f} m", "evidence": "y", **box(t["y"], tol)}]}
    if c["behavior"] == "Takeoff":
        h, tol = th["takeoff_height_m"], th["takeoff_z_m"]
        return {**base, "contract": "takeoff-at-height",
                "evidence": {"claim": claim, "z": {**item, "signal": "pose.z"}},
                "conditions": [{"name": f"z within {tol:.2f} m of {h} m", "evidence": "z", **box(h, tol)}]}
    return {**base, "contract": "land-on-ground",
            "evidence": {"claim": claim, "z": {**item, "signal": "pose.z"}},
            "conditions": [{"name": f"z at most {th['land_z_max_m']:.2f} m", "evidence": "z", "op": "le",
                            "value": {"value": th["land_z_max_m"], "unit": "m"}}]}


def signals(c: dict) -> list[tuple[str, int]]:
    return [("pose.x", 1), ("pose.y", 2)] if c["behavior"] == "GoTo" else [("pose.z", 3)]


def evidence_records(c: dict, poses: list, span_ns: int = 2_000_000_000) -> list[dict]:
    claim = {"id": f"claim-{c['id']}", "signal": f"claim.{c['behavior'].lower()}_done", "value": "ARRIVED",
             "source": {"system": "aerostack2", "device": c["drone"], "kind": "controller", "provenance": "device"},
             "time": {"source": imp.rfc(c["t"]), "clock_domain": CLOCK}, "refs": {"attempt": c["goal"]}}
    out = [claim]
    times = [p[0] for p in poses]
    lo, hi = bisect.bisect_left(times, c["t"] - span_ns), bisect.bisect_right(times, c["t"] + span_ns)
    for n, p in enumerate(poses[lo:hi], lo):
        for signal, idx in signals(c):
            out.append({"id": f"pose#{n}:{signal}", "signal": signal, "value": {"value": p[idx], "unit": "m"},
                        "source": dict(SOURCE), "time": {"source": imp.rfc(p[0]), "clock_domain": CLOCK}, "frame": "earth"})
    return out


# ---------------------------------------------------------------------------------------------- the verifier

def run_verifier(contract_obj: dict, records: list[dict], work: Path, name: str) -> dict:
    """``openmhs outcome verify`` on one claim. Exit 0, 3, 4 are the three verdicts; anything else is an error."""
    cpath, epath = work / f"{name}.contract.json", work / f"{name}.evidence.jsonl"
    cpath.write_text(json.dumps(contract_obj, sort_keys=True), encoding="utf-8")
    epath.write_text("".join(json.dumps(r, sort_keys=True) + "\n" for r in records), encoding="utf-8")
    p = subprocess.run([binary(), "--json", "outcome", "verify", "--contract", str(cpath), "--evidence", str(epath)],
                       capture_output=True, text=True)
    if p.returncode not in (0, 3, 4):
        raise RuntimeError(f"the verifier failed on {name} (exit {p.returncode}): {p.stderr.strip()}")
    report = json.loads(p.stdout)
    return {"verdict": report["verdict"], "reasons": sorted({r["code"] for r in report["reasons"]}), "report": p.stdout}


def thresholds_from(manifest: dict) -> dict[str, Any]:
    """The declared thresholds of a manifest. Refuses a manifest that does not declare all of them."""
    for comp in manifest.get("components", []):
        if comp.get("kind") == "config" and comp.get("name") == "behavior_thresholds":
            missing = [k for k in THRESHOLD_KEYS if k not in comp]
            if missing:
                raise ValueError(f"behavior_thresholds does not declare {', '.join(missing)}")
            return {k: comp[k] for k in THRESHOLD_KEYS}
    raise ValueError("the manifest declares no behavior_thresholds component; nothing to verify against")


def verify_bag(bag: imp.Bag, short: str, th: dict[str, Any]) -> tuple[list[dict], dict]:
    """Verdicts for every succeeded Takeoff, GoTo and Land goal of one bag, judged against ``th``.

    Refuses (``ValueError``) when a mission item asks for a takeoff height other than the declared one: a contract the
    flight itself contradicts would make every Takeoff verdict meaningless.
    """
    heights = takeoff_heights(bag)
    if heights and heights != {float(th["takeoff_height_m"])}:
        raise ValueError(f"{short}: mission items ask for takeoff heights {sorted(heights)}, the manifest declares "
                         f"{th['takeoff_height_m']}")
    poses = {d: load_poses(bag, d) for d in bag.drones()}
    claims, skipped = claims_for(bag, short, poses, waypoints(bag))
    verdicts = []
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        for c in claims:
            v = run_verifier(contract(c, th), evidence_records(c, poses[c["drone"]]), work, "claim")
            verdicts.append({"claim": c, "verdict": v["verdict"], "reasons": v["reasons"],
                             "contract": contract(c, th)["contract"]})
    return verdicts, skipped
