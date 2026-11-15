"""Import a recorded OpenAMRobot Gazebo run as Open-MHS events (RFC 0005). Standard library only.

    python -m openmhs.importers.openamr_gazebo RUN_DIR [RUN_DIR ...] --out events.jsonl [--evidence EVIDENCE.jsonl]

RUN_DIR is what ``research/outcome-verifier/ros2-evaluation/gazebo/run_experiment.sh`` wrote, after
``gazebo_outcomes.py`` analysed it: ``intent.jsonl``, ``claims.json``, ``run.txt``, ``versions.txt``,
``conversion.meta.json`` and ``lift_controller_params.yaml``.

What becomes what:

- one **deployment manifest** per run, from the package versions, the physics engine, the lift controller's recorded
  parameters, and the task driver's goal tolerance; a ``deployment.applied`` event at the start of the run;
- each **lift goal** is a task (``task_type`` ``lift``, component ``lift``) with one action keyed by its ROS 2 goal
  UUID; the outcome the action reported (``SUCCEEDED``, ``ABORTED``) is ``outcome.reported``;
- each **base drive** is a task (``drive``, component ``base``); the driver only knows it finished commanding, so its
  reported outcome says so in ``ext``;
- each **verifier verdict** (RFC 0004, from ``claims.json``) is an ``outcome.verified`` event. Lift verdicts carry the
  goal UUID (joined by id); odometry verdicts carry no id (joined by time window, component ``base``);
- the **bag** is an artifact, by reference: its SHA-256 and local path, never its content;
- with ``--evidence``, the bridge's ``lift.action.status`` records become ``action.status`` events (status
  transitions with their placement uncertainty).

Every run is its own simulated robot instance and its own clock: simulation time restarts at zero in each run, so
times from two runs must never be compared. SIMULATION ONLY.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any

TENANT = "openmhs-sim"
SITE = "wsl-gazebo"
STATUS = {"1": "ACCEPTED", "2": "EXECUTING", "3": "CANCELING", "4": "SUCCEEDED", "5": "CANCELED", "6": "ABORTED"}
REPORTED = {4: "succeeded", 5: "canceled", 6: "aborted"}
VERIFIED = {"SATISFIED": "satisfied", "VIOLATED": "violated", "UNKNOWN": "unknown"}
# The task driver reads the simulation clock when it sees a result; its reading may lag the event by its 10 ms poll
# and message delivery. Stated, not measured.
DRIVER_UNCERTAINTY_MS = 50.0


def canonical(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def digest(value: Any) -> str:
    return "sha256:" + hashlib.sha256(canonical(value).encode("utf-8")).hexdigest()


def rfc(t: float) -> str:
    ms = int(round(t * 1000))
    s, rem = divmod(ms, 1000)
    import datetime as dt

    return dt.datetime.fromtimestamp(s, dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S") + f".{rem:03d}Z"


def manifest(run: Path) -> dict[str, Any]:
    versions = (run / "versions.txt").read_text(encoding="utf-8").splitlines()
    engine = re.search(r"^engine (\S+)", (run / "run.txt").read_text(encoding="utf-8"), re.M)
    components: list[dict[str, Any]] = []
    for line in versions:
        parts = line.split()
        if len(parts) == 2 and parts[0] == "openamr-upperbody-sw":
            components.append({"kind": "software", "name": "openamr-upperbody-sw", "vcs": "git", "revision": parts[1]})
        elif len(parts) == 2 and parts[0].startswith("ros-"):
            components.append({"kind": "software", "name": parts[0], "version": parts[1]})
        elif len(parts) == 1 and "microsoft" in parts[0]:
            components.append({"kind": "software", "name": "wsl-kernel", "version": parts[0]})
    if engine:
        components.append({"kind": "config", "name": "gazebo-physics", "engine": engine.group(1)})
    params = run / "lift_controller_params.yaml"
    if params.exists():
        text = params.read_text(encoding="utf-8")
        goal = re.search(r"lift_joint:\s*\n\s*goal:\s*([0-9.eE+-]+)", text)
        goal_time = re.search(r"\bgoal_time:\s*([0-9.eE+-]+)", text)
        components.append({
            "kind": "config", "name": "lift_controller",
            "config_digest": "sha256:" + hashlib.sha256(text.replace("\r", "").encode()).hexdigest(),
            "goal_tolerance_m": float(goal.group(1)) if goal else None,
            "goal_time_s": float(goal_time.group(1)) if goal_time else None,
        })
    driver = json.loads((run / "driver_config.json").read_text(encoding="utf-8")) if (run / "driver_config.json").exists() else {}
    components.append({"kind": "config", "name": "task-driver",
                       "goal_tolerance_m": driver.get("goal_tolerance_m"),
                       "goal_time_tolerance_s": driver.get("goal_time_tolerance_s")})
    engine_name = engine.group(1).split("-")[2] if engine and len(engine.group(1).split("-")) > 2 else "unknown"
    release = driver.get("release") or f"engine-{engine_name}"
    return {"schema": "openmhs.deployment/0.1", "components": components, "labels": {"release": release}}


class Run:
    def __init__(self, run: Path, robot: str):
        self.name = run.name
        self.robot = robot
        self.domain = f"sim:gazebo-openamr/{run.name}"
        self.out: list[dict[str, Any]] = []

    def event(self, eid: str, kind: str, t: float | None, *, uncertainty: float | None = 0.0,
              domain: str | None = None, provenance: str = "imported", **fields: Any) -> dict[str, Any]:
        time: dict[str, Any] = {"clock_domain": domain or self.domain}
        if t is not None:
            time["observed"] = rfc(t)
        if uncertainty is not None:
            time["uncertainty_ms"] = uncertainty
        e: dict[str, Any] = {
            "schema": "openmhs.event/0.1", "event_id": f"{self.name}/{eid}", "type": kind,
            "source": {"id": f"openamr-gazebo/{fields.pop('source', 'run')}", "kind": "import", "boot_id": self.name},
            "time": time, "scope": {"tenant": TENANT, "site": SITE, "robot": self.robot}, "provenance": provenance,
        }
        e.update({k: v for k, v in fields.items() if v is not None})
        self.out.append(e)
        return e


def import_run(run: Path) -> list[dict[str, Any]]:
    m = manifest(run)
    md = digest(m)
    robot = f"openamrobot-sim/{run.name}"
    r = Run(run, robot)
    intent = [json.loads(line) for line in (run / "intent.jsonl").read_text(encoding="utf-8").splitlines() if line.strip()]
    claims = json.loads((run / "claims.json").read_text(encoding="utf-8"))
    meta = json.loads((run / "conversion.meta.json").read_text(encoding="utf-8"))
    start = next(i["sim"] for i in intent if i.get("task") == "start")
    end = next(i["sim"] for i in intent if i.get("task") == "end")

    records: list[dict[str, Any]] = [m]
    r.event("deployment", "deployment.applied", start, deployment={"manifest": md}, source="driver",
            provenance="reported", uncertainty=DRIVER_UNCERTAINTY_MS)

    bag = "sha256:" + meta["bag_sha256"]
    records.append({"schema": "openmhs.artifact/0.1", "tenant": TENANT, "artifact_id": bag,
                    "uri": f"file:///D:/openmhs-tmp/gzeval/runs/{run.name}/bag/bag_0.mcap",
                    "media_type": "application/x-mcap", "robot": robot,
                    "time_range": [rfc(start), rfc(end)], "streams": sorted(meta.get("topics_used", {}))})

    for i in intent:
        kind = i.get("kind")
        tid = i.get("task")
        if kind == "lift":
            subject = {"task_id": f"{run.name}/{tid}", "task_type": "lift", "scenario_id": f"lift/{tid}"}
            attrs = {"component": "lift"}
            r.event(f"{tid}/task.started", "task.started", i["sent_sim"], subject=subject, attributes=attrs,
                    deployment={"manifest": md}, source="driver", provenance="reported",
                    uncertainty=DRIVER_UNCERTAINTY_MS,
                    ext={"task": {"target_m": i["target_m"], "duration_s": i["duration_s"]}})
            if not i.get("accepted"):
                r.event(f"{tid}/task.ended", "task.ended", i["sent_sim"], subject=subject, attributes=attrs,
                        outcome={"reported": {"value": "failed", "source": "ros2.action", "detail_code": "rejected"}},
                        source="driver", provenance="reported", uncertainty=DRIVER_UNCERTAINTY_MS)
                continue
            goal = i["goal_id"]
            action = {**subject, "action_id": f"{run.name}/{tid}/goal", "goal_id": goal}
            r.event(f"{tid}/action.started", "action.started", i["accepted_sim"], subject=action, attributes=attrs,
                    source="driver", provenance="reported", uncertainty=DRIVER_UNCERTAINTY_MS,
                    ext={"ros2": {"action": "/lift_controller/follow_joint_trajectory"}})
            reported = REPORTED.get(i.get("result_status"), "unknown")
            outcome = {"reported": {"value": reported, "source": "ros2.action_result",
                                    "detail_code": str(i.get("result_error_code"))}}
            ros2 = {"status_code": i.get("result_status"), "error_code": i.get("result_error_code"),
                    "error_string": i.get("result_error_string")}
            r.event(f"{tid}/action.ended", "action.ended", i.get("result_sim"), subject=action, attributes=attrs,
                    outcome=outcome, source="driver", provenance="reported", uncertainty=DRIVER_UNCERTAINTY_MS,
                    ext={"ros2": ros2})
            r.event(f"{tid}/task.ended", "task.ended", i.get("result_sim"), subject=subject, attributes=attrs,
                    outcome=outcome, source="driver", provenance="reported", uncertainty=DRIVER_UNCERTAINTY_MS)
        elif kind == "base":
            subject = {"task_id": f"{run.name}/{tid}", "task_type": "drive", "scenario_id": f"drive/{tid}"}
            attrs = {"component": "base"}
            command = {"vx": i["vx"], "wz": i["wz"], "duration_s": i["duration_s"]}
            r.event(f"{tid}/task.started", "task.started", i["start_sim"], subject=subject, attributes=attrs,
                    deployment={"manifest": md}, source="driver", provenance="reported",
                    uncertainty=DRIVER_UNCERTAINTY_MS, ext={"task": command})
            r.event(f"{tid}/task.ended", "task.ended", i["stop_sim"], subject=subject, attributes=attrs,
                    outcome={"reported": {"value": "succeeded", "source": "task-driver",
                                          "detail_code": "commanded"}},
                    source="driver", provenance="reported", uncertainty=DRIVER_UNCERTAINTY_MS,
                    ext={"note": "the driver finished sending the command; whether the base moved is not known to it"})

    for c in claims:
        verdict = VERIFIED.get(c.get("verifier", ""), "unknown")
        verifier = {"label": c.get("label"), "reasons": c.get("reasons", []), "error_m": c.get("error_m")}
        if c["kind"] == "lift":
            r.event(f"{c['id']}/outcome.verified", "outcome.verified", c["t"], uncertainty=50.5,
                    domain=f"{r.domain}/recorder", subject={"goal_id": c["goal"]}, attributes={"component": "lift"},
                    outcome={"verified": {"value": verdict, "contract": "lift-at-target-when-succeeded"}},
                    source="verifier", provenance="observed", ext={"verifier": verifier})
        else:
            r.event(f"{c['id']}/outcome.verified", "outcome.verified", c["t"], attributes={"component": "base"},
                    outcome={"verified": {"value": verdict, "contract": "base-where-odometry-says"}},
                    source="verifier", provenance="observed", ext={"verifier": verifier})

    return records + r.out


def status_events(run: Path, evidence: Path) -> list[dict[str, Any]]:
    """The bridge's status transitions (``ros2_evidence.py``, time placed from ``/clock``) as ``action.status``."""
    import datetime as dt

    r = Run(run, f"openamrobot-sim/{run.name}")
    for line in evidence.read_text(encoding="utf-8").splitlines():
        rec = json.loads(line)
        if rec.get("signal") != "lift.action.status":
            continue
        t = rec.get("time", {})
        observed = None
        if "source" in t:
            observed = dt.datetime.strptime(t["source"], "%Y-%m-%dT%H:%M:%S.%fZ").replace(
                tzinfo=dt.timezone.utc).timestamp()
        r.event(rec["id"].replace("/", "_"), "action.status", observed, uncertainty=t.get("uncertainty_ms"),
                domain=f"{r.domain}/recorder", subject={"goal_id": rec["refs"]["attempt"]},
                attributes={"component": "lift"}, source="bag",
                ext={"ros2": {"status": rec["value"]}})
    return r.out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("runs", nargs="+", type=Path)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--evidence", type=Path, action="append", default=[],
                    help="bridge evidence for a run, as RUN_NAME=PATH; repeatable")
    args = ap.parse_args()
    evidence = dict(e.as_posix().split("=", 1) for e in args.evidence)
    lines: list[str] = []
    for run in args.runs:
        records = import_run(run)
        if run.name in evidence:
            records += status_events(run, Path(evidence[run.name]))
        lines.extend(canonical(x) for x in records)
    args.out.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print(f"{len(lines)} records from {len(args.runs)} runs -> {args.out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
