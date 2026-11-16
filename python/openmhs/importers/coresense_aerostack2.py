"""Import real-flight rosbag2 recordings of an Aerostack2 drone swarm as Open-MHS events (RFC 0005).

    python -m openmhs.importers.coresense_aerostack2 BAG_DIR [BAG_DIR ...] --out events.jsonl

Standard library only. Reads rosbag2 **SQLite** bags (``*.db3`` plus ``metadata.yaml``) read-only, never touching a
robot. Built for the CoreSense Inspection Testbed ROSbag Dataset (three Crazyflie drones flown under OptiTrack motion
capture; Zenodo 10.5281/zenodo.23243295, CC BY 4.0), whose bags are named ``rosbag2_real_paths_2_OK_<condition>``.

What becomes what:

- one **mission task per drone per bag** (``task_type`` ``coverage_mission``), from the drone's first takeoff to the
  terminal status of its landing goal;
- every ``Takeoff``, ``GoTo``, ``Land`` and ``Auction`` **goal** is an action keyed by its ROS 2 goal UUID, read from
  ``/droneN/<Behavior>/_action/status`` (``action_msgs/GoalStatusArray``); its terminal status is
  ``outcome.reported``. ``CollisionAvoidance`` goals run continuously and are left out;
- the swarm's own knowledge base (``/droneN/kb/add_fact``, ``std_msgs/String``): ``<drone> droneStatus failed`` becomes a
  ``fault.raised`` event and ``<drone> auctionStatus started|completed`` a ``marker`` (only from that drone's own
  stream, so one fact is one event);
- a mission's reported outcome is ``failed`` when the swarm reported its drone failed, otherwise the terminal status of
  its landing goal (``SUCCEEDED`` → succeeded, ``CANCELED`` → canceled, none → unknown);
- one **deployment manifest** shared by all bags (the software is the same; the dataset does not publish a revision, and
  the manifest says so), and the bag file as an **artifact** by reference (SHA-256, never copied).

Honest limits, repeated in every event's ``ext.coresense``: failures in these flights were **induced by the
experimenters** ("induced failures and auction-based waypoint re-assignment"), not observed organically. Times are the
recorder's receipt times on one host clock (assumed single; the dataset does not say). The dataset names no software
revision.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sqlite3
import sys
from pathlib import Path
from typing import Any

from . import cdr

TENANT = "coresense-public-data"
SITE = "coresense-testbed"
CLOCK = "coresense-recorder"
ACTION_BEHAVIORS = ("Takeoff", "GoTo", "Land", "Auction")
TERMINAL = {"SUCCEEDED": "succeeded", "CANCELED": "canceled", "ABORTED": "aborted"}
STARTED = ("ACCEPTED", "EXECUTING", "CANCELING")
REPOSITORY = "https://github.com/CoreSenseEU/aerostack2"


def canonical(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def digest(value: Any) -> str:
    return "sha256:" + hashlib.sha256(canonical(value).encode("utf-8")).hexdigest()


def rfc(ns: int) -> str:
    import datetime as dt

    ms = ns // 1_000_000
    s, rem = divmod(ms, 1000)
    return dt.datetime.fromtimestamp(s, dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S") + f".{rem:03d}Z"


def file_sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return "sha256:" + h.hexdigest()


def condition_of(bag: Path) -> str:
    m = re.search(r"_OK_(.+)$", bag.name)
    return m.group(1) if m else "unknown"


def manifest(bag: Path) -> dict[str, Any]:
    """What ran, as far as the dataset says. The software revision is not published, and that is recorded."""
    paths = re.search(r"real_(paths_\d+)", bag.name)
    return {
        "schema": "openmhs.deployment/0.1",
        "components": [
            {"kind": "software", "name": "aerostack2", "vcs": "git", "repository": REPOSITORY,
             "revision": "not published by the dataset"},
            {"kind": "hardware", "name": "crazyflie-swarm", "drones": 3, "localisation": "OptiTrack motion capture"},
            {"kind": "config", "name": "mission-paths", "paths": paths.group(1) if paths else "unknown"},
        ],
        "labels": {"release": "aerostack2-unversioned"},
    }


class Bag:
    def __init__(self, path: Path):
        self.path = path
        self.db = next(path.glob("*.db3"))
        self.con = sqlite3.connect(f"file:{self.db}?mode=ro", uri=True)
        self.topics = {name: (tid, typ) for tid, name, typ in self.con.execute("select id, name, type from topics")}
        row = self.con.execute("select min(timestamp), max(timestamp) from messages").fetchone()
        self.start_ns, self.end_ns = row

    def messages(self, topic: str):
        if topic not in self.topics:
            return
        tid = self.topics[topic][0]
        yield from self.con.execute(
            "select timestamp, data from messages where topic_id = ? order by timestamp, id", (tid,)
        )

    def drones(self) -> list[str]:
        return sorted({m.group(1) for n in self.topics if (m := re.match(r"/(drone\d+)/", n))})


def goals(bag: Bag, drone: str, behavior: str) -> list[dict[str, Any]]:
    """Each goal on one behavior: when it was first seen, and the first terminal status it reached."""
    seen: dict[str, dict[str, Any]] = {}
    for ts, data in bag.messages(f"/{drone}/{behavior}Behavior/_action/status"):
        for g in cdr.decode_goal_status_array(bytes(data)):
            s = seen.setdefault(g.goal_id, {"goal_id": g.goal_id, "start": None, "end": None, "status": None})
            if s["start"] is None and g.status in STARTED + tuple(TERMINAL):
                s["start"] = ts
            if s["end"] is None and g.status in TERMINAL:
                s["end"], s["status"] = ts, g.status
    return sorted(seen.values(), key=lambda s: (s["start"] or 0, s["goal_id"]))


def facts(bag: Bag, drone: str) -> list[tuple[int, str]]:
    """Facts the drone's own knowledge base added: (time, 'subject predicate object')."""
    out = []
    for ts, data in bag.messages(f"/{drone}/kb/add_fact"):
        for line in cdr.decode_string(bytes(data)).splitlines():
            if line.strip():
                out.append((ts, line.strip()))
    return out


