"""Tests for the simulated battery equipment example.

    python -m unittest discover -s examples/battery-equipment-simulator -p "test_*.py" -v

The first group needs nothing but Python. The second starts the real ``openmhs`` binary
(``$OPENMHS_BIN``, else ``target/debug/openmhs``) against the simulated instrument on localhost,
and fails loudly if the binary is missing rather than skipping.
"""

from __future__ import annotations

import socket
import struct
import sys
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import agent as ag  # noqa: E402
import build_profile  # noqa: E402
import register_map as rm  # noqa: E402
import simulator  # noqa: E402
from battery_lab import CONTROL_POINTS, DEVICE, BatteryLab  # noqa: E402


# --------------------------------------------------------------------------- no gateway


class RegisterMap(unittest.TestCase):
    def test_no_two_capabilities_share_an_address(self) -> None:
        used: dict[tuple[str, int], str] = {}
        for r in rm.registers():
            for i in range(r.width):
                key = (r.table, r.address + i)
                self.assertNotIn(key, used, f"{r.name} overlaps {used.get(key)}")
                used[key] = r.name

    def test_values_round_trip_through_registers(self) -> None:
        for name, value in [("telemetry.voltage", 3.712), ("telemetry.current", -1.5),
                            ("telemetry.temperature", 24.6), ("telemetry.sample_time", 1768471200),
                            ("instrument.state", "testing"), ("instrument.simulated", True)]:
            reg = rm.by_name()[name]
            self.assertEqual(rm.decode(reg, rm.encode(reg, value)), value, name)

    def test_unrepresentable_values_are_refused_not_rounded(self) -> None:
        reg = rm.by_name()["telemetry.voltage"]
        with self.assertRaises(ValueError):
            rm.encode(reg, 3.7123)  # finer than the 1 mV resolution
        with self.assertRaises(ValueError):
            rm.encode(reg, 70.0)  # does not fit 16 bits at 1 mV

    def test_the_committed_profile_is_what_the_generator_produces(self) -> None:
        self.assertEqual(build_profile.PROFILE.read_text(encoding="utf-8"), build_profile.render())

    def test_everything_is_labelled_simulated_and_example_only(self) -> None:
        p = build_profile.build()
        d = p["descriptor"]["device"]
        self.assertIn("SIMULATED", d["name"])
        self.assertIn("EXAMPLE-ONLY", d["name"])
        self.assertIn("EXAMPLE-ONLY", d["description"])
        self.assertTrue(p["connection"]["simulated"])
        for f in HERE.joinpath("fixtures").glob("*.json"):
            self.assertIn("SIMULATED", f.read_text(encoding="utf-8"), f.name)


def modbus(sock: socket.socket, pdu: bytes, tid: int = 1) -> bytes:
    sock.sendall(struct.pack(">HHHB", tid, 0, len(pdu) + 1, 1) + pdu)
    try:
        header = b""
        while len(header) < 7:
            chunk = sock.recv(7 - len(header))
            if not chunk:
                return b""
            header += chunk
        _, _, length, _ = struct.unpack(">HHHB", header)
        body = b""
        while len(body) < length - 1:
            chunk = sock.recv(length - 1 - len(body))
            if not chunk:
                break
            body += chunk
        return body
    except ConnectionError:  # the instrument dropped the connection (Windows reports it as an error)
        return b""


class SimulatorOnTheWire(unittest.TestCase):
    """The simulator speaks Modbus TCP and is read-only at the device, with no gateway involved."""

    def setUp(self) -> None:
        self.sim = simulator.BatterySimulator(simulator.load_fixture("session"))
        self.port = self.sim.start()
        self.sock = socket.create_connection(("127.0.0.1", self.port), timeout=5)

    def tearDown(self) -> None:
        self.sock.close()
        self.sim.stop()

    def test_reads_registers(self) -> None:
        reg = rm.by_name()["telemetry.voltage"]
        reply = modbus(self.sock, struct.pack(">BHH", 4, reg.address, 1))
        self.assertEqual(reply[0], 4)
        self.assertEqual(struct.unpack(">H", reply[2:4])[0], 3650)  # 3.650 V at 1 mV

    def test_every_write_function_is_refused_and_recorded(self) -> None:
        for pdu in (struct.pack(">BHH", 5, 0, 0xFF00),            # write single coil: test.start
                    struct.pack(">BHH", 6, 0, 5000),              # write single register: limit
                    struct.pack(">BHHB", 16, 0, 1, 2) + b"\x13\x88"):  # write multiple registers
            reply = modbus(self.sock, pdu)
            self.assertEqual(reply[0], pdu[0] | 0x80)
            self.assertEqual(reply[1], simulator.ILLEGAL_FUNCTION)
        self.assertEqual([w["function"] for w in self.sim.write_attempts], [5, 6, 16])

    def test_unknown_addresses_are_refused(self) -> None:
        reply = modbus(self.sock, struct.pack(">BHH", 4, 900, 1))
        self.assertEqual(reply, bytes([0x84, simulator.ILLEGAL_ADDRESS]))

    def test_sample_time_follows_the_fixture_age(self) -> None:
        self.sim.load({"name": "t", "sample_age_s": 120, "values": {}})
        reg = rm.by_name()["telemetry.sample_time"]
        reply = modbus(self.sock, struct.pack(">BHH", 4, reg.address, 2))
        stamp = struct.unpack(">I", reply[2:6])[0]
        self.assertAlmostEqual(time.time() - stamp, 120, delta=2)

    def test_link_down_drops_connections(self) -> None:
        self.sim.set_link(False)
        time.sleep(0.2)
        self.assertEqual(modbus(self.sock, struct.pack(">BHH", 4, 0, 1)), b"")


