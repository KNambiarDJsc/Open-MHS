"""Tests for the read-only robot example.

The first group needs nothing. The second needs a Linux host with ROS 2 Jazzy at /opt/ros/jazzy and a built
gateway; without ROS 2 it is skipped with a reason, so check that it ran:

    cargo build -p openmhs-cli
    python3 -m unittest discover -s examples/ros2-readonly-robot -p "test_*.py" -v
"""

from __future__ import annotations

import json
import re
import signal
import sys
import tempfile
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import agent as ag  # noqa: E402
import robot_lab as rl  # noqa: E402

MANIFEST = json.loads((HERE / "robot.manifest.json").read_text(encoding="utf-8"))
CAPS = {c["name"]: c for c in MANIFEST["config"]["descriptor"]["capabilities"]}
RAD = {"value": 0.0, "unit": "rad"}


class ManifestTests(unittest.TestCase):
    """The committed contract, without running it."""

    def test_nothing_is_declared_but_reading(self):
        for name, cap in CAPS.items():
            self.assertEqual(cap["operations"], ["read"], name)
            self.assertEqual(cap["kind"], "property", name)
            self.assertEqual(cap["side_effect"], "none", name)
        for rule in MANIFEST["capabilities"]["allow"]:
            self.assertEqual(rule["operations"], ["read"])

    def test_every_capability_states_a_freshness_limit(self):
        for name, cap in CAPS.items():
            self.assertIn("max_age_ms", cap["freshness"], name)

    def test_joints_are_addressed_by_name_not_position(self):
        for path in MANIFEST["config"]["topics"]["/joint_states"]["fields"].values():
            self.assertRegex(path, r"^position\[name=\w+\]$")

    def test_tf_is_not_mapped(self):
        self.assertFalse([t for t in MANIFEST["config"]["topics"] if t.startswith("/tf")])

    def test_the_source_is_declared_simulated(self):
        self.assertIs(MANIFEST["config"]["simulated_source"], True)

    def test_the_agent_gets_exact_names_and_not_the_free_text(self):
        self.assertTrue(set(rl.AGENT_READS) <= set(CAPS))
        self.assertIn(rl.UNGRANTED, CAPS)
        self.assertNotIn(rl.UNGRANTED, rl.AGENT_READS)
        for scope in rl.read_only_scopes():
            self.assertEqual(scope["operations"], ["read"])
            self.assertNotIn("*", scope["capability"])

    def test_policy_denies_control_for_every_agent(self):
        deny = [g for g in rl.read_only_grants() if g["effect"] == "deny"]
        self.assertEqual(len(deny), 1)
        self.assertEqual(sorted(deny[0]["operations"]), ["invoke", "write"])
        self.assertEqual((deny[0]["principal"], deny[0]["capability"]), ("agent:*", "*"))
        allows = [g for g in rl.read_only_grants() if g["effect"] == "allow"]
        self.assertTrue(all(g["operations"] == ["read"] for g in allows))


class FakeTools:
    def __init__(self, replies: dict[str, Any]) -> None:
        self.replies = replies
        self.calls: list[str] = []

    def call(self, name: str, arguments: dict[str, Any] | None = None) -> Any:
        self.calls.append(name)
        return self.replies[(arguments or {}).get("capability")]


def read_ok(value: Any, usable: bool = True, age: int = 10) -> SimpleNamespace:
    return SimpleNamespace(is_error=False, text="", structured={
        "value": value, "timestamp": "2026-11-05T07:13:52.83Z", "quality": "GOOD", "source": "simulator",
        "age_ms": age, "max_age_ms": 1000, "usable": usable})


def read_error(code: str) -> SimpleNamespace:
    return SimpleNamespace(is_error=True, text=f"[{code}] nothing on /odom for 510 ms", structured=None)


def robot_observations(**changes: Any) -> dict[str, ag.Observation]:
    values = {"base.frame": "odom", "base.child_frame": "base_link", "base.position.x": 1.25,
              "base.position.y": -0.5, "base.orientation.z": 0.0, "base.orientation.w": 1.0,
              "joint.shoulder.position": 0.5, "joint.elbow.position": -1.2, "joint.wrist.position": 0.3,
              "battery.voltage": 24.8, "battery.charge": 0.72, "diagnostics.driver.level": 0}
    out = {}
    for name, value in values.items():
        o = ag.Observation(name, ag.CURRENT, value, CAPS[name]["value"].get("unit"),
                           "2026-11-05T07:13:52.83Z", 10, 1000, "GOOD", "simulator")
        out[name] = changes.get(name, o)
    return out


