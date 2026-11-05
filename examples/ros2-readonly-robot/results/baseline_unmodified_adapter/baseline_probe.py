"""What the ROS 2 adapter did before this example changed it.

Run with `adapters/python/ros2/ros2_adapter.py` as it is at commit a5d8d94 (check that file out
first). It uses array positions, because that adapter has no selector, and no ``simulated_source``.
Writes ``baseline.json`` next to this file. These are observations, not a test: each one is the reason
for a change described in DESIGN.md, or the reason a change was dropped.
"""

from __future__ import annotations

import json
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))

from robot_lab import DEVICE, RobotLab  # noqa: E402


def old_style(m: dict) -> None:
    config = m["config"]
    config.pop("simulated_source")
    t = config["topics"]
    t["/joint_states"]["fields"] = {
        "joint.shoulder.position": "position.0", "joint.elbow.position": "position.1",
        "joint.wrist.position": "position.2"}
    t["/diagnostics"]["fields"] = {
        "diagnostics.driver.level": "status.1.level", "diagnostics.driver.message": "status.1.message"}


def read(client, capability: str) -> dict:
    try:
        r = client.read(DEVICE, capability)
        return {"value": r.state.value, "source": str(r.state.source), "quality": str(r.state.quality),
                "timestamp": r.state.timestamp, "age_ms": r.age_ms, "usable": r.usable}
    except Exception as e:  # noqa: BLE001 - the failure is the observation
        return {"error": getattr(e, "code", type(e).__name__), "message": str(e)[:200]}


def main() -> None:
    out: dict = {"adapter_commit": "a5d8d94"}
    with RobotLab(manifest_change=old_style) as lab:
        client = lab.agent_client()
        lab.wait_readable("joint.elbow.position")
        out["graph_of_observer"] = lab.ros("graph", "openmhs_observer")
        out["normal_shoulder"] = read(client, "joint.shoulder.position")
        out["normal_elbow"] = read(client, "joint.elbow.position")
        out["diagnostic_level"] = read(client, "diagnostics.driver.level")

        lab.sim.send("fault", name="reordered_joints")
        time.sleep(0.5)
        out["reordered_joints_shoulder"] = read(client, "joint.shoulder.position")
        lab.sim.send("clear")

        lab.sim.send("fault", name="range_joint")
        time.sleep(0.5)
        out["out_of_range_elbow"] = read(client, "joint.elbow.position")
        lab.sim.send("clear")

        lab.sim.send("fault", name="regress_small")
        time.sleep(0.5)
        stamps = []
        for _ in range(200):
            r = read(client, "joint.elbow.position")
            if "timestamp" in r:
                stamps.append((r["timestamp"], r["usable"]))
            time.sleep(0.01)
        lab.sim.send("clear")
        # The gateway trims trailing zeros, so compare as numbers. All within one minute of each other
        # is not assumed: the whole time of day is parsed.
        def seconds(text) -> float:
            h, m, s = str(text).split("T")[1].rstrip("Z").split(":")
            return int(h) * 3600 + int(m) * 60 + float(s)

        ordered = [seconds(s) for s, _ in stamps]
        backwards = sum(1 for a, b in zip(ordered, ordered[1:]) if b < a)
        out["regress_small"] = {"reads": len(stamps), "reads_going_back_in_time": backwards,
                                "usable_among_them": sum(1 for _, u in stamps if u)}
    out["leftover_processes_needing_force"] = lab.leftover
    (HERE / "baseline.json").write_text(json.dumps(out, indent=2, default=str) + "\n", encoding="utf-8")
    print(json.dumps(out, indent=2, default=str))


if __name__ == "__main__":
    main()