class Grounding(unittest.TestCase):
    """The check that keeps answers tied to the data."""

    def reading(self, cap: str, value: object, unit: str | None = None) -> ag.Reading:
        return ag.Reading(cap, ag.OK, value, unit, "2026-11-04T05:00:00Z", "simulator", "GOOD", True)

    def test_a_grounded_answer_passes(self) -> None:
        a = ag.Answer()
        a.say("Voltage: 3.712 V.", self.reading("telemetry.voltage", 3.712, "V"))
        self.assertEqual(ag.check_grounding(a), [])

    def test_an_invented_number_is_caught(self) -> None:
        a = ag.Answer()
        a.say("Voltage: 3.9 V.", self.reading("telemetry.voltage", 3.712, "V"))
        self.assertTrue(any("3.9" in p for p in ag.check_grounding(a)))

    def test_an_uncited_claim_is_caught(self) -> None:
        a = ag.Answer()
        a.lines.append(("Voltage: 3.712 V.", []))
        self.assertTrue(any("no citation" in p for p in ag.check_grounding(a)))

    def test_judgements_the_data_cannot_support_are_caught(self) -> None:
        for phrase in ("The battery is healthy.", "Remaining life looks short.",
                       "This diagnosis is clear.", "You should stop the test."):
            a = ag.Answer()
            a.say(phrase, self.reading("telemetry.voltage", 3.7, "V"))
            self.assertTrue(ag.check_grounding(a), phrase)

    def test_gateway_timestamps_parse_on_old_pythons(self) -> None:
        self.assertAlmostEqual(
            ag.parse_time("2026-11-04T05:25:49.52Z") - ag.parse_time("2026-11-04T05:25:49Z"), 0.52, places=3)
        self.assertAlmostEqual(ag.parse_time("2026-01-15T10:00:00Z"), 1768471200)


# --------------------------------------------------------------------------- real gateway


class Counting:
    """Wrap an MCP client and count the successful ``read_state`` calls it makes."""

    def __init__(self, tools: object) -> None:
        self.tools = tools
        self.reads = 0

    def call(self, name: str, arguments: dict | None = None):  # noqa: ANN201
        result = self.tools.call(name, arguments)  # type: ignore[attr-defined]
        if name == "read_state" and not result.is_error:
            self.reads += 1
        return result


