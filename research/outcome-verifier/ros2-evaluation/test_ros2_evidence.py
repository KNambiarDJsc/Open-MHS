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


TIME = """================================================================================
MSG: builtin_interfaces/Time
int32 sec
uint32 nanosec"""

STATUS_ARRAY = """action_msgs/GoalStatus[] status_list
================================================================================
MSG: action_msgs/GoalStatus
action_msgs/GoalInfo goal_info
int8 status
================================================================================
MSG: action_msgs/GoalInfo
unique_identifier_msgs/UUID goal_id
builtin_interfaces/Time stamp
================================================================================
MSG: unique_identifier_msgs/UUID
uint8[16] uuid
""" + TIME

TF = """geometry_msgs/TransformStamped[] transforms
================================================================================
MSG: geometry_msgs/TransformStamped
std_msgs/Header header
string child_frame_id
geometry_msgs/Transform transform
================================================================================
MSG: std_msgs/Header
builtin_interfaces/Time stamp
string frame_id
================================================================================
MSG: geometry_msgs/Transform
geometry_msgs/Vector3 translation
geometry_msgs/Quaternion rotation
================================================================================
MSG: geometry_msgs/Vector3
float64 x
float64 y
float64 z
================================================================================
MSG: geometry_msgs/Quaternion
float64 x
float64 y
float64 z
float64 w
""" + TIME

G1, G2 = list(range(16)), [255] * 16
G1_TEXT = "00010203-0405-0607-0809-0a0b0c0d0e0f"


def status(*goals: tuple[list[int], int]) -> dict:
    return {"status_list": [{"goal_info": {"goal_id": {"uuid": g}, "stamp": {"sec": 0, "nanosec": 0}}, "status": s}
                            for g, s in goals]}


def transform(sec: int, nanosec: int, parent: str, child: str, z: float) -> dict:
    return {"header": {"stamp": {"sec": sec, "nanosec": nanosec}, "frame_id": parent}, "child_frame_id": child,
            "transform": {"translation": {"x": 0.0, "y": 0.0, "z": z},
                          "rotation": {"x": 0.0, "y": 0.0, "z": 0.0, "w": 1.0}}}


