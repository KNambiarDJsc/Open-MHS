"""POST-HOC, EXPLORATORY analysis of the real-flight check. None of this was in the plan (EVALUATION_CORESENSE.md).

    python coresense_post_hoc.py --bags BAGS_DIR --out post_hoc.json

Written after the planned run showed that most GoTo successes were more than 0.20 m from the waypoint, to ask why, and
after the landings showed a constant height. It does not change any verdict of the planned run.

1. GoTo: is the distance at SUCCEEDED a fixed offset, or is the drone still approaching when success is declared?
2. Landings: what does the pose feed do after the landing goal succeeds? A pose that repeats one value for seconds while
   its header stamp advances is a stale reference with a fresh stamp, which no timestamp rule can detect.
3. Staleness sensitivity: for each claim, how long had each pose signal been unchanged at the claim (the longest within
   the +-50 ms window)? How many claims would an opt-in `max_unchanged_ms` of T reject, for a range of T? This is how the
   post-hoc rule's limit should be judged: by the trade-off, not by one value.
"""

from __future__ import annotations

import argparse
import bisect
import json
import math
import statistics
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "python"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import coresense_outcomes as co  # noqa: E402
from openmhs.importers import cdr  # noqa: E402
from openmhs.importers import coresense_aerostack2 as imp  # noqa: E402


def goto(bags: Path) -> dict:
    dx, dy, at, later, closest = [], [], [], [], []
    for bagdir in sorted(bags.glob("rosbag2_real_paths_2_OK_*")):
        bag = imp.Bag(bagdir)
        try:
            wp = co.waypoints(bag)
            for drone in bag.drones():
                poses = co.load_poses(bag, drone)
                times = [p[0] for p in poses]
                links = co.goto_links(bag, drone)
                for g in imp.goals(bag, drone, "GoTo"):
                    if g["status"] != "SUCCEEDED" or not g["start"]:
                        continue
                    best = min(links, key=lambda li: abs(li[0] - g["start"]))
                    w = wp[best[1]]

                    def pose_at(ns, times=times, poses=poses):
                        return poses[min(bisect.bisect_left(times, ns), len(poses) - 1)]

                    p, q = pose_at(g["end"]), pose_at(g["end"] + 500_000_000)
                    dx.append(p[1] - w["x"])
                    dy.append(p[2] - w["y"])
                    at.append(math.hypot(p[1] - w["x"], p[2] - w["y"]))
                    later.append(math.hypot(q[1] - w["x"], q[2] - w["y"]))
                    lo, hi = bisect.bisect_left(times, g["start"]), bisect.bisect_right(times, g["end"] + 2_000_000_000)
                    closest.append(min(math.hypot(r[1] - w["x"], r[2] - w["y"]) for r in poses[lo:hi]))
        finally:
            bag.con.close()
    n = len(at)
    q90 = lambda xs: sorted(xs)[int(0.9 * len(xs))]  # noqa: E731
    return {"goals": n,
            "error_vector_mean_m": [round(statistics.mean(dx), 3), round(statistics.mean(dy), 3)],
            "error_vector_sd_m": [round(statistics.pstdev(dx), 3), round(statistics.pstdev(dy), 3)],
            "distance_at_succeeded_m": {"median": round(statistics.median(at), 3), "p90": round(q90(at), 3), "max": round(max(at), 3)},
            "distance_half_second_later_m": {"median": round(statistics.median(later), 3), "max": round(max(later), 3)},
            "closest_approach_within_2s_m": {"median": round(statistics.median(closest), 3), "p90": round(q90(closest), 3), "max": round(max(closest), 3)},
            "goals_at_or_beyond_0.25m_when_succeeded": sum(d >= 0.25 for d in at)}


def landings(bags: Path) -> list[dict]:
    out = []
    for bagdir in sorted(bags.glob("rosbag2_real_paths_2_OK_*")):
        bag = imp.Bag(bagdir)
        try:
            for drone in bag.drones():
                land = [g for g in imp.goals(bag, drone, "Land") if g["end"] and g["status"] == "SUCCEEDED"]
                if not land:
                    continue
                rows = [(ts, cdr.decode_pose_stamped(bytes(d))) for ts, d in bag.messages(f"/{drone}/self_localization/pose")]
                tail = [(ts, p) for ts, p in rows if ts >= land[0]["end"]]
                stamps = [p.sec * 10**9 + p.nanosec for _, p in tail]
                out.append({"flight": imp.condition_of(bagdir), "drone": drone, "samples_after": len(tail),
                            "distinct_poses_after": len({p.position for _, p in tail}),
                            "header_stamp_advances": sum(1 for a, b in zip(stamps, stamps[1:]) if b > a),
                            "z_after_m": [round(min(p.position[2] for _, p in tail), 3), round(max(p.position[2] for _, p in tail), 3)],
                            "seconds_after": round((tail[-1][0] - land[0]["end"]) / 1e9, 1)})
        finally:
            bag.con.close()
    return out


def staleness(bags: Path) -> dict:
    limits = [50, 100, 150, 200, 300, 400, 600, 1000]
    per_claim = []
    for bagdir in sorted(bags.glob("rosbag2_real_paths_2_OK_*")):
        bag = imp.Bag(bagdir)
        try:
            poses = {d: co.load_poses(bag, d) for d in bag.drones()}
            claims, _ = co.claims_for(bag, imp.condition_of(bagdir), poses, co.waypoints(bag))
        finally:
            bag.con.close()
        for c in claims:
            ps = poses[c["drone"]]
            times = [p[0] for p in ps]
            lo = bisect.bisect_left(times, c["t"] - 2_000_000_000)
            hi = bisect.bisect_right(times, c["t"] + 2_000_000_000)
            worst = 0.0
            for _signal, axis in co.signals(c):
                start = None
                last = None
                for p in ps[lo:hi]:
                    if last is None or p[axis] != last:
                        start, last = p[0], p[axis]
                    if abs(p[0] - c["t"]) <= co.WINDOW_MS * 1_000_000:
                        worst = max(worst, (p[0] - start) / 1e6)
            per_claim.append({"behavior": c["behavior"], "unchanged_ms": round(worst, 1)})
    table = {}
    for t in limits:
        flagged = [x for x in per_claim if x["unchanged_ms"] > t]
        table[str(t)] = {"claims_rejected": len(flagged),
                         "by_behavior": {b: sum(x["behavior"] == b for x in flagged) for b in ("GoTo", "Takeoff", "Land")}}
    values = sorted(x["unchanged_ms"] for x in per_claim)
    return {"claims": len(per_claim), "unchanged_at_claim_ms": {"median": values[len(values) // 2], "p90": values[int(0.9 * len(values))],
                                                              "max": values[-1]}, "claims_rejected_by_limit_ms": table}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bags", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()
    result = {"note": "post-hoc and exploratory; not part of the plan", "goto": goto(args.bags),
              "landings": landings(args.bags), "staleness": staleness(args.bags)}
    args.out.write_text(json.dumps(result, indent=1) + "\n", encoding="utf-8", newline="\n")
    print(json.dumps(result, indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