class AgentTests(unittest.TestCase):
    def test_states_come_from_what_the_gateway_said(self):
        tools = FakeTools({
            "a": read_ok({"value": 1.0, "unit": "m"}),
            "b": read_ok({"value": 1.0, "unit": "m"}, usable=False, age=5000),
            "c": read_error("adapter.disconnected"),
            "d": read_error("adapter.malformed_response"),
            "e": read_error("adapter.device_failure"),
            "f": read_error("auth.scope.denied"),
            "g": read_error("policy.denied.no_grant"),
        })
        agent = ag.ReadOnlyAgent(tools)
        expected = {"a": ag.CURRENT, "b": ag.NOT_CURRENT, "c": ag.UNAVAILABLE, "d": ag.INVALID,
                    "e": ag.NOT_MEASURED, "f": ag.DENIED, "g": ag.DENIED}
        for capability, state in expected.items():
            self.assertEqual(agent.read(capability).state, state, capability)
        for capability in "cdefg":
            self.assertIsNone(agent.read(capability).value, "no value is ever filled in")
        self.assertEqual(agent.read("b").value, 1.0, "not current is shown, flagged, not hidden")

    def test_the_agent_only_reads_and_attempts_through_the_gateway(self):
        tools = set(re.findall(r'tools\.call\("(\w+)"', (HERE / "agent.py").read_text(encoding="utf-8")))
        self.assertEqual(tools, {"read_state", "submit_command"})
        imports = (HERE / "agent.py").read_text(encoding="utf-8")
        self.assertNotIn("rclpy", imports)


class GroundingTests(unittest.TestCase):
    def test_a_good_answer_passes(self):
        answer = ag.answer_robot(robot_observations())
        self.assertEqual(ag.check_grounding(answer), [])

    def test_joint_and_diagnostic_words_are_not_mistaken_for_judgements(self):
        answer = ag.Answer()
        o = ag.Observation("joint.shoulder.position", ag.CURRENT, 0.5, "rad", "2026-11-05T07:13:52.83Z", 1, 1000)
        answer.say("Joint shoulder: 0.5 rad; diagnostic level follows.", o)
        self.assertEqual(ag.check_grounding(answer), [])

    def test_an_invented_number_fails(self):
        answer = ag.answer_robot(robot_observations())
        o = answer.evidence[0]
        answer.say("The elbow is at 9.99 rad.", o)
        self.assertTrue(any("9.99" in p for p in ag.check_grounding(answer)))

    def test_an_uncited_statement_fails(self):
        answer = ag.Answer()
        answer.say("The robot is fine.")
        self.assertTrue(ag.check_grounding(answer))

    def test_judgements_fail(self):
        o = ag.Observation("battery.charge", ag.CURRENT, 0.72, "1", "2026-11-05T07:13:52.83Z", 1, 1000)
        for text in ("The battery is healthy.", "You should stop the robot.", "It is safe.",
                     "A diagnosis follows.", "The robot is located at the door."):
            answer = ag.Answer()
            answer.say(text, o)
            self.assertTrue(ag.check_grounding(answer), text)

    def test_a_missing_observation_stays_missing_in_the_answer(self):
        gone = ag.Observation("base.position.x", ag.UNAVAILABLE, error="adapter.disconnected")
        answer = ag.answer_robot(robot_observations(**{"base.position.x": gone}))
        text = answer.render()
        self.assertIn("x no value (UNAVAILABLE)", text)
        self.assertIn("No value is available for: base.position.x", text)
        self.assertNotIn("1.25", text)
        self.assertEqual(ag.check_grounding(answer), [])

    def test_not_current_is_said(self):
        old = ag.Observation("joint.elbow.position", ag.NOT_CURRENT, -1.2, "rad", "2026-11-05T07:13:52.83Z", 5013, 1000)
        answer = ag.answer_robot(robot_observations(**{"joint.elbow.position": old}))
        self.assertIn("NOT current", answer.render())
        self.assertIn("Returned but not current: joint.elbow.position", answer.render())
        self.assertEqual(ag.check_grounding(answer), [])


