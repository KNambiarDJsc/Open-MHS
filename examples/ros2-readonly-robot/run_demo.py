"""The demo, start to finish, in about a minute or two. Everything is simulated.

    python examples/ros2-readonly-robot/run_demo.py

Starts a simulated robot, a throwaway gateway hosting the ROS 2 adapter, and a read-only credential.
Then it reads the robot, stops a topic, kills the node, attempts control, restarts the node, and
verifies the audit chain, printing what the gateway says at each step.
"""

from __future__ import annotations

import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from agent import ReadOnlyAgent, answer_robot, check_grounding  # noqa: E402
from robot_lab import CONTROL_SURFACES, DEVICE, UNGRANTED, RobotLab, command_attempt  # noqa: E402


def step(n: int, title: str) -> None:
    print(f"\n=== {n}. {title}", flush=True)


def show(obs) -> None:
    print("  " + obs.evidence(), flush=True)


def main() -> int:
    print("SIMULATED. No real robot is involved. This is not a safety system.")
    failures = []
    with RobotLab() as lab:
        step(1, "A simulated robot and a gateway are up; the agent holds a read-only credential")
        print(f"  ROS 2 domain {lab.domain}, localhost discovery only, one clock for everything")
        lab.wait_readable("joint.elbow.position")
        graph = lab.ros("graph", "openmhs_observer")
        print(f"  The adapter's node in the ROS graph: publishes {graph['publishers']}; "
              f"services {graph['services']}; service clients {graph['service_clients']}; "
              f"action clients {graph['action_clients']}")

        with lab.mcp() as tools:
            agent = ReadOnlyAgent(tools)

            step(2, "The agent reads the robot")
            print("  Q: Where is the robot, what are the joint positions, is the battery reading current, "
                  "and is anything unavailable?")
            answer = answer_robot(agent.observe_robot())
            print(answer.render())
            problems = check_grounding(answer)
            print(f"  grounding check: {'passed' if not problems else problems}")
            failures += problems

            step(3, "One topic stops publishing (/odom)")
            lab.sim.send("stop", topic="/odom")
            time.sleep(0.8)
            show(agent.read("base.position.x"))
            show(agent.read("joint.elbow.position"))
            print("  The stopped topic has no value; the others are unaffected.")
            lab.sim.send("start", topic="/odom")
            lab.wait_readable("base.position.x")

            step(4, "A bad value: the elbow reports 1000 rad, outside the declared range")
            lab.sim.send("fault", name="range_joint")
            time.sleep(0.3)
            show(agent.read("joint.elbow.position"))
            lab.sim.send("clear")
            lab.wait_readable("joint.elbow.position")

            step(5, "The agent reads something it was not granted (the driver's free-text message)")
            denied = agent.read(UNGRANTED)
            show(denied)

            step(6, "The agent attempts control")
            attempts = [agent.attempt_control("joint.elbow.position", {"value": 0.0, "unit": "rad"}),
                        agent.attempt_control("base.cmd_vel", {"value": 1.0, "unit": "m/s"})]
            for a in attempts:
                print(f"  {a['capability']}: {a['state'] or 'refused'} {a['reasons']}; "
                      f"reached device: {a['reached_device']}")
            with lab.mcp(lab.unscoped_agent_token()) as unscoped:
                a = ReadOnlyAgent(unscoped).attempt_control("joint.elbow.position", {"value": 0.0, "unit": "rad"})
                print(f"  a credential with no scope limit, so only policy decides: {a['state']} {a['reasons']}; "
                      f"reached device: {a['reached_device']}")
            assert lab.operator is not None
            lab.operator.add_grant({"id": "operator-grants-itself-everything", "principal": "operator:*",
                                    "device": "*", "capability": "*", "operations": ["read", "write", "invoke"],
                                    "effect": "allow"})
            a = command_attempt(lab.operator, "joint.elbow.position", "write", {"value": 0.0, "unit": "rad"})
            print(f"  the operator, having granted itself everything: {a['state'] or 'refused'} {a['code']}; "
                  f"dispatched: {a.get('dispatched')}. The capability declares no write, so a grant cannot add one.")

            step(7, "The robot's node crashes (SIGKILL)")
            lab.sim.kill()
            time.sleep(2.5)
            answer = answer_robot(agent.observe_robot())
            print(answer.render())
            problems = check_grounding(answer)
            print(f"  grounding check: {'passed' if not problems else problems}")
            failures += problems

            step(8, "The node restarts")
            started = time.monotonic()
            lab.sim.start()
            lab.wait_readable("joint.elbow.position")
            print(f"  Readable again {time.monotonic() - started:.1f} s after the restart began.")
            show(agent.read("joint.elbow.position"))

        step(9, "Did anything reach a control surface on the robot?")
        print(f"  Commands that reached {', '.join(CONTROL_SURFACES)}: {len(lab.sim.tripwires)}")
        if lab.sim.tripwires:
            failures.append(f"tripwires: {lab.sim.tripwires}")
        print("  Positive control: a separate ROS node, not the agent, now pokes each one directly.")
        reached = lab.ros("poke")["reached"]
        time.sleep(0.5)
        print(f"  reached: {reached}")
        print(f"  counted on the robot: {sorted(set(lab.sim.tripwires))}")
        if set(lab.sim.tripwires) != set(CONTROL_SURFACES):
            failures.append(f"positive control saw only {sorted(set(lab.sim.tripwires))}")

        step(10, "The audit log, and `openmhs audit verify`")
        assert lab.operator is not None
        entries = lab.operator.audit_tail(5000)
        kinds: dict[str, int] = {}
        for e in entries:
            kinds[e.kind] = kinds.get(e.kind, 0) + 1
        for kind in ("read", "read.denied", "command.created", "command.transition"):
            print(f"  {kind}: {kinds.get(kind, 0)}")
        failed_reads = [e for e in entries if e.kind == "read" and "error" in e.data]
        print(f"  reads recorded with a failure: {len(failed_reads)}")
        verify = lab.cli("audit", "verify")
        print("  " + verify.stdout.strip().replace("\n", "\n  "))
        if verify.returncode != 0:
            failures.append(f"audit verify: {verify.stdout} {verify.stderr}")
        print(f"\n  Device id throughout: {DEVICE}")

    if lab.leftover:
        failures.append(f"processes needed force at cleanup: {lab.leftover}")
    print("\nThe demo behaved as described." if not failures else f"\nPROBLEMS: {failures}")
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
