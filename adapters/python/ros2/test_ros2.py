"""The ROS 2 adapter's refusals and conversions. Needs rclpy, so run it in the image:

    docker run --rm --network none -v "$PWD/adapters/python/ros2:/openmhs/adapters/python/ros2:ro" \
      -v "$PWD/python:/openmhs/python:ro" ros@sha256:066420e0… \
      python3 -m unittest discover -s /openmhs/adapters/python/ros2 -v
"""

import copy
import json
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import ros2_adapter as ra  # noqa: E402
from openmhs.adapter import AdapterError  # noqa: E402
from diagnostic_msgs.msg import DiagnosticArray, DiagnosticStatus  # noqa: E402
from sensor_msgs.msg import BatteryState, JointState  # noqa: E402


def config():
    return copy.deepcopy(json.loads((HERE / "manifest.sim.json").read_text(encoding="utf-8"))["config"])


class SettingsTests(unittest.TestCase):
    def test_the_simulator_manifest_is_accepted(self):
        s = ra.Settings(config())
        self.assertEqual(s.fields["battery.charge"], ("/battery_state", "percentage"))

    def test_only_observation_is_accepted(self):
        c = config()
        c["descriptor"]["capabilities"][0]["operations"] = ["read", "write"]
        with self.assertRaisesRegex(ra.ConfigError, "only observes"):
            ra.Settings(c)

    def test_mistakes_are_refused(self):
        cases = {
            "relative topic": lambda c: c["topics"].update({"battery": c["topics"].pop("/battery_state")}),
            "unknown key": lambda c: c["topics"]["/battery_state"].update({"rate": 10}),
            "unknown type": lambda c: c["topics"]["/battery_state"].update({"type": "nope_msgs/msg/Thing"}),
            "unmapped property": lambda c: c["topics"]["/battery_state"]["fields"].pop("battery.voltage"),
            "unknown capability": lambda c: c["topics"]["/battery_state"]["fields"].update({"x.y": "voltage"}),
            "unknown config key": lambda c: c.update({"publish": True}),
        }
        for what, change in cases.items():
            with self.subTest(what):
                c = config()
                change(c)
                with self.assertRaises(ra.ConfigError):
                    ra.Settings(c)


class ValueTests(unittest.TestCase):
    def test_fields_and_stamps(self):
        msg = JointState()
        msg.name = ["wheel_left", "wheel_right"]
        msg.position = [0.5, 1.5]
        self.assertEqual(ra.field(msg, "position.1"), 1.5)
        self.assertIsNone(ra.stamp(msg))  # unset stamp: the adapter uses receive time
        msg.header.stamp.sec = 1_800_000_000
        msg.header.stamp.nanosec = 250_000_000
        self.assertEqual(ra.stamp(msg), "2027-01-15T08:00:00.250Z")

    def test_nan_means_not_measured(self):
        msg = BatteryState()
        msg.temperature = float("nan")  # BatteryState's convention for "not measured"
        with self.assertRaises(AdapterError) as e:
            ra.convert(ra.field(msg, "temperature"), {"type": "number", "unit": "Cel"}, "t")
        self.assertEqual(e.exception.kind, "device_failure")

    def test_types_follow_the_descriptor(self):
        self.assertEqual(ra.convert(24, {"type": "number", "unit": "V"}, "v"), {"value": 24.0, "unit": "V"})
        for value, spec in [(True, {"type": "number"}), (1.5, {"type": "boolean"}), ("x", {"type": "enum", "values": ["y"]})]:
            with self.subTest(value=value), self.assertRaises(AdapterError):
                ra.convert(value, spec, "f")

    def test_a_value_outside_the_declared_range_is_refused(self):
        spec = {"type": "number", "unit": "rad", "range": {"min": -6.3, "max": 6.3}}
        self.assertEqual(ra.convert(6.3, spec, "j"), {"value": 6.3, "unit": "rad"})
        with self.assertRaises(AdapterError) as e:
            ra.convert(1000.0, spec, "j")
        self.assertEqual(e.exception.kind, "malformed_response")
        with self.assertRaises(AdapterError):
            ra.convert(4, {"type": "integer", "range": {"min": 0, "max": 3}}, "level")

    def test_infinity_is_not_a_measurement(self):
        with self.assertRaises(AdapterError) as e:
            ra.convert(float("inf"), {"type": "number", "unit": "m"}, "x")
        self.assertEqual(e.exception.kind, "device_failure")