class ThroughTheGateway(unittest.TestCase):
    def test_the_device_is_discoverable_and_its_data_is_labelled_simulated(self) -> None:
        with BatteryLab("normal") as lab, lab.mcp() as m:
            devices = m.call("list_devices", {}).structured["devices"]
            self.assertEqual([d.get("id", d.get("device")) for d in devices], [DEVICE])
            self.assertEqual(devices[0]["availability"], "ONLINE")
            desc = m.call("describe_device", {"device": DEVICE}).structured
            self.assertIn("SIMULATED", str(desc))
            snap = ag.ReadOnlyAgent(m).snapshot()
            for r in snap.values():
                self.assertEqual(r.source, "simulator", r.capability)  # never "device"

    def test_status_and_telemetry_are_parsed_with_units_identity_and_timestamps(self) -> None:
        with BatteryLab("normal") as lab, lab.mcp() as m:
            a = ag.ReadOnlyAgent(m)
            status, snap = a.status(), a.snapshot()
            self.assertEqual(status["availability"].value, "ONLINE")
            self.assertEqual(status["state"].value, "testing")
            self.assertIs(status["simulated"].value, True)
            v, i, t = (snap[k] for k in ("telemetry.voltage", "telemetry.current", "telemetry.temperature"))
            self.assertEqual((v.value, v.unit), (3.712, "V"))
            self.assertEqual((i.value, i.unit), (1.5, "A"))
            self.assertEqual((t.value, t.unit), (24.6, "Cel"))
            for r in (v, i, t):
                self.assertEqual((r.state, r.quality, r.usable), (ag.OK, "GOOD", True))
                self.assertRegex(r.timestamp or "", r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d+)?Z$")

    def test_sessions_are_listed_and_one_is_retrieved_with_its_samples(self) -> None:
        with BatteryLab("session") as lab, lab.mcp() as m:
            a = ag.ReadOnlyAgent(m)
            listing = a.sessions()
            self.assertEqual(listing["count"].value, 2)
            ids = [s["id"].value for s in listing["items"]]
            self.assertEqual(ids, [1002, 1001])
            self.assertEqual([s["status"].value for s in listing["items"]], ["completed", "aborted"])
            detail = a.session(1)
            meta = detail["meta"]
            self.assertEqual((meta["id"].value, meta["asset_id"].value, meta["procedure_version"].value),
                             (1002, 7001, 3))
            self.assertEqual(meta["start_time"].value, 1768471200)
            volts = [s["voltage"].value for s in detail["samples"]]
            self.assertEqual(volts, [3.912, 3.887, 3.861, 3.838])
            self.assertTrue(all(s["voltage"].state == ag.OK for s in detail["samples"]))
            times = [s["time"].value for s in detail["samples"]]
            self.assertEqual(times, sorted(times))

    def test_stale_data_is_flagged_and_not_presented_as_current(self) -> None:
        with BatteryLab("stale") as lab, lab.mcp() as m:
            snap = ag.ReadOnlyAgent(m).snapshot()
            v = snap["telemetry.voltage"]
            self.assertEqual(v.value, 3.712)  # still shown
            self.assertIn(ag.STALE, v.flags)
            self.assertAlmostEqual(v.derived["sample_age_s"], 300, delta=2)
            text = ag.answer_snapshot(snap).render()
            self.assertIn("do not treat as current", text)

    def test_suspect_data_is_returned_with_the_flag(self) -> None:
        with BatteryLab("suspect") as lab, lab.mcp() as m:
            v = ag.ReadOnlyAgent(m).snapshot()["telemetry.voltage"]
            self.assertEqual((v.value, v.state), (3.712, ag.SUSPECT))

    def test_malformed_data_is_refused_and_leaves_no_value(self) -> None:
        with BatteryLab("malformed") as lab, lab.mcp() as m:
            snap = ag.ReadOnlyAgent(m).snapshot()
            v = snap["telemetry.voltage"]
            self.assertIsNone(v.value)
            self.assertEqual((v.state, v.error), (ag.MALFORMED, "adapter.malformed_response"))
            self.assertEqual(snap["telemetry.current"].value, 1.5)  # others still read
            self.assertIn("no value", ag.answer_snapshot(snap).render())

    def test_missing_and_out_of_order_samples_are_flagged_and_never_filled(self) -> None:
        with BatteryLab("irregular_session") as lab, lab.mcp() as m:
            samples = ag.ReadOnlyAgent(m).session(1)["samples"]
            missing = samples[1]
            self.assertTrue(all(missing[f].value is None and missing[f].state == ag.MISSING
                                for f in ("voltage", "current", "temperature")))
            late = samples[2]  # timestamped before sample 2
            self.assertEqual(late["voltage"].state, ag.OUT_OF_ORDER)
            self.assertEqual(late["voltage"].value, 3.85)  # flagged, not corrected or reordered
            self.assertEqual(samples[3]["voltage"].state, ag.SUSPECT)
            self.assertEqual(samples[0]["voltage"].state, ag.OK)

    def test_disconnect_and_reconnect(self) -> None:
        with BatteryLab("normal") as lab, lab.mcp() as m:
            a = ag.ReadOnlyAgent(m)
            self.assertEqual(a.read("telemetry.voltage").state, ag.OK)
            lab.sim.set_link(False)
            deadline = time.monotonic() + 15
            while lab.operator.device_status(DEVICE)[0].availability != "OFFLINE":
                self.assertLess(time.monotonic(), deadline, "gateway never marked the device OFFLINE")
                time.sleep(0.1)
            down = a.read("telemetry.voltage")
            self.assertEqual((down.state, down.error, down.value), (ag.UNREACHABLE, "adapter.disconnected", None))
            lab.sim.set_link(True)
            deadline = time.monotonic() + 15
            back = a.read("telemetry.voltage")
            while back.value is None:
                self.assertLess(time.monotonic(), deadline, "reading never recovered")
                time.sleep(0.25)
                back = a.read("telemetry.voltage")
            self.assertEqual((back.state, back.value), (ag.OK, 3.712))

    def test_successful_reads_are_audited_one_for_one(self) -> None:
        with BatteryLab("session") as lab, lab.mcp() as m:
            counted = Counting(m)
            a = ag.ReadOnlyAgent(counted)
            a.snapshot()
            a.sessions()
            a.session(1)
            audit = [e for e in lab.operator.audit_tail(1000) if e.kind == "read"]
            self.assertEqual(len(audit), counted.reads)
            self.assertGreater(counted.reads, 20)
            for e in audit:
                self.assertEqual(e.subject, DEVICE)
                self.assertEqual(e.data["principal"], "agent:battery-example")
                self.assertEqual(e.data["state"]["source"], "simulator")
            self.assertTrue(lab.operator.audit_verify().valid)

    def test_reading_a_control_point_is_denied_and_audited(self) -> None:
        with BatteryLab("normal") as lab, lab.mcp() as m:
            r = ag.ReadOnlyAgent(m).read("limit.voltage_max")
            self.assertEqual((r.state, r.value), (ag.DENIED, None))
            kinds = [e.kind for e in lab.operator.audit_tail(1000)]
            self.assertIn("read.denied", kinds)

    def test_write_and_control_requests_are_denied_audited_and_never_reach_the_device(self) -> None:
        with BatteryLab("normal") as lab:
            outcomes = {}
            with lab.mcp() as m:
                a = ag.ReadOnlyAgent(m)
                outcomes["scoped"] = {
                    "limit.voltage_max": a.attempt_control("limit.voltage_max", "5V"),
                    "test.start": a.attempt_control("test.start"),
                    "test.stop": a.attempt_control("test.stop"),
                }
            with lab.mcp(lab.unscoped_agent_token()) as m2:  # the policy layer alone
                a2 = ag.ReadOnlyAgent(m2)
                outcomes["unscoped"] = {
                    "limit.voltage_max": a2.attempt_control("limit.voltage_max", "5V"),
                    "test.start": a2.attempt_control("test.start"),
                    "test.stop": a2.attempt_control("test.stop"),
                }
            for who, attempts in outcomes.items():
                self.assertEqual(sorted(attempts), sorted(CONTROL_POINTS))
                for cap, att in attempts.items():
                    self.assertEqual(att["outcome"], "denied", (who, cap, att))
                    self.assertEqual(att["state"], "DENIED", (who, cap))
                    self.assertIs(att["reached_device"], False, (who, cap))
            self.assertEqual(outcomes["scoped"]["test.start"]["reasons"], ["auth.scope.denied"])
            self.assertEqual(outcomes["unscoped"]["test.start"]["reasons"], ["policy.denied.explicit"])
            self.assertEqual(lab.sim.write_attempts, [], "a write reached the simulator")
            entries = lab.operator.audit_tail(1000)
            denied = [e for e in entries if e.kind == "command.transition"
                      and isinstance(e.data, dict) and e.data.get("to") == "DENIED"]
            self.assertEqual(len(denied), 6)
            principals = {e.data["principal"] for e in entries if e.kind == "command.created"}
            self.assertEqual(principals, {"agent:battery-example", "agent:unscoped"})
            self.assertTrue(lab.operator.audit_verify().valid)

    def test_the_simulator_refuses_writes_even_if_the_gateway_were_to_send_one(self) -> None:
        # Defence in depth, checked directly against the simulated instrument.
        sim = simulator.BatterySimulator(simulator.load_fixture("normal"))
        port = sim.start()
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=5) as s:
                reply = modbus(s, struct.pack(">BHH", 5, 0, 0xFF00))
            self.assertEqual(reply[1], simulator.ILLEGAL_FUNCTION)
            self.assertEqual(len(sim.write_attempts), 1)
        finally:
            sim.stop()

    def test_every_generated_answer_is_grounded_in_the_returned_data(self) -> None:
        answers = []
        for fixture in ("normal", "stale", "suspect", "malformed"):
            with BatteryLab(fixture) as lab, lab.mcp() as m:
                a = ag.ReadOnlyAgent(m)
                answers.append(ag.answer_snapshot(a.snapshot()))
                if fixture == "normal":
                    answers.append(ag.answer_status(a.status()))
        with BatteryLab("session") as lab, lab.mcp() as m:
            a = ag.ReadOnlyAgent(m)
            answers += [ag.answer_session_list(a.sessions()), ag.answer_session(a.session(1))]
        with BatteryLab("irregular_session") as lab, lab.mcp() as m:
            answers.append(ag.answer_session(ag.ReadOnlyAgent(m).session(1)))
        for ans in answers:
            self.assertEqual(ag.check_grounding(ans), [], ans.render())
        # and the grounded numbers really are the fixture's
        self.assertIn("3.912 V", answers[-3].render() + answers[-2].render())


if __name__ == "__main__":
    unittest.main(verbosity=2)
