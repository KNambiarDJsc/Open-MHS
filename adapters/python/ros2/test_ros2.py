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


if __name__ == "__main__":
    unittest.main()