class SelectorTests(unittest.TestCase):
    def joints(self, names, positions):
        msg = JointState()
        msg.name = names
        msg.position = positions
        return msg

    def test_joints_are_found_by_name_whatever_the_order(self):
        path = "position[name=elbow]"
        self.assertEqual(ra.field(self.joints(["a", "elbow"], [1.0, 2.0]), path), 2.0)
        self.assertEqual(ra.field(self.joints(["elbow", "a"], [2.0, 1.0]), path), 2.0)

    def test_what_cannot_be_identified_is_refused(self):
        path = "position[name=elbow]"
        cases = {
            "absent": (["a", "b"], [1.0, 2.0]),
            "no positions": (["a", "elbow"], []),
            "too few positions": (["a", "b", "elbow"], [1.0, 2.0]),
            "twice": (["elbow", "elbow"], [1.0, 2.0]),
        }
        for what, (names, positions) in cases.items():
            with self.subTest(what), self.assertRaises(LookupError):
                ra.field(self.joints(names, positions), path)

    def test_a_list_of_messages_and_a_byte_field(self):
        msg = DiagnosticArray()
        msg.status = [DiagnosticStatus(name="other"), DiagnosticStatus(name="driver", level=DiagnosticStatus.ERROR)]
        self.assertEqual(ra.field(msg, "status[name=driver].level"), 2)
        self.assertEqual(ra.convert(ra.field(msg, "status[name=driver].level"), {"type": "integer"}, "l"), 2)
        with self.assertRaises(LookupError):
            ra.field(DiagnosticArray(), "status[name=driver].level")

    def test_a_bad_path_is_refused_at_start(self):
        c = config()
        c["topics"]["/battery_state"]["fields"]["battery.voltage"] = "cells[name=]"
        with self.assertRaisesRegex(ra.ConfigError, "bad path part"):
            ra.Settings(c)


class OrderTests(unittest.TestCase):
    """The newest message by stamp is the one kept, without any ROS traffic."""

    class Fake:
        def __init__(self, stamp_seconds):
            self.header = JointState().header
            self.header.stamp.sec = stamp_seconds

    def observer(self):
        o = ra.Observer.__new__(ra.Observer)
        o.latest, o.newest_stamp, o.older = {}, {}, {}
        o.lock = ra.threading.Lock()
        return o, o._keep("/t")

    def test_an_older_message_does_not_replace_a_newer_one(self):
        o, keep = self.observer()
        now = int(ra.time.time())
        keep(self.Fake(now))
        keep(self.Fake(now - 1))
        self.assertEqual(o.newest("/t")[0].header.stamp.sec, now)
        self.assertEqual(o.refused("/t"), 1)
        keep(self.Fake(now))
        self.assertEqual(o.refused("/t"), 0)

    def test_one_future_stamp_does_not_hide_what_follows(self):
        o, keep = self.observer()
        now = int(ra.time.time())
        keep(self.Fake(now))
        keep(self.Fake(now + 3600))
        self.assertEqual(o.newest("/t")[0].header.stamp.sec, now + 3600)  # kept, for the gateway to refuse
        keep(self.Fake(now + 1))
        self.assertEqual(o.newest("/t")[0].header.stamp.sec, now + 1)

    def test_an_unset_stamp_is_accepted(self):
        o, keep = self.observer()
        keep(self.Fake(0))
        self.assertIsNotNone(o.newest("/t"))
        self.assertIsNone(ra.stamp(o.newest("/t")[0]))


class ControlSurfaceTests(unittest.TestCase):
    def test_the_observer_creates_no_way_to_command_anything(self):
        """Publishers belong to the conformance-only RobotSim. The observer makes subscriptions only."""
        import ast

        tree = ast.parse((HERE / "ros2_adapter.py").read_text(encoding="utf-8"))
        forbidden = {"create_publisher", "create_client", "ActionClient", "ActionServer", "create_service"}
        allowed = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "RobotSim")
        inside = set(map(id, ast.walk(allowed)))
        found = [
            n.lineno
            for n in ast.walk(tree)
            if isinstance(n, ast.Call) and id(n) not in inside
            and (getattr(n.func, "attr", None) in forbidden or getattr(n.func, "id", None) in forbidden)
        ]
        self.assertEqual(found, [])


if __name__ == "__main__":
    unittest.main()
