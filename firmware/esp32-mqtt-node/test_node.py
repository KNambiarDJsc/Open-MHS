"""Experiment 02: the ESP32 MQTT node firmware (running in Espressif's QEMU) with Mosquitto
and an OpenMHS gateway.

    python -m unittest discover -s firmware/esp32-mqtt-node -v

Needs mosquitto, mosquitto_pub, mosquitto_sub (PATH or OPENMHS_MOSQUITTO_DIR), OPENMHS_QEMU
(qemu-system-xtensa), OPENMHS_FLASH_IMAGE (the merged flash image; README.md) and, for the
gateway tests, OPENMHS_BIN (the openmhs binary). Anything missing skips the tests that need
it. Everything stays on loopback; no hardware and no external service is involved.

The firmware is real, compiled ESP-IDF code running on an emulated ESP32. What is emulated,
not physical: the CPU, the Ethernet MAC, the network path, the LED (a GPIO level nothing
reads). Nothing here says anything about Wi-Fi radio behaviour, a real board's timing or power.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import harness as h  # noqa: E402

BIN = os.environ.get("OPENMHS_BIN")
MISSING = h.available()
DEVICE = "esp32.node-01"
PROFILE = HERE / "profile.json"

#: What the run observed that the tests only constrain loosely; saved by the demo and the report.
OBSERVED: dict = {}


def parse_time(text: str) -> float:
    return datetime.strptime(text, "%Y-%m-%dT%H:%M:%S.%fZ").replace(tzinfo=timezone.utc).timestamp()


def wait_for(predicate, timeout: float, what: str, interval: float = 0.5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(interval)
    raise AssertionError(f"timed out after {timeout:g} s waiting for {what}")


@unittest.skipIf(MISSING, f"firmware rig unavailable: {MISSING}")
class FirmwareTests(unittest.TestCase):
    """The firmware against a plain MQTT client: no OpenMHS involved."""

    @classmethod
    def setUpClass(cls):
        cls.rig = h.Rig().__enter__()
        cls.broker = cls.rig.broker

    @classmethod
    def tearDownClass(cls):
        cls.rig.__exit__()

    def test_1_state_is_retained_and_carries_the_nodes_own_clock(self):
        a = self.broker.state()
        time.sleep(2.2)
        b = self.broker.state()  # a late subscriber gets the retained message
        self.assertIsNotNone(a)
        self.assertGreater(b["uptime_s"], a["uptime_s"])
        self.assertLess(abs(parse_time(b["time"]) - time.time()), 4.0, "node clock vs host clock")
        self.assertGreaterEqual(self.rig.clock.requests, 1, "the node asked for the time")

    def test_2_a_led_command_is_acknowledged_and_visible_in_the_state(self):
        ack = self.broker.command("set", {"led": True, "id": "t2-on"})
        self.assertEqual(ack, {"id": "t2-on", "ok": True})
        wait_for(lambda: self.broker.state(2)["led"] is True, 10, "led true in the state")
        self.assertIn("led -> on (command t2-on)", self.rig.node.serial())
        self.assertEqual(self.broker.command("set", {"led": False, "id": "t2-off"}), {"id": "t2-off", "ok": True})
        wait_for(lambda: self.broker.state(2)["led"] is False, 10, "led false in the state")

    def test_3_bad_commands_are_refused_or_ignored_and_change_nothing(self):
        before = self.broker.state()
        for payload in ({"led": "yes", "id": "t3a"}, {"led": 1, "id": "t3b"}, {"id": "t3c"}):
            ack = self.broker.command("set", payload)
            self.assertEqual((ack or {}).get("ok"), False, payload)
            self.assertEqual(ack["id"], payload["id"])
        for payload in ("not json", "[1,2]", json.dumps({"led": True}), json.dumps({"led": True, "id": 7})):
            self.assertIsNone(self.broker.command("set", payload, wait=3), payload)
        after = self.broker.state()
        self.assertEqual((after["led"], after["pulses"]), (before["led"], before["pulses"]))

    def test_4_pulses_count_once_per_message_and_are_not_idempotent(self):
        start = self.broker.state()["pulses"]
        for n in (0, 11, 1.5, "3", None):
            ack = self.broker.command("pulse", {"n": n, "id": "t4bad"})
            self.assertEqual((ack or {}).get("ok"), False, n)
        self.assertEqual(self.broker.state()["pulses"], start)
        self.assertEqual(self.broker.command("pulse", {"n": 3, "id": "t4"}), {"id": "t4", "ok": True})
        # The same id again counts again: the firmware keeps no record of ids. That is why
        # the gateway never retries a non-idempotent command.
        self.assertEqual(self.broker.command("pulse", {"n": 3, "id": "t4"}), {"id": "t4", "ok": True})
        wait_for(lambda: self.broker.state(2)["pulses"] == start + 6, 10, "6 more pulses")

    def test_5_acknowledgement_uses_the_commands_id_for_overlapping_commands(self):
        ids = [f"t5-{i}" for i in range(5)]
        with h.Tap(self.broker, f"{h.PREFIX}/ack") as tap:
            for i in ids:
                self.broker.publish(f"{h.PREFIX}/set", json.dumps({"led": True, "id": i}))
            wait_for(lambda: len(tap.messages) >= 5, 15, "five acknowledgements")
        self.assertEqual(sorted(a["id"] for a in tap.on("ack")), sorted(ids))


@unittest.skipIf(MISSING or not BIN or not Path(BIN or "").exists(), f"gateway rig unavailable: {MISSING or 'set OPENMHS_BIN'}")
class ThroughTheGateway(unittest.TestCase):
    """OpenMHS in front of the firmware. Numbered: the grants and the state build on each other."""

    @classmethod
    def setUpClass(cls):
        cls.home = Path(tempfile.mkdtemp(prefix="openmhs-esp32-gw-"))
        (cls.home / "rig").mkdir()
        cls.rig = h.Rig(cls.home / "rig").__enter__()
        cls.broker, cls.node = cls.rig.broker, cls.rig.node
        env = dict(os.environ, OPENMHS_HOME=str(cls.home / "gw"))
        env.pop("OPENMHS_TOKEN", None)
        cls.env = env
        cls.port = h._free_port()
        cls.log = open(cls.home / "gateway.log", "wb")
        cls.gateway = subprocess.Popen(
            [BIN, "serve", "--profile", str(PROFILE), "--port", str(cls.port)], env=env, stdout=cls.log, stderr=subprocess.STDOUT
        )
        wait_for(lambda: cls.cli("devices", "list", token=None)[0] == 0, 60, "the gateway")
        rc, out = cls.cli("tokens", "create", "agent:demo", "--any-scope", token=None)
        cls.agent = re.search(r"omhs_[A-Za-z0-9_\-]+", out).group(0)

    @classmethod
    def tearDownClass(cls):
        cls.gateway.terminate()
        try:
            cls.gateway.wait(timeout=15)
        except subprocess.TimeoutExpired:
            cls.gateway.kill()
        cls.log.close()
        keep = os.environ.get("OPENMHS_KEEP_LOGS")
        if keep:
            Path(keep).mkdir(parents=True, exist_ok=True)
            cls.node.serial()
            for src, name in ((cls.home / "rig" / "qemu-serial.log", "node-serial.log"), (cls.home / "gateway.log", "gateway.log"),
                              (cls.broker.dir / "mosquitto.log", "mosquitto.log")):
                if src.exists():
                    shutil.copyfile(src, Path(keep) / name)
        cls.rig.__exit__()
        shutil.rmtree(cls.home, ignore_errors=True)

    @classmethod
    def cli(cls, *args, token="agent"):
        env = dict(cls.env)
        if token == "agent":
            env["OPENMHS_TOKEN"] = cls.agent
        p = subprocess.run([BIN, *args], env=env, capture_output=True, text=True, timeout=120)
        return p.returncode, p.stdout + p.stderr

    def read(self, cap):
        rc, out = self.cli("--json", "read", DEVICE, cap)
        return rc, (json.loads(out) if rc == 0 else out)

    def exec(self, cap, *args):
        rc, out = self.cli("exec", DEVICE, cap, *args)
        state = re.search(r"^state\s+(\w+)", out, re.M)
        cid = re.search(r"^command\s+(\S+)", out, re.M)
        return state.group(1) if state else None, cid.group(1) if cid else None, out

    def wait_recovered(self, timeout=120):
        """Until the node is back on the broker and the gateway reads fresh, usable data."""
        def ok():
            rc, r = self.read("led.on")
            return rc == 0 and r["usable"]
        wait_for(ok, timeout, "the node to recover", interval=2)

    def grant(self, gid, ops):
        rc, out = self.cli("grants", "add", "--id", gid, "--principal", "agent:demo", "--device", DEVICE,
                           "--capability", "*", "--ops", ops, "--effect", "allow", token=None)
        self.assertEqual(rc, 0, out)

    # ---- discovery and reads ----

    def test_01_the_device_is_discovered_with_typed_capabilities(self):
        rc, out = self.cli("--json", "devices", "list", token=None)
        self.assertEqual(rc, 0, out)
        d = next(x for x in json.loads(out) if x["device"]["id"] == DEVICE)
        caps = {c["name"]: c for c in d["capabilities"]}
        self.assertEqual(set(caps), {"led.on", "node.uptime", "led.pulses", "led.pulse"})
        self.assertEqual(caps["led.on"]["operations"], ["read", "write"])
        self.assertEqual(caps["led.pulse"]["operations"], ["invoke"])

    def test_02_no_grant_means_no_message_reaches_the_node(self):
        with h.Tap(self.broker, f"{h.PREFIX}/+") as tap:
            rc, out = self.cli("read", DEVICE, "led.on", token=None)
            self.assertIn("policy.denied.no_grant", out)
            state, _, out = self.exec("led.on", "--value", "true")
            self.assertEqual(state, "DENIED", out)
            self.assertIn("nothing sent to the device", out)
            time.sleep(1.5)
        self.assertEqual(tap.on("set") + tap.on("pulse"), [], "a denied command reached the broker")
        self.assertEqual(tap.on("ack"), [])

    def test_03_granted_reads_come_from_the_node_with_the_nodes_timestamp(self):
        self.grant("agent-read", "read")
        for cap in ("led.on", "node.uptime", "led.pulses"):
            rc, r = self.read(cap)
            self.assertEqual(rc, 0, r)
            self.assertEqual((r["state"]["quality"], r["state"]["source"], r["usable"]), ("GOOD", "device", True), cap)
            self.assertLess(abs(parse_time(r["state"]["timestamp"]) - time.time()), 4.5, cap)
        rc, r = self.read("node.uptime")
        self.assertEqual(r["state"]["value"]["unit"], "s")

    # ---- commands ----

    def test_04_a_write_is_acknowledged_applied_and_verified_by_readback(self):
        self.grant("agent-act", "write,invoke")
        state, cid, out = self.exec("led.on", "--value", "true")
        self.assertEqual(state, "VERIFIED", out)
        self.assertIn("device   acknowledged; reported applied", out)
        self.assertIn("MATCH", out)
        self.assertIn(f"(command {cid})", self.node.serial(), "the node logged this command id")
        self.assertIs(self.broker.state()["led"], True)
        state, _, out = self.exec("led.on", "--value", "false")
        self.assertEqual(state, "VERIFIED", out)
        self.assertIs(self.broker.state()["led"], False)

    def test_05_a_value_outside_the_declared_range_never_leaves_the_gateway(self):
        before = self.broker.state()["pulses"]
        with h.Tap(self.broker, f"{h.PREFIX}/+") as tap:
            for n in ("0", "11", "-3"):
                state, _, out = self.exec("led.pulse", "--param", f"n={n}")
                self.assertEqual(state, "DENIED", f"{n}: {out}")
            # Not an integer at all: refused before a command exists.
            rc, out = self.cli("exec", DEVICE, "led.pulse", "--param", "n=2.5")
            self.assertNotEqual(rc, 0)
            self.assertIn("command.schema", out)
            time.sleep(1.5)
        self.assertEqual(tap.on("pulse"), [])
        self.assertEqual(self.broker.state()["pulses"], before)

    def test_06_an_action_runs_exactly_once(self):
        before = self.broker.state()["pulses"]
        with h.Tap(self.broker, f"{h.PREFIX}/+") as tap:
            state, cid, out = self.exec("led.pulse", "--param", "n=2")
            time.sleep(1.5)
        self.assertEqual(state, "UNVERIFIABLE", out)  # acknowledged; the profile declares no readback for it
        self.assertEqual(len(tap.on("pulse")), 1, "one request on the wire")
        wait_for(lambda: self.broker.state(2)["pulses"] == before + 2, 10, "the counter")

    # ---- faults ----
    #
    # The node is made to misbehave in ways a deployed one does: powered off, hung, behind a
    # dead broker. (QEMU's emulated Ethernet link did not reliably come back after being
    # cut, so a "cable pulled" fault is not tested; see REPORT.md.)

    def test_07_a_command_to_a_powered_off_node_is_uncertain_and_not_retried(self):
        self.wait_recovered()
        self.node.kill()
        try:
            with h.Tap(self.broker, f"{h.PREFIX}/+") as tap:
                state, cid, out = self.exec("led.pulse", "--param", "n=1")
                time.sleep(1)
            self.assertEqual(state, "UNKNOWN_OUTCOME", out)
            self.assertIn("adapter.timeout", out)
            self.assertIn("not retried", out)
            self.assertEqual(len(tap.on("pulse")), 1, "sent once, never again")
        finally:
            self.node.power_cycle()
            self.rig.clock.sync_now()
        self.wait_recovered()
        self.assertEqual(self.broker.state()["pulses"], 0, "a fresh boot: the command was not applied late")
        # The action declares no readback, so there is nothing to reconcile it against: the
        # gateway says so and leaves the decision to an operator instead of guessing.
        rc, out = self.cli("commands", "reconcile", cid, token=None)
        self.assertNotEqual(rc, 0)
        self.assertIn("command.not_reconcilable", out)
        rc, out = self.cli("commands", "show", cid, token=None)
        self.assertRegex(out, r"state\s+UNKNOWN_OUTCOME")

    def test_08_a_hung_node_s_readings_age_out_instead_of_staying_good(self):
        self.wait_recovered()
        self.node.pause()
        try:
            time.sleep(6)
            rc, out = self.cli("read", DEVICE, "led.on")
            self.assertIn("STALE", out, out)
            rc, r = self.read("led.on")
            self.assertEqual(rc, 0, r)
            self.assertFalse(r["usable"], "a stale reading must not be usable")
            self.assertEqual(r["state"]["quality"], "GOOD", "the value is old, not wrong: the flag is what protects the caller")
        finally:
            self.node.resume()
            self.rig.clock.sync_now()  # the paused machine lost its clock time
        self.wait_recovered()

    def test_09_a_write_to_a_hung_node_reconciles_to_what_the_node_did(self):
        self.wait_recovered()
        self.node.pause()
        try:
            state, cid, out = self.exec("led.on", "--value", "true")
            self.assertEqual(state, "UNKNOWN_OUTCOME", out)
        finally:
            self.node.resume()
            self.rig.clock.sync_now()
        self.wait_recovered()
        time.sleep(3)
        actual = self.broker.state()["led"]
        OBSERVED["write_applied_after_hang"] = actual
        rc, out = self.cli("commands", "reconcile", cid, token=None)
        OBSERVED["reconcile_after_hang"] = out
        self.assertEqual(rc, 0, out)
        rc, show = self.cli("commands", "show", cid, token=None)
        final = re.search(r"^state\s+(\w+)", show, re.M).group(1)
        self.assertIn(final, ("RECONCILED_APPLIED", "RECONCILED_NOT_APPLIED"), show)
        # Whichever way it went, the gateway's conclusion matches the node.
        self.assertEqual(final == "RECONCILED_APPLIED", actual is True, f"gateway says {final}, node says led={actual}\n{show}")
        self.exec("led.on", "--value", "false")

    def test_10_an_action_sent_to_a_hung_node_runs_at_most_once(self):
        self.wait_recovered()
        before = self.broker.state()["pulses"]
        self.node.pause()
        try:
            with h.Tap(self.broker, f"{h.PREFIX}/+") as tap:
                state, cid, out = self.exec("led.pulse", "--param", "n=1")
                time.sleep(1)
            self.assertEqual(state, "UNKNOWN_OUTCOME", out)
            self.assertEqual(len(tap.on("pulse")), 1)
        finally:
            self.node.resume()
            self.rig.clock.sync_now()
        self.wait_recovered()
        time.sleep(3)
        delta = self.broker.state()["pulses"] - before
        OBSERVED["pulse_applied_after_hang"] = delta
        self.assertIn(delta, (0, 1), "applied late at most once; never twice")

    def test_11_broker_down_reads_fail_loudly_and_a_command_is_not_sent(self):
        self.wait_recovered()
        self.broker.stop()
        try:
            time.sleep(2)
            rc, out = self.cli("read", DEVICE, "led.on")
            self.assertNotEqual(rc, 0)
            self.assertIn("adapter.disconnected", out)
            state, _, out = self.exec("led.on", "--value", "true")
            self.assertEqual(state, "FAILED", out)
            self.assertIn("adapter.not_sent", out)
        finally:
            self.broker.start()
        self.rig.clock.stop()
        self.rig.clock.start()  # its subscription died with the broker
        self.wait_recovered(timeout=180)
        self.assertIs(self.broker.state()["led"], False, "the refused command must not have been applied")

    def test_12_a_node_reboot_is_visible_in_the_readings(self):
        self.wait_recovered()
        self.exec("led.pulse", "--param", "n=1")
        rc, before = self.read("node.uptime")
        self.node.power_cycle()
        self.rig.clock.sync_now()
        self.wait_recovered(timeout=180)
        rc, after = self.read("node.uptime")
        self.assertLess(after["state"]["value"]["value"], before["state"]["value"]["value"], "uptime restarts at a reboot")
        rc, pulses = self.read("led.pulses")
        self.assertEqual(pulses["state"]["value"], 0, "the counter does not survive a reboot")

    def test_13_the_audit_log_verifies(self):
        rc, out = self.cli("audit", "verify", token=None)
        self.assertEqual(rc, 0, out)
        self.assertIn("intact", out)


if __name__ == "__main__":
    unittest.main()
