"""Run the evaluation in EVALUATION.md and write what happened. Nothing here is estimated.

    python3 examples/ros2-readonly-robot/run_evaluation.py --cycles 20 --out /tmp/robot-eval

Writes ``summary.json`` (metrics and checks) and ``observations.jsonl`` (every timed cycle). Exits non-zero
if any failure condition in EVALUATION.md happened.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import platform
import signal
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from agent import (  # noqa: E402
    CURRENT, DENIED, ReadOnlyAgent, answer_robot, check_grounding)
from robot_lab import (  # noqa: E402
    AGENT_READS, CONTROL_SURFACES, DEVICE, REPO, UNGRANTED, RobotLab, command_attempt)

#: What the fixture publishes when no fault is active.
EXPECTED: dict[str, Any] = {
    "joint.shoulder.position": 0.5, "joint.elbow.position": -1.2, "joint.wrist.position": 0.3,
    "base.frame": "odom", "base.child_frame": "base_link", "base.position.x": 1.25, "base.position.y": -0.5,
    "base.orientation.z": 0.0, "base.orientation.w": 1.0,
    "battery.voltage": 24.8, "battery.charge": 0.72, "battery.temperature": 31.5,
    "diagnostics.driver.level": 0,
}
POLL_S = 0.02


def same(value: Any, expected: Any) -> bool:
    if isinstance(expected, float):
        return isinstance(value, (int, float)) and math.isclose(value, expected, abs_tol=1e-4)
    return value == expected


def stats(values: list[float]) -> dict[str, Any]:
    if not values:
        return {"n": 0}
    return {"n": len(values), "min": round(min(values), 1), "median": round(statistics.median(values), 1),
            "max": round(max(values), 1)}


class ClockWatch:
    """Steps of the wall clock against the monotonic clock while the run is going. Message stamps come
    from the wall clock, so a step here explains an odd reading; recording them keeps that visible."""

    def __init__(self) -> None:
        self.steps_ms: list[float] = []  # signed; only changes above 20 ms between samples
        self.started = time.monotonic()
        self._stop = threading.Event()

    def __enter__(self) -> ClockWatch:
        threading.Thread(target=self._run, daemon=True).start()
        return self

    def _run(self) -> None:
        last = time.time() - time.monotonic()
        while not self._stop.wait(0.1):
            now = time.time() - time.monotonic()
            if abs(now - last) > 0.02:
                self.steps_ms.append(round((now - last) * 1000, 1))
            last = now

    def __exit__(self, *exc: object) -> None:
        self._stop.set()


class Run:
    def __init__(self, out: Path, cycles: int) -> None:
        self.out = out
        self.cycles = cycles
        self.summary: dict[str, Any] = {"metrics": {}, "scenarios": {}, "failures": []}
        out.mkdir(parents=True, exist_ok=True)
        self.raw = open(out / "observations.jsonl", "w", encoding="utf-8")

    def record(self, scenario: str, **data: Any) -> None:
        self.raw.write(json.dumps({"scenario": scenario, **data}, default=str) + "\n")
        self.raw.flush()

    def fail(self, condition: int, text: str) -> None:
        self.summary["failures"].append({"condition": condition, "what": text})
        print(f"  FAILURE (condition {condition}): {text}", flush=True)

    def say(self, text: str) -> None:
        print(text, flush=True)


def poll_until_failure(client: Any, capability: str, t0: float, timeout: float = 5.0) -> dict[str, Any]:
    """Poll a capability after ``t0``. Returns when the first read failed and the last usable one."""
    last_usable = None
    while time.monotonic() - t0 < timeout:
        try:
            r = client.read(DEVICE, capability)
            if r.usable:
                last_usable = time.monotonic()
        except Exception as e:  # noqa: BLE001 - a failed read is what is being waited for
            return {"first_failure_ms": (time.monotonic() - t0) * 1000,
                    "last_usable_ms": None if last_usable is None else (last_usable - t0) * 1000,
                    "error": getattr(e, "code", type(e).__name__)}
        time.sleep(POLL_S)
    return {"first_failure_ms": None, "last_usable_ms": None, "error": None}


def keeps_failing(client: Any, capability: str, seconds: float = 0.3) -> int:
    """Reads that returned a value during ``seconds`` when none should. Used right after a first failure."""
    values = 0
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        try:
            client.read(DEVICE, capability)
            values += 1
        except Exception:  # noqa: BLE001, S110 - failing is the expected outcome here
            pass
        time.sleep(POLL_S)
    return values


def settle(lab: RobotLab, client: Any, capability: str, expected: Any, seconds: float = 5.0) -> tuple[dict, bool, int]:
    """Read until ``expected`` holds, up to ``seconds``. The fault is held throughout, so this only allows
    for a wall clock step in the environment (the run records those)."""
    deadline = time.monotonic() + seconds
    reads = 0
    while True:
        got = lab.read_plain(client, capability)
        reads += 1
        if expected(got):
            return got, True, reads
        if time.monotonic() > deadline:
            return got, False, reads
        time.sleep(0.1)


def scenario_a_normal(run: Run, lab: RobotLab, agent: ReadOnlyAgent) -> None:
    run.say("A. normal operation")
    attempts = successes = usable = wrong = wrong_source = 0
    ages = []
    failed: dict[str, int] = {}
    for _ in range(20):
        for capability in AGENT_READS:
            attempts += 1
            o = agent.read(capability)
            if o.value is None:
                failed[str(o.error)] = failed.get(str(o.error), 0) + 1
                continue
            successes += 1
            ages.append(o.age_ms)
            usable += o.state == CURRENT
            wrong += not same(o.value, EXPECTED[capability])
            wrong_source += o.source != "simulator"
    run.summary["scenarios"]["A"] = {"attempts": attempts, "successes": successes, "usable": usable,
                                     "wrong_value": wrong, "wrong_source": wrong_source, "failed_reads": failed}
    run.summary["metrics"]["read_success_rate"] = f"{successes}/{attempts}"
    run.summary["metrics"]["observation_age_ms"] = stats(ages)
    if wrong or wrong_source:
        run.fail(5, f"scenario A: {wrong} wrong values, {wrong_source} wrong sources")


def scenario_b_topic_stop(run: Run, lab: RobotLab) -> None:
    run.say("B. one topic stops")
    client = lab.agent_client()
    stale, last_usable, recovery = [], [], []
    leaked = neighbours_lost = 0
    errors: dict[str, int] = {}
    for i in range(run.cycles):
        lab.wait_readable("base.position.x")
        t0 = lab.sim.send("stop", topic="/odom")
        p = poll_until_failure(client, "base.position.x", t0)
        if p["first_failure_ms"] is None:
            run.fail(3, f"cycle {i}: no failure after the topic stopped")
            lab.sim.send("start", topic="/odom")
            continue
        errors[p["error"]] = errors.get(p["error"], 0) + 1
        # A wall clock step can make the first failure "stamped older" before the silence rule fires. That
        # cycle says nothing about the silence rule, so it is counted under errors and left out of the times.
        if p["error"] == "adapter.disconnected":
            stale.append(p["first_failure_ms"])
            if p["last_usable_ms"] is not None:
                last_usable.append(p["last_usable_ms"])
        leaked += keeps_failing(client, "base.position.x")
        try:
            neighbours_lost += not client.read(DEVICE, "joint.elbow.position").usable
        except Exception:  # noqa: BLE001
            neighbours_lost += 1
        t1 = lab.sim.send("start", topic="/odom")
        recovery.append((lab.wait_readable("base.position.x") - t1) * 1000)
        run.record("B", cycle=i, first_failure_ms=p["first_failure_ms"], error=p["error"],
                   last_usable_ms=p["last_usable_ms"], recovery_ms=recovery[-1])
    run.summary["scenarios"]["B"] = {"cycles": run.cycles, "errors": errors, "values_after_first_failure": leaked,
                                     "other_topic_affected": neighbours_lost}
    run.summary["metrics"]["stale_detection_ms"] = stats(stale)
    run.summary["metrics"]["last_usable_after_stop_ms"] = stats(last_usable)
    run.summary["metrics"]["topic_recovery_ms"] = stats(recovery)
    if leaked:
        run.fail(3, f"scenario B: {leaked} values returned after the first failure")


def scenario_cd_kill_restart(run: Run, lab: RobotLab) -> None:
    run.say("C, D. the node dies and returns")
    client = lab.agent_client()
    assert lab.operator is not None
    disconnect, availability, from_ready, from_start = [], [], [], []
    leaked = wrong = 0
    errors: dict[str, int] = {}
    silent_capabilities = 0
    for i in range(run.cycles):
        lab.wait_readable("joint.elbow.position")
        t0 = lab.sim.kill()
        offline_at = None
        p = poll_until_failure(client, "joint.elbow.position", t0)
        if p["first_failure_ms"] is None:
            run.fail(3, f"cycle {i}: no failure after the node died")
        else:
            errors[p["error"]] = errors.get(p["error"], 0) + 1
            if p["error"] == "adapter.disconnected":
                disconnect.append(p["first_failure_ms"])
        leaked += keeps_failing(client, "joint.elbow.position")
        # Every capability, once each topic has had time to go silent (the longest is 2000 ms).
        while time.monotonic() - t0 < 2.4:
            status = lab.operator.device_status(DEVICE)[0]
            if offline_at is None and "ONLINE" not in str(status.availability):
                offline_at = time.monotonic()
            time.sleep(0.05)
        for capability in AGENT_READS:
            try:
                client.read(DEVICE, capability)
                silent_capabilities += 1
            except Exception:  # noqa: BLE001, S110
                pass
        if offline_at is not None:
            availability.append((offline_at - t0) * 1000)
        start = time.monotonic()
        lab.sim.start()
        ready = lab.sim.last_ready
        usable_at = lab.wait_readable("joint.elbow.position")
        from_ready.append((usable_at - ready) * 1000)
        from_start.append((usable_at - start) * 1000)
        lab.wait_readable("diagnostics.driver.level")
        lab.wait_readable("battery.voltage")
        lab.wait_readable("base.position.x")
        for c in ("joint.elbow.position", "base.position.x"):
            value = lab.read_plain(client, c).get("value")
            wrong += value is not None and not same(value, EXPECTED[c])
        run.record("CD", cycle=i, first_failure_ms=p["first_failure_ms"], error=p["error"],
                   offline_ms=availability[-1] if offline_at else None,
                   from_ready_ms=from_ready[-1], from_start_ms=from_start[-1])
    run.summary["scenarios"]["C"] = {"cycles": run.cycles, "errors": errors, "values_after_first_failure": leaked,
                                     "values_from_any_capability_when_all_silent": silent_capabilities}
    run.summary["scenarios"]["D"] = {"cycles": run.cycles, "wrong_values_after_reconnect": wrong,
                                     "gateway_restarted": False}
    run.summary["metrics"]["disconnect_detection_ms"] = stats(disconnect)
    run.summary["metrics"]["availability_change_ms"] = stats(availability)
    run.summary["metrics"]["reconnect_from_ready_ms"] = stats(from_ready)
    run.summary["metrics"]["reconnect_from_process_start_ms"] = stats(from_start)
    if leaked or silent_capabilities:
        run.fail(3, f"scenario C: {leaked} leaked, {silent_capabilities} values while all silent")
    if wrong:
        run.fail(5, f"scenario D: {wrong} wrong values after reconnect")


def scenario_e_unauthorized(run: Run, lab: RobotLab, agent: ReadOnlyAgent) -> None:
    run.say("E. ungranted reads")
    results = {}
    for capability in (UNGRANTED, "tf.base_link.x", "cmd_vel.linear.x"):
        o = agent.read(capability)
        results[capability] = {"state": o.state, "error": o.error, "value": o.value}
    other = ReadOnlyAgent(agent.tools, device="sim.psu-01").read("output.voltage")
    results["another device"] = {"state": other.state, "error": other.error, "value": other.value}
    run.summary["scenarios"]["E"] = results
    run.summary["metrics"]["denied_reads"] = sum(1 for r in results.values() if r["state"] == DENIED)
    if any(r["state"] != DENIED or r["value"] is not None for r in results.values()):
        run.fail(9, f"scenario E: a read was not denied: {results}")


def scenario_f_control(run: Run, lab: RobotLab) -> None:
    run.say("F. attempted control")
    assert lab.operator is not None
    attempts = []
    credentials = {"scoped agent": lab.agent_client(),
                   "unscoped agent (policy only)": lab.client_for(lab.unscoped_agent_token()),
                   "operator": lab.operator}
    quantity = {"value": 0.0, "unit": "rad"}
    targets = [
        ("joint.elbow.position", "write", quantity), ("joint.elbow.position", "invoke", None),
        ("base.cmd_vel", "write", {"value": 1.0, "unit": "m/s"}), ("joint.elbow.move", "invoke", None),
    ]
    for who, client in credentials.items():
        for capability, operation, value in targets:
            attempts.append({"credential": who, **command_attempt(client, capability, operation, value)})
    run.summary["scenarios"]["F"] = {"attempts": attempts}
    refused = [a for a in attempts if a["refused"]]
    run.summary["metrics"]["denied_commands"] = f"{len(refused)}/{len(attempts)}"
    for a in attempts:
        if not a["refused"]:
            run.fail(9, f"not refused: {a}")
    # A permissive policy cannot create a command the descriptor does not declare. This is the structural
    # claim: the operator grants itself everything, and the capability still has no write.
    lab.operator.add_grant({"id": "operator-grants-itself-everything", "principal": "operator:*", "device": "*",
                            "capability": "*", "operations": ["read", "write", "invoke"], "effect": "allow"})
    self_granted = [command_attempt(lab.operator, "joint.elbow.position", "write", quantity),
                    command_attempt(lab.operator, "joint.elbow.position", "invoke")]
    run.summary["scenarios"]["F"]["operator_with_a_grant_for_everything"] = self_granted
    for a in self_granted:
        if not a["refused"] or a.get("dispatched"):
            run.fail(9, f"a self-granted operator was not refused: {a}")
    # Plans are another way in.
    with lab.mcp() as tools:
        plan = tools.call("submit_plan", {"plan": {"description": "an attempt", "steps": [
            {"id": "s1", "request": {"target": {"device": DEVICE, "capability": "joint.elbow.position"},
                                     "operation": "write", "value": quantity}}]}})
    run.summary["scenarios"]["F"]["plan"] = {"is_error": plan.is_error, "text": (plan.text or "")[:200],
                                             "structured": str(plan.structured)[:300]}
    graph = lab.ros("graph", "openmhs_observer")
    run.summary["scenarios"]["F"]["graph_of_adapter_node"] = graph
    extras = (set(graph.get("publishers", [])) - {"/parameter_events"}) | set(graph.get("services", [])) | set(
        graph.get("service_clients", [])) | set(graph.get("action_clients", [])) | set(graph.get("action_servers", []))
    if not graph.get("found") or extras:
        run.fail(1, f"the adapter's node has more than it should: {sorted(extras)}, found={graph.get('found')}")
    run.summary["metrics"]["commands_reaching_control_surfaces"] = len(lab.sim.tripwires)
    if lab.sim.tripwires:
        run.fail(1, f"tripwires hit: {lab.sim.tripwires}")


def scenario_g_invalid(run: Run, lab: RobotLab) -> None:
    run.say("G. invalid data")
    client = lab.agent_client()
    cases = [
        ("nan_joint", "joint.elbow.position", "adapter.device_failure", None),
        ("inf_odom", "base.position.x", "adapter.device_failure", None),
        ("nan_battery_temperature", "battery.temperature", "adapter.device_failure", None),
        ("range_joint", "joint.elbow.position", "adapter.malformed_response", None),
        ("range_battery", "battery.charge", "adapter.malformed_response", None),
        ("empty_positions", "joint.elbow.position", "adapter.malformed_response", None),
        ("mismatched_arrays", "joint.shoulder.position", "adapter.malformed_response", None),
        ("missing_joint", "joint.elbow.position", "adapter.malformed_response", None),
        ("no_diag_status", "diagnostics.driver.level", "adapter.malformed_response", None),
        ("reordered_joints", "joint.shoulder.position", None, 0.5),
        ("diag_error", "diagnostics.driver.level", None, 2),
    ]
    out = {}
    for fault, capability, want_error, want_value in cases:
        lab.wait_readable("joint.elbow.position")
        lab.sim.send("fault", name=fault)
        time.sleep(0.8)  # the slowest of these topics publishes every 500 ms

        def expected(got: dict[str, Any]) -> bool:
            if "stamped older" in got.get("message", ""):  # a wall clock step, not this fault
                return False
            if want_error is not None:
                return got.get("error") == want_error and "value" not in got
            return "error" not in got and same(got.get("value"), want_value)

        got, ok, reads = settle(lab, client, capability, expected)
        lab.sim.send("clear")
        out[fault] = {"capability": capability, "got": got, "as_expected": ok, "reads_until_expected": reads}
        if not ok:
            run.fail(4, f"{fault}: {got}, expected error {want_error} value {want_value}")
        time.sleep(0.8)
    run.summary["scenarios"]["G"] = out


def scenario_h_timestamps(run: Run, lab: RobotLab) -> None:
    run.say("H. timestamps")
    client = lab.agent_client()
    capability = "joint.elbow.position"
    out: dict[str, Any] = {}

    lab.wait_readable(capability)
    t0 = lab.sim.send("fault", name="late_stamp")
    p = poll_until_failure(client, capability, t0)
    during = lab.read_plain(client, capability)
    t1 = lab.sim.send("clear")
    recovery = (lab.wait_readable(capability) - t1) * 1000
    ok = p["error"] == "adapter.malformed_response" and "value" not in during
    out["late_stamp"] = {"failed_after_ms": p["first_failure_ms"], "last_usable_ms": p["last_usable_ms"],
                         "error": p["error"], "during": during, "recovered_after_ms": round(recovery, 1),
                         "as_expected": ok}
    if not ok:
        run.fail(4, f"late_stamp: {out['late_stamp']}")

    lab.wait_readable(capability)
    lab.sim.send("fault", name="future_stamp")
    time.sleep(0.3)
    got, ok, _ = settle(lab, client, capability, lambda g: g.get("usable") is False and "error" not in g)
    t1 = lab.sim.send("clear")
    recovery = (lab.wait_readable(capability) - t1) * 1000
    out["future_stamp"] = {"got": got, "as_expected": ok, "usable_again_after_ms": round(recovery, 1)}
    if not ok:
        run.fail(4, f"future_stamp: {got}")

    lab.wait_readable(capability)
    lab.sim.send("fault", name="zero_stamp")
    time.sleep(0.3)
    while_publishing = lab.read_plain(client, capability)
    t0 = lab.sim.send("stop", topic="/joint_states")
    time.sleep(0.3)
    while_stopped = lab.read_plain(client, capability)
    p = poll_until_failure(client, capability, t0)
    lab.sim.send("start", topic="/joint_states")
    lab.sim.send("clear")
    # The age must grow while nothing arrives: the stamp is the arrival, not the read.
    ok = while_publishing.get("usable") is True and while_stopped.get("age_ms", 0) >= 250 \
        and p["first_failure_ms"] is not None
    out["zero_stamp"] = {"while_publishing": while_publishing, "after_300_ms_of_silence": while_stopped,
                         "failed_after_ms": p["first_failure_ms"], "as_expected": ok}
    if not ok:
        run.fail(4, f"zero_stamp: {out['zero_stamp']}")

    lab.wait_readable(capability)
    lab.sim.send("fault", name="regress_small")
    time.sleep(0.3)
    times, usable, no_value = [], 0, 0
    for _ in range(200):
        got = lab.read_plain(client, capability)
        if "seconds" in got:
            times.append(got["seconds"])
            usable += bool(got.get("usable"))
        else:
            no_value += 1  # a wall clock step; a refusal is not a wrong answer
        time.sleep(0.01)
    lab.sim.send("clear")
    back = sum(1 for a, b in zip(times, times[1:]) if b < a)
    # A wall clock step back moves stamps themselves back, which this check cannot tell from the fault
    # unless the run saw one: such a read is reported, and only counts against the rule if no step happened.
    out["regress_small"] = {"reads_with_value": len(times), "usable": usable, "reads_with_no_value": no_value,
                            "reads_going_back_in_time": back, "as_expected": back == 0}
    if back:
        run.fail(4, f"regress_small: {out['regress_small']}")

    lab.wait_readable(capability)
    t0 = lab.sim.send("fault", name="regress_step")
    p = poll_until_failure(client, capability, t0)
    during = lab.read_plain(client, capability)
    t1 = lab.sim.send("clear")
    recovery = (lab.wait_readable(capability) - t1) * 1000
    ok = p["error"] == "adapter.malformed_response" and "value" not in during
    out["regress_step"] = {"failed_after_ms": p["first_failure_ms"], "error": p["error"], "during": during,
                           "recovered_after_ms": round(recovery, 1), "as_expected": ok}
    if not ok:
        run.fail(4, f"regress_step: {out['regress_step']}")
    run.summary["scenarios"]["H"] = out


def scenario_j_grounded(run: Run, lab: RobotLab, agent: ReadOnlyAgent) -> None:
    run.say("J. grounded answers")
    lab.wait_readable("joint.elbow.position")
    normal = answer_robot(agent.observe_robot())
    lab.sim.send("stop", topic="/odom")
    time.sleep(0.8)
    degraded = answer_robot(agent.observe_robot())
    lab.sim.send("start", topic="/odom")
    lab.wait_readable("base.position.x")
    problems = check_grounding(normal) + check_grounding(degraded)
    (run.out / "answer_normal.txt").write_text(normal.render() + "\n", encoding="utf-8")
    (run.out / "answer_topic_stopped.txt").write_text(degraded.render() + "\n", encoding="utf-8")
    run.summary["scenarios"]["J"] = {"grounding_problems": problems, "statements_normal": len(normal.lines),
                                     "statements_topic_stopped": len(degraded.lines)}
    if problems:
        run.fail(7, f"grounding: {problems}")


def positive_control(run: Run, lab: RobotLab) -> None:
    run.say("F. positive control: a separate ROS node pokes each control surface")
    before = len(lab.sim.tripwires)
    reached = lab.ros("poke")["reached"]
    time.sleep(0.5)
    counted = sorted(set(lab.sim.tripwires[before:]))
    run.summary["scenarios"]["F"]["positive_control"] = {"reached": reached, "counted": counted}
    if set(counted) != set(CONTROL_SURFACES):
        run.fail(2, f"positive control counted only {counted}")


def scenario_i_audit(run: Run, lab: RobotLab) -> None:
    run.say("I. audit")
    assert lab.operator is not None
    verify = lab.cli("audit", "verify")
    # The export, not the audit tail: the tail returns a bounded window and this log is longer than that.
    with tempfile.TemporaryDirectory(prefix="openmhs-evidence-") as d:
        export = lab.cli("audit", "export", "--out", f"{d}/evidence.json", "--checkpoint-out", f"{d}/checkpoint.json")
        offline = lab.cli("--json", "audit", "verify-file", f"{d}/evidence.json", "--checkpoint", f"{d}/checkpoint.json")
        entries = json.loads(Path(f"{d}/evidence.json").read_text(encoding="utf-8"))["audit"]
    for e in entries:
        e["data"] = json.loads(e["data"])  # kept as text in the bundle, because the chain covers those bytes
    kinds: dict[str, int] = {}
    for e in entries:
        kinds[e["kind"]] = kinds.get(e["kind"], 0) + 1
    failed_reads = sum(1 for e in entries if e["kind"] == "read" and "error" in e["data"])
    denied_reads = [e["data"] for e in entries if e["kind"] == "read.denied"]
    denied_commands = [e for e in entries if e["kind"] == "command.transition" and "DENIED" in json.dumps(e["data"])]
    run.summary["scenarios"]["I"] = {
        "records_by_kind": kinds, "reads_recorded_as_failed": failed_reads,
        "denied_read_records": len(denied_reads),
        "denied_read_capabilities": sorted({str(d.get("capability")) for d in denied_reads}),
        "denied_command_records": len(denied_commands),
        "audit_verify": {"exit": verify.returncode, "output": verify.stdout.strip()},
        "export_exit": export.returncode,
        "verify_file": {"exit": offline.returncode, "output": offline.stdout.strip()[:400]},
    }
    run.summary["metrics"]["audit_verification"] = verify.stdout.strip()
    if verify.returncode or offline.returncode or export.returncode:
        run.fail(6, f"audit verification failed: {verify.stdout} {offline.stdout} {offline.stderr}")
    if UNGRANTED not in {str(d.get("capability")) for d in denied_reads}:
        run.fail(6, "the ungranted read has no audit record")


def late_from_the_first_message(run: Run) -> None:
    """Stamps old from the start, so no newer message exists to compare them with. The adapter accepts them
    and the gateway's freshness check is what must catch them."""
    run.say("H. stamps 5 s old from the first message")
    with RobotLab(fault_from_start="late_stamp") as lab:
        client = lab.agent_client()
        deadline = time.monotonic() + 20
        got: dict[str, Any] = {}
        while time.monotonic() < deadline:
            got = lab.read_plain(client, "joint.elbow.position")
            if "value" in got:
                break
            time.sleep(0.05)
    ok = got.get("usable") is False and got.get("age_ms", 0) >= 4000
    run.summary["scenarios"]["H"]["late_from_the_first_message"] = {"got": got, "as_expected": ok}
    if not ok:
        run.fail(4, f"late from the first message: {got}")
    if lab.leftover:
        run.fail(8, f"processes needed force: {lab.leftover}")


