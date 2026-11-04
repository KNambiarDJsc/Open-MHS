"""Run the whole battery-equipment example against a throwaway gateway.

    python run_example.py            # human-readable
    python run_example.py --json     # one JSON object on the last line, for tests

SIMULATED instrument -> OpenMHS Modbus adapter -> read-only agent credential -> cited answers.
Needs the ``openmhs`` binary: ``cargo build -p openmhs-cli`` or set ``OPENMHS_BIN``. No hardware,
no network beyond localhost, no credentials.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import agent as ag  # noqa: E402
from battery_lab import CONTROL_POINTS, DEVICE, BatteryLab  # noqa: E402


def audit_summary(lab: BatteryLab) -> dict[str, Any]:
    """What the audit log recorded about the agent credential, from the operator's side."""
    assert lab.operator is not None
    entries = lab.operator.audit_tail(1000)
    reads = [e for e in entries if e.kind == "read"]
    denied_reads = [e for e in entries if e.kind == "read.denied"]
    denied_commands = [e for e in entries
                       if e.kind == "command.transition" and isinstance(e.data, dict)
                       and e.data.get("to") == "DENIED"]
    verify = lab.operator.audit_verify()
    return {
        "successful_reads": len(reads),
        "denied_reads": len(denied_reads),
        "denied_commands": len(denied_commands),
        "read_principals": sorted({e.data.get("principal") for e in reads if isinstance(e.data, dict)}),
        "chain_valid": verify.valid,
        "records": verify.records,
    }


def wait_for(predicate, timeout: float = 15.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.1)
    return False


# ------------------------------------------------------------------------- scenarios


def scenario_normal() -> dict[str, Any]:
    with BatteryLab("normal") as lab, lab.mcp() as m:
        a = ag.ReadOnlyAgent(m)
        status, snap = a.status(), a.snapshot()
        return {"status": ag.answer_status(status), "snapshot": ag.answer_snapshot(snap),
                "audit": audit_summary(lab)}


def scenario_session() -> dict[str, Any]:
    with BatteryLab("session") as lab, lab.mcp() as m:
        a = ag.ReadOnlyAgent(m)
        listing = a.sessions()
        detail = a.session(1)
        return {"list": ag.answer_session_list(listing), "session": ag.answer_session(detail),
                "audit": audit_summary(lab)}


def scenario_snapshot(fixture: str) -> dict[str, Any]:
    with BatteryLab(fixture) as lab, lab.mcp() as m:
        return {"snapshot": ag.answer_snapshot(ag.ReadOnlyAgent(m).snapshot())}


def scenario_irregular_session() -> dict[str, Any]:
    with BatteryLab("irregular_session") as lab, lab.mcp() as m:
        return {"session": ag.answer_session(ag.ReadOnlyAgent(m).session(1))}


def scenario_disconnect() -> dict[str, Any]:
    with BatteryLab("normal") as lab, lab.mcp() as m:
        a = ag.ReadOnlyAgent(m)
        assert lab.sim is not None and lab.operator is not None
        before = a.read("telemetry.voltage")
        lab.sim.set_link(False)
        wait_for(lambda: lab.operator.device_status(DEVICE)[0].availability == "OFFLINE")
        during = a.read("telemetry.voltage")
        status_during = a.status()
        lab.sim.set_link(True)
        # The first read after the link returns reconnects; the gateway then marks it ONLINE.
        after = None
        for _ in range(40):
            after = a.read("telemetry.voltage")
            if after.value is not None:
                break
            time.sleep(0.25)
        wait_for(lambda: lab.operator.device_status(DEVICE, refresh=True)[0].availability == "ONLINE")
        answer = ag.Answer()
        answer.say(f"Before the outage the voltage read {before.text}.", before)
        answer.say(f"During the outage the read failed: no value ({during.state}).", during)
        answer.say(f"After the link returned the voltage read {after.text}.", after)
        return {"before": before, "during": during, "after": after,
                "availability_during": status_during["availability"].value,
                "answer": answer, "audit": audit_summary(lab)}