def write_sim_bag(path: Path, clock_back: bool = False) -> None:
    """A simulation bag: /clock every 100 ms of receipt time and 10 ms of simulation time, an action status topic
    (no header) and a pose list."""
    base = 1_000_000_000_000_000_000
    with open(path, "wb") as f:
        w = Writer(f)
        c = w.register_msgdef("rosgraph_msgs/msg/Clock", CLOCK)
        s = w.register_msgdef("action_msgs/msg/GoalStatusArray", STATUS_ARRAY)
        t = w.register_msgdef("tf2_msgs/msg/TFMessage", TF)
        w.write_message("/status", s, status((G1, 1)), log_time=base - 50_000_000, publish_time=base)
        for k in range(5):
            sim_ns = 10_000_000_000 + k * 10_000_000
            if clock_back and k == 3:
                sim_ns = 9_000_000_000
            w.write_message("/clock", c, {"clock": {"sec": sim_ns // 10**9, "nanosec": sim_ns % 10**9}},
                            log_time=base + k * 100_000_000, publish_time=base)
        for k, goals in enumerate([[(G1, 2)], [(G1, 2)], [(G1, 4), (G2, 1)]]):
            log = base + 50_000_000 + k * 100_000_000
            w.write_message("/status", s, status(*goals), log_time=log, publish_time=log)
        w.write_message("/gz/dynamic_pose", t, {"transforms": [
            transform(10, 5_000_000, "gz:world", "openamrobot", 0.002),
            transform(10, 5_000_000, "gz:openamrobot", "lift_carriage_link", 0.834)]}, log_time=base, publish_time=base)
        w.finish()


SIM_MAPPING = {
    "source": {"system": "test", "provenance": "simulator"},
    "clock": {"domain": "sim:test", "uncertainty_ms": 0, "receipt_allowance_ms": 20},
    "mappings": [
        {"topic": "/status", "signal": "lift.action.status", "each": "status_list", "field": "status",
         "enum": {"1": "ACCEPTED", "2": "EXECUTING", "4": "SUCCEEDED"}, "ref_fields": {"attempt": "goal_info.goal_id.uuid"},
         "changes_only": True, "time": "clock", "kind": "controller", "device": "lift_controller"},
        {"topic": "/gz/dynamic_pose", "signal": "carriage.z", "each": "transforms",
         "where": {"child_frame_id": "lift_carriage_link"}, "field": "transform.translation.z", "unit": "m",
         "kind": "independent_sensor", "device": "gz-physics"},
    ],
}


class SimulationBagTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.bag = Path(self.tmp.name) / "sim.mcap"
        write_sim_bag(self.bag)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def convert(self, mapping=None):
        return bridge.convert(self.bag, json.loads(json.dumps(mapping or SIM_MAPPING)))

    def statuses(self, records):
        return [(r["refs"]["attempt"], r["value"]) for r in records if r["signal"] == "lift.action.status"]

    def test_an_action_status_array_becomes_one_record_per_transition(self):
        records, meta = self.convert()
        got = sorted(self.statuses(records))
        self.assertEqual(got, sorted([(G1_TEXT, "ACCEPTED"), (G1_TEXT, "EXECUTING"), (G1_TEXT, "SUCCEEDED"),
                                      ("ffffffff-ffff-ffff-ffff-ffffffffffff", "ACCEPTED")]))
        self.assertEqual(meta["counts"]["lift.action.status: unchanged"], 1, "the repeated EXECUTING")

    def test_an_unstamped_message_is_placed_between_clock_messages_with_an_uncertainty(self):
        records, _ = self.convert()
        done = next(r for r in records if r.get("value") == "SUCCEEDED")
        # received midway between /clock 10.020 s and 10.030 s
        self.assertEqual(done["time"]["source"], "1970-01-01T00:00:10.025Z")
        self.assertEqual(done["time"]["uncertainty_ms"], 25.0, "half of 10 ms, plus the 20 ms allowance")
        self.assertEqual(done["time"]["clock_domain"], "sim:test/recorder")

    def test_without_a_clock_message_before_it_there_is_no_source_time(self):
        records, meta = self.convert()
        first = next(r for r in records if r.get("value") == "ACCEPTED" and r["refs"]["attempt"] == G1_TEXT)
        self.assertNotIn("source", first["time"])
        self.assertNotIn("uncertainty_ms", first["time"])
        self.assertEqual(meta["counts"]["lift.action.status: no source stamp"], 1)

    def test_a_clock_that_goes_back_gives_no_placement(self):
        write_sim_bag(self.bag, clock_back=True)
        records, _ = self.convert()
        succeeded = next(r for r in records if r.get("value") == "SUCCEEDED")
        self.assertNotIn("source", succeeded["time"], "between /clock 10.020 s and 9.000 s nothing is honest")

    def test_one_element_of_a_pose_list_is_selected_with_its_own_header(self):
        records, _ = self.convert()
        z = [r for r in records if r["signal"] == "carriage.z"]
        self.assertEqual(len(z), 1, "the model's own entry is not the carriage")
        self.assertEqual(z[0]["frame"], "gz:openamrobot")
        self.assertEqual(z[0]["value"], {"value": 0.834, "unit": "m"})
        self.assertEqual(z[0]["time"], {"source": "1970-01-01T00:00:10.005Z", "ingest": z[0]["time"]["ingest"],
                                        "clock_domain": "sim:test", "uncertainty_ms": 0})

    def _bag_with_an_empty_schema(self, type_name: str) -> None:
        """As rosbag2 recorded the hidden action status topic: a schema with the type name and no definition."""
        from mcap.writer import Writer as RawWriter
        from mcap_ros2._dynamic import serialize_dynamic

        encode = serialize_dynamic("action_msgs/msg/GoalStatusArray", STATUS_ARRAY)["action_msgs/msg/GoalStatusArray"]
        base = 1_000_000_000_000_000_000
        with open(self.bag, "wb") as f:
            w = RawWriter(f)
            w.start("ros2")
            c = w.register_channel("/clock", "cdr", w.register_schema("rosgraph_msgs/msg/Clock", "ros2msg", CLOCK.encode()))
            clock_enc = serialize_dynamic("rosgraph_msgs/msg/Clock", CLOCK)["rosgraph_msgs/msg/Clock"]
            s = w.register_channel("/status", "cdr", w.register_schema(type_name, "", b""))
            w.add_message(c, log_time=base, data=clock_enc({"clock": {"sec": 10, "nanosec": 0}}), publish_time=base)
            w.add_message(s, log_time=base + 1, data=encode(status((G1, 4))), publish_time=base + 1)
            w.add_message(c, log_time=base + 2, data=clock_enc({"clock": {"sec": 10, "nanosec": 1_000_000}}),
                          publish_time=base + 2)
            w.finish()

    def test_an_empty_action_status_schema_is_decoded_with_a_supplied_definition_and_says_so(self):
        self._bag_with_an_empty_schema("action_msgs/msg/GoalStatusArray")
        self.assertEqual(bridge.inspect(self.bag)["topics"]["/status"]["definition"], "missing from the bag")
        m = json.loads(json.dumps(SIM_MAPPING))
        m["mappings"] = m["mappings"][:1]
        records, meta = self.convert(m)
        self.assertEqual(self.statuses(records), [(G1_TEXT, "SUCCEEDED")])
        self.assertEqual(meta["definitions_supplied_by_bridge"], ["action_msgs/msg/GoalStatusArray"])

    def test_an_empty_schema_the_bridge_has_no_definition_for_is_refused(self):
        self._bag_with_an_empty_schema("some_pkg/msg/Unknown")
        m = json.loads(json.dumps(SIM_MAPPING))
        m["mappings"] = m["mappings"][:1]
        with self.assertRaisesRegex(SystemExit, "no message definition"):
            self.convert(m)

    def test_clock_placement_needs_a_simulation_bag_and_a_stated_allowance(self):
        m = json.loads(json.dumps(SIM_MAPPING))
        del m["clock"]["receipt_allowance_ms"]
        with self.assertRaisesRegex(SystemExit, "receipt_allowance_ms"):
            self.convert(m)
        m = json.loads(json.dumps(SIM_MAPPING))
        m["clock"]["domain"] = "host"
        with self.assertRaises(SystemExit):
            self.convert(m)


if __name__ == "__main__":
    unittest.main()
