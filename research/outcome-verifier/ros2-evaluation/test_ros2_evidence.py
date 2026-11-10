"""Tests for the ROS 2 evidence bridge. They write tiny MCAP files with the ROS 2 MCAP writer, so the converter is
tested on real CDR encoding and embedded message definitions. These files are test fixtures, not data.

    python -m unittest discover -s research/outcome-verifier/ros2-evaluation -p "test_*.py" -v

Needs `mcap` and `mcap-ros2-support` (see README.md).
"""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

try:
    from mcap_ros2.writer import Writer

    import ros2_evidence as bridge
except ImportError as e:  # pragma: no cover
    raise SystemExit(f"needs mcap and mcap-ros2-support: {e}")

ODOM = """std_msgs/Header header
string child_frame_id
geometry_msgs/PoseWithCovariance pose
================================================================================
MSG: std_msgs/Header
builtin_interfaces/Time stamp
string frame_id
================================================================================
MSG: builtin_interfaces/Time
int32 sec
uint32 nanosec
================================================================================
MSG: geometry_msgs/PoseWithCovariance
geometry_msgs/Pose pose
float64[36] covariance
================================================================================
MSG: geometry_msgs/Pose
geometry_msgs/Point position
geometry_msgs/Quaternion orientation
================================================================================
MSG: geometry_msgs/Point
float64 x
float64 y
float64 z
================================================================================
MSG: geometry_msgs/Quaternion
float64 x
float64 y
float64 z
float64 w"""

CLOCK = """builtin_interfaces/Time clock
================================================================================
MSG: builtin_interfaces/Time
int32 sec
uint32 nanosec"""

STRING = "string data"


def odom(sec: int, nanosec: int, x: float, frame: str = "map") -> dict:
    return {"header": {"stamp": {"sec": sec, "nanosec": nanosec}, "frame_id": frame}, "child_frame_id": "base_link",
            "pose": {"pose": {"position": {"x": x, "y": 0.0, "z": 0.0},
                              "orientation": {"x": 0.0, "y": 0.0, "z": 0.0, "w": 1.0}},
                     "covariance": [0.0] * 36}}


def write_bag(path: Path, with_clock: bool = False) -> None:
    with open(path, "wb") as f:
        w = Writer(f)
        o = w.register_msgdef("nav_msgs/msg/Odometry", ODOM)
        s = w.register_msgdef("std_msgs/msg/String", STRING)
        base = 1_000_000_000_000_000_000
        w.write_message("/odom", o, odom(1000, 0, 1.0), log_time=base, publish_time=base)
        w.write_message("/odom", o, odom(1000, 500_000_000, float("nan")), log_time=base + 10, publish_time=base + 10)
        w.write_message("/odom", o, odom(0, 0, 3.0), log_time=base + 20, publish_time=base + 20)
        w.write_message("/odom", o, odom(1001, 0, 4.0, frame="odom"), log_time=base + 30, publish_time=base + 30)
        w.write_message("/status", s, {"data": "SUCCEEDED"}, log_time=base + 40, publish_time=base + 40)
        if with_clock:
            c = w.register_msgdef("rosgraph_msgs/msg/Clock", CLOCK)
            w.write_message("/clock", c, {"clock": {"sec": 1000, "nanosec": 0}}, log_time=base, publish_time=base)
        w.finish()


MAPPING = {
    "source": {"system": "test", "provenance": "simulator"},
    "clock": {"domain": "test-host"},
    "mappings": [
        {"topic": "/odom", "signal": "base.x", "field": "pose.pose.position.x", "unit": "m", "kind": "device_state"},
        {"topic": "/status", "signal": "status", "field": "data", "kind": "controller"},
    ],
}


class BridgeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.bag = Path(self.tmp.name) / "t.mcap"
        write_bag(self.bag)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def convert(self, mapping=None):
        return bridge.convert(self.bag, json.loads(json.dumps(mapping or MAPPING)))

    def test_inspect_lists_topics_and_types(self):
        info = bridge.inspect(self.bag)
        self.assertEqual(info["topics"]["/odom"], {"type": "nav_msgs/msg/Odometry", "messages": 4})

    def test_header_stamp_is_the_source_time_and_receipt_is_kept_apart(self):
        records, _ = self.convert()
        first = next(r for r in records if r["id"] == "/odom#1:base.x")
        self.assertEqual(first["time"]["source"], "1970-01-01T00:16:40.000Z")
        self.assertEqual(first["time"]["ingest"][:4], "2001")
        self.assertEqual(first["value"], {"value": 1.0, "unit": "m"})
        self.assertEqual(first["frame"], "map")
        self.assertEqual(first["source"]["provenance"], "simulator")

    def test_nan_is_skipped_not_filled(self):
        records, meta = self.convert()
        self.assertNotIn("/odom#2:base.x", [r["id"] for r in records])
        self.assertEqual(meta["counts"]["base.x: not a finite value"], 1)

    def test_a_zero_stamp_gives_no_source_time(self):
        records, meta = self.convert()
        third = next(r for r in records if r["id"] == "/odom#3:base.x")
        self.assertNotIn("source", third["time"])
        self.assertEqual(meta["counts"]["base.x: no source stamp"], 1, "the zero stamp")
        self.assertEqual(meta["counts"]["status: no source stamp"], 1, "a message with no header")

    def test_the_frame_is_kept_as_recorded(self):
        records, _ = self.convert()
        self.assertEqual(next(r for r in records if r["id"] == "/odom#4:base.x")["frame"], "odom")

    def test_log_time_is_used_only_when_asked_and_says_so(self):
        m = json.loads(json.dumps(MAPPING))
        m["mappings"][1]["time"] = "log_time"
        records, _ = self.convert(m)
        status = next(r for r in records if r["signal"] == "status")
        self.assertIn("source", status["time"])
        self.assertEqual(status["time"]["clock_domain"], "test-host/recorder")

    def test_unknown_provenance_stays_unknown(self):
        m = json.loads(json.dumps(MAPPING))
        del m["source"]["provenance"]
        records, _ = self.convert(m)
        self.assertNotIn("provenance", records[0]["source"])

    def test_a_topic_not_in_the_bag_is_refused(self):
        m = json.loads(json.dumps(MAPPING))
        m["mappings"][0]["topic"] = "/nope"
        with self.assertRaises(SystemExit):
            self.convert(m)

    def test_simulation_time_must_be_declared(self):
        write_bag(self.bag, with_clock=True)
        with self.assertRaisesRegex(SystemExit, "/clock"):
            self.convert()
        m = json.loads(json.dumps(MAPPING))
        m["clock"]["domain"] = "sim:test"
        records, _ = self.convert(m)
        self.assertEqual(records[0]["time"]["clock_domain"], "sim:test")

    def test_the_same_bag_gives_the_same_records(self):
        a, meta_a = self.convert()
        b, meta_b = self.convert()
        self.assertEqual(json.dumps(a), json.dumps(b))
        self.assertEqual(meta_a["bag_sha256"], meta_b["bag_sha256"])


if __name__ == "__main__":
    unittest.main()