def eventually(check: Any, timeout: float = 8.0) -> Any:
    """Poll until ``check`` returns something true. Faults and silence are held while it polls, so this only
    waits out a topic's period, its ``stale_after_ms``, or a wall clock step in the environment."""
    deadline = time.monotonic() + timeout
    while True:
        result = check()
        if result or time.monotonic() > deadline:
            return result
        time.sleep(0.05)


@unittest.skipUnless(rl.ros_available(), "needs ROS 2 Jazzy at /opt/ros/jazzy: these tests did not run")
class RosIntegrationTests(unittest.TestCase):
    """A real gateway, the real adapter, real DDS. The robot is simulated."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.lab = rl.RobotLab()
        cls.lab.__enter__()
        for capability in ("joint.elbow.position", "base.position.x", "battery.voltage", "diagnostics.driver.level"):
            cls.lab.wait_readable(capability)
        cls.client = cls.lab.agent_client()
        cls.tools = cls.lab.mcp()
        cls.tools.__enter__()
        cls.agent = ag.ReadOnlyAgent(cls.tools)

    @classmethod
    def tearDownClass(cls) -> None:
        cls.tools.__exit__(None, None, None)
        cls.lab.__exit__(None, None, None)

    def setUp(self) -> None:
        self.lab.sim.send("clear")
        for topic in ("/odom", "/joint_states"):
            self.lab.sim.send("start", topic=topic)
        self.lab.wait_readable("joint.elbow.position")
        self.lab.wait_readable("base.position.x")

    def read(self, capability: str) -> dict[str, Any]:
        return self.lab.read_plain(self.client, capability)

    def fails_with(self, capability: str, code: str) -> dict[str, Any]:
        def check() -> Any:
            got = self.read(capability)
            return got if got.get("error") == code else None

        got = eventually(check)
        self.assertTrue(got, f"{capability} never failed with {code}: last {self.read(capability)}")
        self.assertNotIn("value", got)
        return got

    # -------------------------------------------------------------- normal reads and provenance

    def test_every_granted_capability_reads_with_its_provenance(self):
        for capability in rl.AGENT_READS:
            o = self.agent.read(capability)
            with self.subTest(capability):
                self.assertEqual(o.state, ag.CURRENT, o.evidence())
                self.assertEqual(o.source, "simulator")
                self.assertEqual(o.quality, "GOOD")
                self.assertIsNotNone(o.timestamp)
                self.assertIsNotNone(o.age_ms)
                self.assertEqual(o.max_age_ms, CAPS[capability]["freshness"]["max_age_ms"])
                self.assertEqual(o.unit, CAPS[capability]["value"].get("unit"))

    def test_joint_values_are_the_fixtures(self):
        self.assertAlmostEqual(self.read("joint.shoulder.position")["value"], 0.5)
        self.assertAlmostEqual(self.read("joint.elbow.position")["value"], -1.2)
        self.assertEqual(self.read("base.frame")["value"], "odom")

    # -------------------------------------------------------------- scope

    def test_a_read_only_credential_cannot_exceed_its_scope(self):
        for capability in (rl.UNGRANTED, "tf.base_link.x", "cmd_vel.linear.x"):
            o = self.agent.read(capability)
            self.assertEqual((o.state, o.value), (ag.DENIED, None), capability)
            self.assertEqual(o.error, "auth.scope.denied")

    def test_scope_covers_other_devices(self):
        o = ag.ReadOnlyAgent(self.tools, device="sim.psu-01").read("output.voltage")
        self.assertEqual((o.state, o.error), (ag.DENIED, "auth.scope.denied"))

    # -------------------------------------------------------------- control

    def test_control_cannot_reach_ros(self):
        before = len(self.lab.sim.tripwires)
        tries = [("joint.elbow.position", "write", RAD), ("joint.elbow.position", "invoke", None),
                 ("base.cmd_vel", "write", {"value": 1.0, "unit": "m/s"}), ("joint.elbow.move", "invoke", None)]
        unscoped = self.lab.client_for(self.lab.unscoped_agent_token("agent:unscoped-control"))
        for client, name, code_prefix in ((self.client, "scoped", "auth."), (unscoped, "policy only", ("policy.", "command."))):
            for capability, operation, value in tries:
                a = rl.command_attempt(client, capability, operation, value)
                with self.subTest(credential=name, capability=capability, operation=operation):
                    self.assertTrue(a["refused"], a)
                    self.assertFalse(a.get("dispatched"), a)
                    self.assertTrue(str(a["code"]).startswith(code_prefix), a)
        time.sleep(0.3)
        self.assertEqual(self.lab.sim.tripwires[before:], [])

    def test_a_grant_cannot_create_an_operation_the_descriptor_does_not_declare(self):
        """Stronger than any instruction to the agent: the operator allows itself everything."""
        assert self.lab.operator is not None
        self.lab.operator.add_grant({"id": "operator-everything", "principal": "operator:*", "device": "*",
                                     "capability": "*", "operations": ["read", "write", "invoke"], "effect": "allow"})
        before = len(self.lab.sim.tripwires)
        for operation, value in (("write", RAD), ("invoke", None)):
            a = rl.command_attempt(self.lab.operator, "joint.elbow.position", operation, value)
            self.assertEqual((a["refused"], a["code"], a.get("dispatched")),
                             (True, "command.operation.unsupported", False), a)
        time.sleep(0.3)
        self.assertEqual(self.lab.sim.tripwires[before:], [])

    def test_a_plan_is_refused_too(self):
        before = len(self.lab.sim.tripwires)
        plan = self.tools.call("submit_plan", {"plan": {"description": "an attempt", "steps": [
            {"id": "s1", "request": {"target": {"device": rl.DEVICE, "capability": "joint.elbow.position"},
                                     "operation": "write", "value": RAD}}]}})
        self.assertTrue(plan.is_error, plan)
        self.assertEqual(self.lab.sim.tripwires[before:], [])

    def added_records(self, seq: int) -> list[Any]:
        assert self.lab.operator is not None
        return [e for e in self.lab.operator.audit_tail(50)
                if e.seq > seq and e.kind != "device.availability"]

    def test_a_command_to_a_capability_that_does_not_exist_leaves_evidence(self):
        """This evaluation found this refusal unrecorded at 6a324f4. The MCP server now forwards it, so
        the gateway refuses and records it: a command, denied, with the scope code."""
        assert self.lab.operator is not None
        seq = self.lab.operator.audit_tail(1)[-1].seq
        before = len(self.lab.sim.tripwires)
        unknown = self.tools.call("submit_command", {"device": rl.DEVICE, "capability": "base.cmd_vel",
                                                     "value": {"value": 1.0, "unit": "m/s"},
                                                     "idempotency_key": f"unknown-{time.monotonic_ns()}"})
        self.assertTrue(unknown.is_error)
        self.assertIn("auth.scope.denied", unknown.text)
        time.sleep(0.2)
        added = self.added_records(seq)
        self.assertEqual([e.kind for e in added if e.kind == "command.created"], ["command.created"])
        self.assertTrue(any(e.kind == "command.transition" and "DENIED" in json.dumps(e.data) for e in added))
        self.assertEqual(self.lab.sim.tripwires[before:], [])

    def test_a_refused_plan_leaves_evidence_of_who_and_why(self):
        """Also found unrecorded at 6a324f4. No command or plan came to exist, so it is request.rejected."""
        assert self.lab.operator is not None
        seq = self.lab.operator.audit_tail(1)[-1].seq
        plan = self.tools.call("submit_plan", {"plan": {"description": "an attempt", "steps": [
            {"id": "s1", "request": {"target": {"device": rl.DEVICE, "capability": "joint.elbow.position"},
                                     "operation": "write", "value": RAD}}]}})
        self.assertTrue(plan.is_error)
        time.sleep(0.2)
        records = [e for e in self.added_records(seq) if e.kind == "request.rejected"]
        self.assertEqual(len(records), 1)
        data = records[0].data
        self.assertEqual((data["method"], data["code"], data["principal"]),
                         ("plans.submit", "lease.not_authorized", rl.AGENT))
        self.assertEqual(data["target"]["devices"], [rl.DEVICE])
        self.assertNotIn("an attempt", json.dumps(data))
        self.assertNotIn(self.lab.agent_token, json.dumps(data))

    def test_reaching_for_operator_authority_leaves_evidence(self):
        assert self.lab.operator is not None
        seq = self.lab.operator.audit_tail(1)[-1].seq
        with self.assertRaises(Exception) as caught:
            self.client.create_token("agent:another", any_scope=True)
        self.assertEqual(getattr(caught.exception, "code", None), "gateway.forbidden")
        records = [e for e in self.added_records(seq) if e.kind == "request.rejected"]
        self.assertEqual([(e.data["method"], e.data["code"]) for e in records],
                         [("tokens.create", "gateway.forbidden")])

    def test_the_adapter_offers_nothing_to_the_ros_graph(self):
        graph = self.lab.ros("graph", "openmhs_observer")
        self.assertTrue(graph["found"])
        self.assertEqual(graph["publishers"], ["/parameter_events"], "the only one, and rclpy creates it")
        self.assertEqual(graph["services"], [])
        self.assertEqual(graph["service_clients"], [])
        self.assertEqual(graph["action_servers"], [])
        self.assertEqual(graph["action_clients"], [])

    def test_it_subscribes_only_to_what_is_declared(self):
        graph = self.lab.ros("graph", "openmhs_observer")
        self.assertEqual(sorted(graph["subscriptions"]), sorted(MANIFEST["config"]["topics"]))
        self.assertNotIn("/tf", graph["subscriptions"])

    # -------------------------------------------------------------- freshness

    def test_a_stopped_topic_is_visible_and_others_are_not_affected(self):
        self.lab.sim.send("stop", topic="/odom")
        got = self.fails_with("base.position.x", "adapter.disconnected")
        self.assertIn("/odom", got["message"])
        self.assertTrue(self.read("joint.elbow.position").get("usable"))
        self.lab.sim.send("start", topic="/odom")
        self.lab.wait_readable("base.position.x")
        self.assertAlmostEqual(self.read("base.position.x")["value"], 1.25)

    def test_stale_data_is_not_presented_as_fresh(self):
        self.lab.sim.send("stop", topic="/joint_states")
        self.fails_with("joint.elbow.position", "adapter.disconnected")
        for _ in range(10):
            self.assertNotIn("value", self.read("joint.elbow.position"))
            time.sleep(0.05)

    def test_old_stamps_from_the_first_message_are_not_usable(self):
        with rl.RobotLab(fault_from_start="late_stamp") as lab:
            client = lab.agent_client()
            got = eventually(lambda: (lambda g: g if "value" in g else None)(lab.read_plain(client, "joint.elbow.position")), 20)
        self.assertTrue(got)
        self.assertIs(got["usable"], False, got)
        self.assertGreaterEqual(got["age_ms"], 4000)

    def test_a_stamp_from_the_future_is_not_usable_and_does_not_poison_the_topic(self):
        self.lab.sim.send("fault", name="future_stamp")
        got = eventually(lambda: (lambda g: g if g.get("usable") is False and "value" in g else None)(
            self.read("joint.elbow.position")))
        self.assertTrue(got)
        self.assertLess(got["age_ms"], 0)
        self.lab.sim.send("clear")
        self.lab.wait_readable("joint.elbow.position", timeout=8)

    def test_an_unset_stamp_means_arrival_not_the_time_of_the_read(self):
        self.lab.sim.send("fault", name="zero_stamp")
        eventually(lambda: self.read("joint.elbow.position").get("age_ms", 9999) < 200)
        self.lab.sim.send("stop", topic="/joint_states")
        time.sleep(0.3)
        got = self.read("joint.elbow.position")
        self.assertGreaterEqual(got.get("age_ms", 0), 250, got)

    def test_time_never_goes_backwards_in_what_is_returned(self):
        self.lab.sim.send("fault", name="regress_small")
        time.sleep(0.3)
        times = []
        for _ in range(100):
            got = self.read("joint.elbow.position")
            if "seconds" in got:
                times.append(got["seconds"])
            time.sleep(0.01)
        self.assertGreater(len(times), 50)
        self.assertEqual([(a, b) for a, b in zip(times, times[1:]) if b < a], [])

    def test_stamps_held_behind_become_a_visible_failure_that_says_why(self):
        self.lab.sim.send("fault", name="regress_step")
        got = self.fails_with("joint.elbow.position", "adapter.malformed_response")
        self.assertIn("older", got["message"])
        self.lab.sim.send("clear")
        self.lab.wait_readable("joint.elbow.position", timeout=8)

    # -------------------------------------------------------------- invalid data

    def test_invalid_data_is_not_silently_accepted(self):
        cases = [("nan_joint", "joint.elbow.position", "adapter.device_failure"),
                 ("inf_odom", "base.position.x", "adapter.device_failure"),
                 ("nan_battery_temperature", "battery.temperature", "adapter.device_failure"),
                 ("range_joint", "joint.elbow.position", "adapter.malformed_response"),
                 ("range_battery", "battery.charge", "adapter.malformed_response"),
                 ("empty_positions", "joint.elbow.position", "adapter.malformed_response"),
                 ("mismatched_arrays", "joint.shoulder.position", "adapter.malformed_response"),
                 ("missing_joint", "joint.elbow.position", "adapter.malformed_response"),
                 ("no_diag_status", "diagnostics.driver.level", "adapter.malformed_response")]
        for fault, capability, code in cases:
            with self.subTest(fault):
                self.lab.sim.send("fault", name=fault)
                self.fails_with(capability, code)
                self.lab.sim.send("clear")
                self.lab.wait_readable(capability, timeout=8)

    def test_joints_follow_their_names_when_the_array_order_changes(self):
        self.lab.sim.send("fault", name="reordered_joints")
        time.sleep(0.2)
        for _ in range(20):
            got = self.read("joint.shoulder.position")
            if "value" in got:
                self.assertAlmostEqual(got["value"], 0.5)
            time.sleep(0.02)

    def test_a_reported_error_level_is_a_value_not_a_failure(self):
        self.lab.sim.send("fault", name="diag_error")
        got = eventually(lambda: (lambda g: g if g.get("value") == 2 else None)(self.read("diagnostics.driver.level")))
        self.assertTrue(got)
        self.lab.sim.send("clear")
        eventually(lambda: self.read("diagnostics.driver.level").get("value") == 0)

    # -------------------------------------------------------------- answers

    def test_the_answer_is_grounded_when_things_are_normal_and_when_they_are_not(self):
        normal = ag.answer_robot(self.agent.observe_robot())
        self.assertEqual(ag.check_grounding(normal), [])
        self.assertIn("-1.2 rad, current", normal.render())
        self.lab.sim.send("stop", topic="/odom")
        self.fails_with("base.position.x", "adapter.disconnected")
        degraded = ag.answer_robot(self.agent.observe_robot())
        self.assertEqual(ag.check_grounding(degraded), [])
        self.assertNotIn("1.25", degraded.render())
        self.assertIn("base.position.x", degraded.render())
        self.assertIn("-1.2 rad, current", degraded.render())

    # -------------------------------------------------------------- audit

    def test_the_audit_records_reads_failures_and_denials_and_verifies(self):
        self.agent.read(rl.UNGRANTED)
        self.lab.sim.send("stop", topic="/odom")
        self.fails_with("base.position.x", "adapter.disconnected")
        a = rl.command_attempt(self.client, "joint.elbow.position", "write", RAD)
        self.assertTrue(a["refused"])
        with tempfile.TemporaryDirectory() as d:
            export = self.lab.cli("audit", "export", "--out", f"{d}/e.json", "--checkpoint-out", f"{d}/c.json")
            self.assertEqual(export.returncode, 0, export.stderr)
            bundle = json.loads(Path(f"{d}/e.json").read_text(encoding="utf-8"))
            offline = self.lab.cli("--json", "audit", "verify-file", f"{d}/e.json", "--checkpoint", f"{d}/c.json")
        self.assertEqual(offline.returncode, 0, offline.stdout)
        self.assertIs(json.loads(offline.stdout)["valid"], True)
        records = [(r["kind"], json.loads(r["data"])) for r in bundle["audit"]]
        self.assertTrue(any(k == "read.denied" and d["capability"] == rl.UNGRANTED and d["principal"] == rl.AGENT
                            for k, d in records))
        self.assertTrue(any(k == "read" and d.get("capability") == "base.position.x"
                            and d.get("error") == "adapter.disconnected" for k, d in records))
        self.assertTrue(any(k == "read" and d.get("state", {}).get("source") == "simulator" for k, d in records))
        self.assertTrue(any(k == "command.transition" and "DENIED" in json.dumps(d) for k, d in records))
        verify = self.lab.cli("audit", "verify")
        self.assertEqual(verify.returncode, 0, verify.stdout)
        self.assertIn("hash chain verified", verify.stdout)

    # -------------------------------------------------------------- what leaks

    def test_the_credential_is_not_in_a_command_line_a_child_environment_or_a_log(self):
        token = self.lab.agent_token.encode()
        self.assertGreater(len(token), 8)
        assert self.lab.proc is not None
        adapters = [p for p in self.lab.processes() if b"ros2_adapter" in Path(f"/proc/{p}/cmdline").read_bytes()]
        self.assertTrue(adapters)
        for pid in [str(self.lab.proc.pid), *adapters]:
            self.assertNotIn(token, Path(f"/proc/{pid}/cmdline").read_bytes())
        for pid in adapters:
            environ = Path(f"/proc/{pid}/environ").read_bytes()
            self.assertNotIn(token, environ)
            self.assertNotIn(b"OPENMHS_TOKEN", environ)
        self.assertNotIn(token.decode(), self.lab.log_tail(10_000_000))


@unittest.skipUnless(rl.ros_available(), "needs ROS 2 Jazzy at /opt/ros/jazzy: these tests did not run")
class RosProcessTests(unittest.TestCase):
    """Each with a lab of its own, because each breaks it."""

    def test_the_tripwires_work(self):
        """The positive control: without it, a count of zero could be a broken counter."""
        with rl.RobotLab() as lab:
            reached = lab.ros("poke")["reached"]
            self.assertEqual(set(reached), set(rl.CONTROL_SURFACES))
            self.assertTrue(all(reached.values()), reached)
            eventually(lambda: set(lab.sim.tripwires) == set(rl.CONTROL_SURFACES), 5)
            self.assertEqual(set(lab.sim.tripwires), set(rl.CONTROL_SURFACES))

    def test_node_death_is_visible_and_a_restart_recovers_without_restarting_the_gateway(self):
        with rl.RobotLab() as lab:
            client = lab.agent_client()
            lab.wait_readable("joint.elbow.position")
            assert lab.proc is not None
            pid = lab.proc.pid
            lab.sim.kill()

            def all_failed() -> bool:
                return all("value" not in lab.read_plain(client, c) for c in rl.AGENT_READS)

            self.assertTrue(eventually(all_failed), "every capability fails once its topic has gone silent")
            lab.sim.start()
            lab.wait_readable("joint.elbow.position")
            self.assertAlmostEqual(lab.read_plain(client, "joint.elbow.position")["value"], -1.2)
            self.assertEqual(lab.proc.pid, pid)
            self.assertIsNone(lab.proc.poll())
        self.assertEqual(lab.leftover, [])

    def test_a_descriptor_declaring_write_does_not_start(self):
        def widen(m: dict[str, Any]) -> None:
            m["config"]["descriptor"]["capabilities"][0]["operations"] = ["read", "write"]
            m["capabilities"]["allow"] = [{"name": "*", "operations": ["read", "write", "invoke"]}]

        with self.assertRaisesRegex(RuntimeError, "only observes"):
            with rl.RobotLab(manifest_change=widen):
                self.fail("the gateway started with a descriptor that declares write")

    def test_a_killed_adapter_is_restarted_and_recovers(self):
        with rl.RobotLab() as lab:
            client = lab.agent_client()
            lab.wait_readable("joint.elbow.position")
            adapter = [p for p in lab.processes() if b"ros2_adapter" in Path(f"/proc/{p}/cmdline").read_bytes()][0]
            import os
            os.kill(int(adapter), signal.SIGKILL)
            self.assertTrue(eventually(lambda: "value" not in lab.read_plain(client, "joint.elbow.position"), 5))
            lab.wait_readable("joint.elbow.position", timeout=60)
            self.assertEqual(lab.cli("audit", "verify").returncode, 0)
        self.assertEqual(lab.leftover, [])

    def test_the_adapter_does_not_outlive_a_killed_gateway(self):
        with rl.RobotLab() as lab:
            lab.wait_readable("joint.elbow.position")
            adapters = [p for p in lab.processes() if b"ros2_adapter" in Path(f"/proc/{p}/cmdline").read_bytes()]
            assert lab.proc is not None
            lab.proc.kill()
            lab.proc.wait()
            self.assertTrue(eventually(lambda: not any(Path(f"/proc/{p}").exists() for p in adapters), 15))
        self.assertEqual(lab.leftover, [])


if __name__ == "__main__":
    unittest.main()
