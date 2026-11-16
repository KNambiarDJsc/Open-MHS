"""The Aerostack2 rosbag2 (SQLite) importer: on small synthetic bags, and on the real recordings when they are present."""

from __future__ import annotations

import json
import os
import shutil
import sqlite3
import struct
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "python"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from openmhs.importers import coresense_aerostack2 as imp  # noqa: E402
from tests_cdr_vectors import GSA_THREE  # noqa: E402

GSA_TYPE = "action_msgs/msg/GoalStatusArray"
STRING_TYPE = "std_msgs/msg/String"
S = 1_000_000_000
T0 = 1_791_374_000 * S


def enc_goal_status_array(goals: list[tuple[bytes, int, int]]) -> bytes:
    """Little-endian CDR, aligned from the first byte after the 4-byte header. Checked against the reference encoder."""
    out = bytearray(b"\x00\x01\x00\x00") + struct.pack("<I", len(goals))
    for uuid, status, sec in goals:
        out += uuid
        out += b"\x00" * ((-(len(out) - 4)) % 4)
        out += struct.pack("<iI", sec, 5) + struct.pack("<b", status)
    return bytes(out)


def enc_string(text: str) -> bytes:
    body = text.encode() + b"\x00"
    return b"\x00\x01\x00\x00" + struct.pack("<I", len(body)) + body


def enc_pose(x: float, y: float, z: float) -> bytes:
    """geometry_msgs/PoseStamped, little-endian CDR: stamp, empty frame id, position, identity orientation."""
    out = bytearray(b"\x00\x01\x00\x00") + struct.pack("<iI", 1, 0) + struct.pack("<I", 1) + b"\x00"
    out += b"\x00" * ((-(len(out) - 4)) % 8)
    out += struct.pack("<7d", x, y, z, 0.0, 0.0, 0.0, 1.0)
    return bytes(out)


def uuid(n: int) -> bytes:
    return bytes([n]) * 16


EXECUTING, SUCCEEDED, CANCELED, ABORTED = 2, 4, 5, 6


def make_bag(root: Path, name: str, drones: dict[str, dict]) -> Path:
    """drones: {'drone0': {'goals': {'Takeoff': [(uuid_n, [(t_s, status), ...]), ...]}, 'facts': [(t_s, text)]}}"""
    d = root / name
    d.mkdir(parents=True)
    con = sqlite3.connect(d / "x_0.db3")
    con.execute("create table topics (id integer primary key, name text, type text, serialization_format text, offered_qos_profiles text)")
    con.execute("create table messages (id integer primary key, topic_id integer, timestamp integer, data blob)")
    tid = 0
    rows: list[tuple[int, int, bytes]] = []

    def topic(topic_name: str, typ: str) -> int:
        nonlocal tid
        tid += 1
        con.execute("insert into topics values (?, ?, ?, 'cdr', '')", (tid, topic_name, typ))
        return tid

    for drone, spec in drones.items():
        for behavior in ("Takeoff", "GoTo", "Land", "Auction", "CollisionAvoidance", "FollowPath"):
            t = topic(f"/{drone}/{behavior}Behavior/_action/status", GSA_TYPE)
            for goal_n, steps in spec.get("goals", {}).get(behavior, []):
                for at, status in steps:
                    rows.append((t, T0 + int(at * S), enc_goal_status_array([(uuid(goal_n), status, 1)])))
        t = topic(f"/{drone}/kb/add_fact", STRING_TYPE)
        for at, text in spec.get("facts", []):
            rows.append((t, T0 + int(at * S), enc_string(text)))
        if "poses" in spec:
            t = topic(f"/{drone}/self_localization/pose", "geometry_msgs/msg/PoseStamped")
            for at, x, y, z in spec["poses"]:
                rows.append((t, T0 + int(at * S), enc_pose(x, y, z)))
        if "mission_status" in spec:
            t = topic(f"/{drone}/mission_status", STRING_TYPE)
            for at, doc in spec["mission_status"]:
                rows.append((t, T0 + int(at * S), enc_string(json.dumps(doc))))
    for n, (t, ts, data) in enumerate(sorted(rows, key=lambda r: r[1]), 1):
        con.execute("insert into messages values (?, ?, ?, ?)", (n, t, ts, data))
    con.commit()
    con.close()
    (d / "metadata.yaml").write_text("rosbag2_bagfile_information:\n  version: 5\n", encoding="utf-8")
    return d