def scenario_processes(run: Run) -> None:
    run.say("Process failure: adapter, then gateway")
    out: dict[str, Any] = {}
    with RobotLab() as lab:
        client = lab.agent_client()
        lab.wait_readable("joint.elbow.position")
        adapter = [p for p in lab.processes() if b"ros2_adapter" in Path(f"/proc/{p}/cmdline").read_bytes()]
        t0 = time.monotonic()
        os.kill(int(adapter[0]), signal.SIGKILL)
        p = poll_until_failure(client, "joint.elbow.position", t0, timeout=10)
        recovered = lab.wait_readable("joint.elbow.position", timeout=60)
        out["adapter_kill"] = {"first_failure_ms": p["first_failure_ms"], "error": p["error"],
                               "recovered_after_ms": round((recovered - t0) * 1000, 1)}
        verify = lab.cli("audit", "verify")
        out["adapter_kill"]["audit_verify_exit"] = verify.returncode
        adapter = [p for p in lab.processes() if b"ros2_adapter" in Path(f"/proc/{p}/cmdline").read_bytes()]
        assert lab.proc is not None
        t1 = time.monotonic()
        lab.proc.kill()
        lab.proc.wait()
        gone = None
        while time.monotonic() - t1 < 15:
            if not any(Path(f"/proc/{a}").exists() for a in adapter):
                gone = time.monotonic()
                break
            time.sleep(0.05)
        out["gateway_kill"] = {"adapter_gone_after_ms": None if gone is None else round((gone - t1) * 1000, 1)}
    out["leftover_needing_force"] = lab.leftover
    run.summary["scenarios"]["processes"] = out
    if out["gateway_kill"]["adapter_gone_after_ms"] is None:
        run.fail(8, "the adapter outlived the gateway by more than 15 s")
    if lab.leftover:
        run.fail(8, f"processes needed force: {lab.leftover}")