def import_bag(path: Path) -> list[dict[str, Any]]:
    """Every record for one bag directory. The bag is opened read-only and closed before returning."""
    bag = Bag(path)
    try:
        return _import(bag, path)
    finally:
        bag.con.close()


def _import(bag: Bag, path: Path) -> list[dict[str, Any]]:
    cond = condition_of(path)
    name = path.name
    m = manifest(path)
    md = digest(m)
    ext = {"coresense": {"bag": name, "condition": cond, "failures_induced_by_experimenters": cond != "no_failure",
                         "clock": "recorder receipt time; one host clock assumed"}}
    out: list[dict[str, Any]] = [m]
    out.append({"schema": "openmhs.artifact/0.1", "tenant": TENANT, "artifact_id": file_sha256(bag.db),
                "uri": bag.db.resolve().as_uri(), "media_type": "application/x-rosbag2-sqlite3",
                "bytes": bag.db.stat().st_size, "time_range": [rfc(bag.start_ns), rfc(bag.end_ns)],
                "dataset": "10.5281/zenodo.23243295"})

    def event(eid: str, kind: str, ns: int, drone: str, **fields: Any) -> None:
        e: dict[str, Any] = {
            "schema": "openmhs.event/0.1", "event_id": f"{name}/{eid}", "type": kind,
            "source": {"id": f"coresense-import/{drone}", "kind": "import", "boot_id": name},
            "time": {"observed": rfc(ns), "clock_domain": CLOCK},
            "scope": {"tenant": TENANT, "site": SITE, "robot": f"crazyflie/{drone}"},
            "provenance": fields.pop("provenance", "imported"), "ext": ext,
        }
        e.update({k: v for k, v in fields.items() if v is not None})
        out.append(e)

    paths = re.search(r"paths_\d+", name)
    for drone in bag.drones():
        task = f"{name}/{drone}/mission"
        subject = {"task_id": task, "task_type": "coverage_mission"}
        if paths:
            subject["scenario_id"] = f"coverage/{paths.group(0)}"
        attrs = {"component": "mission"}
        all_goals = {b: goals(bag, drone, b) for b in ACTION_BEHAVIORS}
        starts = [g["start"] for g in all_goals["Takeoff"] if g["start"]] or [g["start"] for b in all_goals.values() for g in b if g["start"]]
        if not starts:
            continue
        task_start = min(starts)
        landing = [g for g in all_goals["Land"] if g["end"]]
        ends = [g["end"] for b in all_goals.values() for g in b if g["end"]]
        task_end = max(g["end"] for g in landing) if landing else (max(ends) if ends else bag.end_ns)

        kb = facts(bag, drone)
        failed = next(((ts, ln) for ts, ln in kb if ln == f"{drone} droneStatus failed"), None)

        event(f"{drone}/deployment", "deployment.applied", bag.start_ns, drone, deployment={"manifest": md})
        event(f"{drone}/task.started", "task.started", task_start, drone, subject=subject, attributes=attrs,
              deployment={"manifest": md})

        for behavior, items in all_goals.items():
            for g in items:
                if not g["start"]:
                    continue
                gid = g["goal_id"]
                key = f"{drone}/{behavior}/{gid.replace('-', '')[:12]}"
                action = {**subject, "action_id": key, "goal_id": gid}
                a_attrs = {"component": behavior.lower()}
                event(f"{key}/start", "action.started", g["start"], drone, subject=action, attributes=a_attrs,
                      ext={**ext, "ros2": {"action": f"/{drone}/{behavior}Behavior"}})
                if g["end"]:
                    event(f"{key}/end", "action.ended", g["end"], drone, subject=action, attributes=a_attrs,
                          outcome={"reported": {"value": TERMINAL[g["status"]], "source": "ros2.action_status"}},
                          ext={**ext, "ros2": {"action": f"/{drone}/{behavior}Behavior", "status": g["status"]}})

        for ts, line in kb:
            if line == f"{drone} auctionStatus started" or line == f"{drone} auctionStatus completed":
                event(f"{drone}/auction/{ts}", "marker", ts, drone, subject=subject,
                      attributes={"component": "auction", "category": line.split()[-1]}, ext=ext)
        if failed:
            event(f"{drone}/fault/droneStatus", "fault.raised", failed[0], drone, subject=subject,
                  attributes={"component": "mission", "severity": "error", "category": "hardware.drone",
                              "code": "drone_status_failed"},
                  provenance="reported", ext={**ext, "kb_fact": failed[1]})

        if failed:
            reported = {"value": "failed", "source": "aerostack2.kb", "detail_code": "drone_status_failed"}
        elif landing:
            status = landing[-1]["status"]
            reported = {"value": TERMINAL[status], "source": "ros2.action_status", "detail_code": f"land_{status.lower()}"}
        else:
            reported = {"value": "unknown", "source": "importer", "detail_code": "no_terminal_land"}
        event(f"{drone}/task.ended", "task.ended", task_end, drone, subject=subject, attributes=attrs,
              outcome={"reported": reported})
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("bags", nargs="+", type=Path)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()
    lines: list[str] = []
    for bag in args.bags:
        lines.extend(canonical(r) for r in import_bag(bag))
    args.out.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print(f"{len(lines)} records from {len(args.bags)} bags -> {args.out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