class Synthetic(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        spec = {
            "drone0": {  # a clean flight with one goal canceled by a re-plan
                "goals": {
                    "Takeoff": [(1, [(10, EXECUTING), (14, SUCCEEDED)])],
                    "GoTo": [(2, [(15, EXECUTING), (17, SUCCEEDED)]), (3, [(18, EXECUTING), (19, CANCELED)])],
                    "Land": [(4, [(30, EXECUTING), (36, SUCCEEDED)])],
                    "CollisionAvoidance": [(9, [(15, EXECUTING), (17, SUCCEEDED)])],
                },
                "facts": [(18.5, "drone0 auctionStatus started"), (18.7, "drone0 auctionStatus completed")],
            },
            "drone1": {  # the swarm reports the drone failed; its landing is canceled
                "goals": {
                    "Takeoff": [(5, [(10, EXECUTING), (14, SUCCEEDED)])],
                    "GoTo": [(6, [(15, EXECUTING), (25, CANCELED)])],
                    "Land": [(7, [(26, EXECUTING), (27, CANCELED)])],
                },
                "facts": [(20, "drone1 droneStatus failed"), (21, "drone0 auctionStatus started")],
            },
            "drone2": {  # never lands: the recording ends mid-flight
                "goals": {"Takeoff": [(8, [(10, EXECUTING), (14, SUCCEEDED)])], "GoTo": [(10, [(15, EXECUTING)])]},
            },
        }
        self.bag = make_bag(Path(self.tmp.name), "rosbag2_real_paths_2_OK_panel_failure", spec)
        self.records = imp.import_bag(self.bag)
        self.events = [r for r in self.records if r.get("schema", "").startswith("openmhs.event/")]

    def ended(self, drone: str):
        return next(e for e in self.events if e["type"] == "task.ended" and e["scope"]["robot"] == f"crazyflie/{drone}")

    def test_the_test_encoder_matches_the_reference_encoder(self):
        goals = [(bytes(range(i, i + 16)), st, sec) for i, st, sec in ((0, 2, 1791374946), (16, 4, 1791374950), (32, 5, 7))]
        self.assertEqual(enc_goal_status_array(goals), bytes.fromhex(GSA_THREE))

    def test_each_drone_gets_one_mission_with_the_outcome_the_swarm_reported(self):
        self.assertEqual(self.ended("drone0")["outcome"]["reported"]["value"], "succeeded")
        failed = self.ended("drone1")["outcome"]["reported"]
        self.assertEqual((failed["value"], failed["source"]), ("failed", "aerostack2.kb"))
        unknown = self.ended("drone2")["outcome"]["reported"]
        self.assertEqual((unknown["value"], unknown["detail_code"]), ("unknown", "no_terminal_land"))

    def test_goals_are_actions_keyed_by_their_uuid_and_terminal_status_is_the_reported_outcome(self):
        ended = [e for e in self.events if e["type"] == "action.ended" and e["scope"]["robot"] == "crazyflie/drone0"]
        by_status = sorted(e["outcome"]["reported"]["value"] for e in ended)
        self.assertEqual(by_status, ["canceled", "succeeded", "succeeded", "succeeded"])
        self.assertTrue(all(e["subject"]["goal_id"].startswith("0") for e in ended))
        self.assertTrue(all("collision" not in e["attributes"]["component"] for e in self.events if "attributes" in e))

    def test_a_goal_that_never_finished_is_started_but_never_ended(self):
        d2 = [e for e in self.events if e["scope"]["robot"] == "crazyflie/drone2" and e["type"].startswith("action.")]
        goto = [e for e in d2 if e["attributes"]["component"] == "goto"]
        self.assertEqual([e["type"] for e in goto], ["action.started"])

    def test_a_drones_facts_become_events_only_from_its_own_stream(self):
        faults = [e for e in self.events if e["type"] == "fault.raised"]
        self.assertEqual(len(faults), 1)
        self.assertEqual(faults[0]["scope"]["robot"], "crazyflie/drone1")
        markers = [e for e in self.events if e["type"] == "marker"]
        self.assertEqual(sorted(m["scope"]["robot"] for m in markers), ["crazyflie/drone0", "crazyflie/drone0"],
                         "drone1's stream mentions drone0's auction; that fact belongs to drone0's own stream")

    def test_failures_are_labelled_induced_and_the_software_revision_unknown(self):
        self.assertTrue(all(e["ext"]["coresense"]["failures_induced_by_experimenters"] for e in self.events))
        manifest = next(r for r in self.records if r.get("schema", "").startswith("openmhs.deployment/"))
        software = next(c for c in manifest["components"] if c["name"] == "aerostack2")
        self.assertEqual(software["revision"], "not published by the dataset")

    def test_the_bag_is_referenced_by_digest_and_uri_never_copied(self):
        art = next(r for r in self.records if r.get("schema", "").startswith("openmhs.artifact/"))
        self.assertTrue(art["artifact_id"].startswith("sha256:"))
        self.assertTrue(art["uri"].startswith("file:///"))
        self.assertNotIn("data", art)

    def test_same_bag_same_bytes_and_unique_event_ids(self):
        again = [imp.canonical(r) for r in imp.import_bag(self.bag)]
        self.assertEqual(again, [imp.canonical(r) for r in self.records])
        ids = [e["event_id"] for e in self.events]
        self.assertEqual(len(ids), len(set(ids)))

    def test_the_bag_is_opened_read_only(self):
        db = next(self.bag.glob("*.db3"))
        before = db.read_bytes()
        imp.import_bag(self.bag)
        self.assertEqual(db.read_bytes(), before)


REAL = Path(os.environ.get("CORESENSE_BAGS", r"D:\openmhs-research\datasets\coresense\bags"))


@unittest.skipUnless((REAL / "rosbag2_real_paths_2_OK_double_failure").exists(), "the CoreSense bags are not on this machine")
class RealFlights(unittest.TestCase):
    """Pinned values from the real recordings (Zenodo 10.5281/zenodo.23243295, CC BY 4.0)."""

    def events(self, condition: str):
        recs = imp.import_bag(REAL / f"rosbag2_real_paths_2_OK_{condition}")
        return [r for r in recs if r.get("schema", "").startswith("openmhs.event/")]

    def outcomes(self, condition: str):
        out: dict[str, int] = {}
        for e in self.events(condition):
            if e["type"] == "action.ended":
                v = e["outcome"]["reported"]["value"]
                out[v] = out.get(v, 0) + 1
        return out

    def test_goal_outcomes_in_the_three_flights(self):
        self.assertEqual(self.outcomes("no_failure"), {"succeeded": 41})
        self.assertEqual(self.outcomes("panel_failure"), {"succeeded": 45, "canceled": 3})
        self.assertEqual(self.outcomes("double_failure"), {"succeeded": 47, "canceled": 6})

    def test_only_one_drone_reported_failed_and_only_in_the_double_failure_flight(self):
        for condition, failed in (("no_failure", 0), ("panel_failure", 0), ("double_failure", 1)):
            ended = [e for e in self.events(condition) if e["type"] == "task.ended"]
            self.assertEqual(len(ended), 3)
            self.assertEqual(sum(e["outcome"]["reported"]["value"] == "failed" for e in ended), failed, condition)


def flight_pose(t: float) -> tuple[float, float, float]:
    """Where the drone is at t seconds: hovering at 1.5 m, at waypoint wp_0 (3, 1) from 16 s, 0.5 m past waypoint wp_1
    (5, 1) from 19 s, on the ground from 31 s."""
    if t < 16:
        return 0.0, 0.0, 1.5
    if t < 19:
        return 3.0, 1.0, 1.5
    if t < 31:
        return 5.5, 1.0, 1.5
    return 5.5, 1.0, 0.05


def verified_flight(**override) -> dict:
    spec = {
        "goals": {
            "Takeoff": [(1, [(10, EXECUTING), (14, SUCCEEDED)])],
            "GoTo": [(2, [(15, EXECUTING), (17, SUCCEEDED)]), (3, [(18, EXECUTING), (20, SUCCEEDED)])],
            "Land": [(4, [(30, EXECUTING), (36, SUCCEEDED)])],
        },
        "facts": [(1, 'wp_0 xCoord "3.0"'), (1, 'wp_0 yCoord "1.0"'), (1, 'wp_1 xCoord "5.0"'), (1, 'wp_1 yCoord "1.0"'),
                  (15.05, "goto_1 to wp_0"), (18.05, "goto_2 to wp_1")],
        "poses": [(k / 100, *flight_pose(k / 100)) for k in range(800, 4200)],
    }
    spec.update(override)
    return {"drone0": spec}


HAVE_BINARY = bool(os.environ.get("OPENMHS_BIN") or shutil.which("openmhs"))


@unittest.skipUnless(HAVE_BINARY, "needs the openmhs binary (OPENMHS_BIN)")
class Verified(unittest.TestCase):
    """``--verify``: each succeeded Takeoff, GoTo and Land is checked against the motion-capture pose."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def import_(self, spec: dict, verify: bool = True, name: str = "rosbag2_real_paths_2_OK_no_failure"):
        bag = make_bag(self.root, name, spec)
        records = imp.import_bag(bag, verify=verify)
        return records, [r for r in records if r.get("schema", "").startswith("openmhs.event/")]

    def test_each_succeeded_goal_gets_a_verdict_joined_to_it_by_goal_id(self):
        _, events = self.import_(verified_flight())
        verdicts = [e for e in events if e["type"] == "outcome.verified"]
        by_goal = {e["subject"]["goal_id"]: e["outcome"]["verified"]["value"] for e in verdicts}
        self.assertEqual(len(verdicts), 4)
        self.assertEqual(len(by_goal), 4, "one verdict per goal, each under its own goal id")
        order = [e["outcome"]["verified"]["value"] for e in sorted(verdicts, key=lambda e: e["time"]["observed"])]
        self.assertEqual(order, ["satisfied", "satisfied", "violated", "satisfied"],
                         "takeoff at 1.5 m, arrived at wp_0, 0.5 m past wp_1, on the ground")
        self.assertTrue(all("task_id" not in e["subject"] for e in verdicts), "joined by goal id, not by owning the task")
        self.assertEqual({e["outcome"]["verified"]["contract"] for e in verdicts},
                         {"takeoff-at-height", "goto-arrived-at-waypoint", "land-on-ground"})

    def test_the_thresholds_are_in_the_manifest_and_make_a_different_release(self):
        plain, _ = self.import_(verified_flight(), verify=False, name="rosbag2_real_paths_2_OK_a")
        verified, events = self.import_(verified_flight(), verify=True, name="rosbag2_real_paths_2_OK_b")
        self.assertFalse([e for e in plain if e.get("type") == "outcome.verified"], "off unless asked for")
        m_plain = next(r for r in plain if r.get("schema", "").startswith("openmhs.deployment/"))
        m_ver = next(r for r in verified if r.get("schema", "").startswith("openmhs.deployment/"))
        th = next(c for c in m_ver["components"] if c["name"] == "behavior_thresholds")
        self.assertEqual((th["go_to_xy_m"], th["takeoff_height_m"], th["land_z_max_m"]), (0.2, 1.5, 0.15))
        self.assertIn("assumption", th["basis"], "the source of the GoTo number says it is assumed")
        self.assertNotEqual(imp.digest(m_plain), imp.digest(m_ver))
        self.assertEqual(m_ver["labels"]["release"], "aerostack2-unversioned+thresholds")
        started = next(e for e in events if e["type"] == "task.started")
        self.assertEqual(started["deployment"]["manifest"], imp.digest(m_ver))

    def test_a_wider_declared_threshold_changes_the_verdict_not_the_code(self):
        from openmhs.importers import coresense_verify as cv

        claim = {"behavior": "GoTo", "goal": "g", "drone": "drone0", "target": {"x": 5.0, "y": 1.0}}
        tight = cv.contract(claim, {**cv.DEFAULT_THRESHOLDS})
        loose = cv.contract(claim, {**cv.DEFAULT_THRESHOLDS, "go_to_xy_m": 0.6})
        self.assertAlmostEqual(tight["conditions"][0]["max"]["value"], 5.2)
        self.assertAlmostEqual(loose["conditions"][0]["max"]["value"], 5.6, msg="the same 0.5 m miss now fits")

    def test_a_manifest_without_the_thresholds_cannot_be_verified_against(self):
        from openmhs.importers import coresense_verify as cv

        with self.assertRaisesRegex(ValueError, "no behavior_thresholds"):
            cv.thresholds_from({"components": []})
        partial = {"components": [{"kind": "config", "name": "behavior_thresholds", "go_to_xy_m": 0.2}]}
        with self.assertRaisesRegex(ValueError, "does not declare"):
            cv.thresholds_from(partial)

    def test_a_flight_that_asked_for_another_takeoff_height_is_refused(self):
        spec = verified_flight(mission_status=[(9, {"current_item": {"behavior": "takeoff", "args": {"height": 2.0}}})])
        with self.assertRaisesRegex(ValueError, "takeoff heights"):
            self.import_(spec)

    def test_a_goto_without_a_waypoint_is_left_out_not_guessed(self):
        spec = verified_flight(facts=[(1, 'wp_0 xCoord "3.0"')])  # no yCoord, no goto link
        _, events = self.import_(spec)
        goals = {e["subject"]["goal_id"] for e in events if e["type"] == "outcome.verified"}
        self.assertEqual(len(goals), 2, "only the takeoff and the landing")


if __name__ == "__main__":
    unittest.main()