def hostile_descriptor(run: Run) -> None:
    run.say("F. a descriptor declaring write must not start")

    def widen(m: dict) -> None:
        cap = m["config"]["descriptor"]["capabilities"][0]
        cap["operations"] = ["read", "write"]
        m["capabilities"]["allow"] = [{"name": "*", "operations": ["read", "write", "invoke"]}]

    try:
        with RobotLab(manifest_change=widen) as lab:
            client = lab.agent_client()
            outcome = {"gateway_started": True, "devices": [d.device.id for d in client.devices()]}
            outcome["read"] = lab.read_plain(client, "joint.shoulder.position")
    except Exception as e:  # noqa: BLE001 - refusing to start is the expected outcome
        outcome = {"gateway_started": False, "message": str(e)[-300:]}
    run.summary["scenarios"]["F"]["descriptor_declaring_write"] = outcome
    if outcome.get("gateway_started") and DEVICE in outcome.get("devices", []) and "value" in outcome.get("read", {}):
        run.fail(9, f"the adapter served a descriptor that declares write: {outcome}")


def environment() -> dict[str, Any]:
    def sh(*cmd: str) -> str:
        return subprocess.run(cmd, capture_output=True, text=True, check=False, cwd=REPO).stdout.strip()

    return {
        "commit": sh("git", "rev-parse", "HEAD"),
        "uncommitted_changes": sh("git", "status", "--short", "--", "examples/ros2-readonly-robot",
                                  "adapters/python/ros2", "crates", "python"),
        "date_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "kernel": platform.release(),
        "python": platform.python_version(),
        "distribution": sh("bash", "-c", ". /etc/os-release; echo $PRETTY_NAME"),
        "rmw": sh("bash", "-c", "source /opt/ros/jazzy/setup.bash; ros2 doctor --report 2>/dev/null | "
                                "grep -m1 'middleware name' | awk '{print $NF}'"),
        "ros_distro": sh("bash", "-c", "source /opt/ros/jazzy/setup.bash; echo $ROS_DISTRO"),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cycles", type=int, default=20)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    run = Run(args.out, args.cycles)
    run.summary["environment"] = environment()
    run.summary["cycles"] = args.cycles
    started = time.monotonic()
    with ClockWatch() as clock:
        with RobotLab() as lab:
            lab.wait_readable("joint.elbow.position")
            lab.wait_readable("diagnostics.driver.level")
            lab.wait_readable("battery.voltage")
            lab.wait_readable("base.position.x")
            with lab.mcp() as tools:
                agent = ReadOnlyAgent(tools)
                scenario_a_normal(run, lab, agent)
                scenario_e_unauthorized(run, lab, agent)
                scenario_b_topic_stop(run, lab)
                scenario_cd_kill_restart(run, lab)
                scenario_g_invalid(run, lab)
                scenario_h_timestamps(run, lab)
                scenario_j_grounded(run, lab, agent)
            scenario_f_control(run, lab)
            positive_control(run, lab)
            scenario_i_audit(run, lab)
        if lab.leftover:
            run.fail(8, f"processes needed force at cleanup: {lab.leftover}")
        late_from_the_first_message(run)
        hostile_descriptor(run)
        scenario_processes(run)
    run.summary["metrics"]["wall_clock_steps_ms"] = clock.steps_ms
    run.summary["duration_s"] = round(time.monotonic() - started, 1)
    (args.out / "summary.json").write_text(json.dumps(run.summary, indent=2, default=str) + "\n", encoding="utf-8")
    run.raw.close()
    print(json.dumps(run.summary["metrics"], indent=2))
    print(f"failures: {len(run.summary['failures'])}")
    for f in run.summary["failures"]:
        print(f"  condition {f['condition']}: {f['what']}")
    return 1 if run.summary["failures"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