def scenario_control_denied() -> dict[str, Any]:
    """The agent asks to change a limit, start a test and stop one. All are refused, nothing
    reaches the simulator, and the refusals are in the audit log."""
    with BatteryLab("normal") as lab:
        assert lab.sim is not None
        out: dict[str, Any] = {"attempts": {}, "unscoped_attempts": {}}
        with lab.mcp() as m:
            a = ag.ReadOnlyAgent(m)
            out["attempts"]["limit.voltage_max"] = a.attempt_control("limit.voltage_max", "5V")
            out["attempts"]["test.start"] = a.attempt_control("test.start")
            out["attempts"]["test.stop"] = a.attempt_control("test.stop")
        # Same requests with a credential that has no scope limit: policy alone must refuse.
        with lab.mcp(lab.unscoped_agent_token()) as m2:
            a2 = ag.ReadOnlyAgent(m2)
            out["unscoped_attempts"]["limit.voltage_max"] = a2.attempt_control("limit.voltage_max", "5V")
            out["unscoped_attempts"]["test.start"] = a2.attempt_control("test.start")
        out["simulator_write_attempts"] = list(lab.sim.write_attempts)
        out["audit"] = audit_summary(lab)
        return out


# ------------------------------------------------------------------------- main


def run_all() -> dict[str, Any]:
    results = {
        "normal": scenario_normal(),
        "session": scenario_session(),
        "stale": scenario_snapshot("stale"),
        "suspect": scenario_snapshot("suspect"),
        "malformed": scenario_snapshot("malformed"),
        "irregular_session": scenario_irregular_session(),
        "disconnect": scenario_disconnect(),
        "control_denied": scenario_control_denied(),
    }
    return results


def check(results: dict[str, Any]) -> list[str]:
    """The example's own acceptance checks; the test suite asserts the same and more."""
    problems = []
    for name, r in results.items():
        for key, value in r.items():
            if isinstance(value, ag.Answer):
                problems += [f"{name}.{key}: {p}" for p in ag.check_grounding(value)]
    c = results["control_denied"]
    if c["simulator_write_attempts"]:
        problems.append("a write reached the simulator")
    for group in ("attempts", "unscoped_attempts"):
        for cap, att in c[group].items():
            if att["outcome"] != "denied" or att["reached_device"]:
                problems.append(f"{group}.{cap} was not denied: {att}")
    if not c["audit"]["chain_valid"]:
        problems.append("audit chain did not verify")
    return problems


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()
    results = run_all()
    problems = check(results)
    if args.json:
        def plain(v: Any) -> Any:
            if isinstance(v, ag.Answer):
                return {"text": v.render(), "problems": ag.check_grounding(v)}
            if isinstance(v, ag.Reading):
                return {"capability": v.capability, "state": v.state, "value": v.value}
            if isinstance(v, dict):
                return {k: plain(x) for k, x in v.items()}
            return v
        print(json.dumps({"ok": not problems, "problems": problems,
                          "results": plain(results)}))
        return 0 if not problems else 1

    banner = {
        "normal": "1. Normal operation",
        "session": "2. A completed test session with samples",
        "stale": "3a. Stale data: the instrument stopped updating",
        "suspect": "3b. Suspect data: the instrument doubts its own measurement",
        "malformed": "4a. Malformed data: the adapter refuses an out-of-range value",
        "irregular_session": "4b. Missing, out-of-order and suspect samples in one session",
        "disconnect": "5. Disconnect and reconnect",
        "control_denied": "6. Attempted write and control requests",
    }
    for name, r in results.items():
        print(f"\n=== {banner[name]}")
        for key, value in r.items():
            if isinstance(value, ag.Answer):
                print(value.render())
        if name == "control_denied":
            for group in ("attempts", "unscoped_attempts"):
                label = "scoped read-only credential" if group == "attempts" else "unscoped credential (policy only)"
                print(f"  {label}:")
                for cap in CONTROL_POINTS:
                    if cap in r[group]:
                        a = r[group][cap]
                        print(f"    {cap}: {a['outcome']} {a['reasons']} reached_device={a['reached_device']}")
            print(f"  writes that reached the simulator: {len(r['simulator_write_attempts'])}")
        if "audit" in r:
            au = r["audit"]
            print(f"  audit: {au['successful_reads']} read(s), {au['denied_reads']} denied read(s), "
                  f"{au['denied_commands']} denied command(s); chain valid: {au['chain_valid']}")
    print("\nAll data above is SIMULATED (EXAMPLE-ONLY profile). Nothing here talks to real equipment.")
    if problems:
        print("\nPROBLEMS:\n  " + "\n  ".join(problems))
        return 1
    print("The example behaved as described.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
