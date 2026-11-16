"""The real-flight check's own logic, on synthetic poses (no bag needed)."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import coresense_outcomes as co  # noqa: E402

S = 1_000_000_000
T = 1_791_374_000 * S


def goto_claim(x: float = 3.0, y: float = 1.0) -> dict:
    return {"id": "f/drone0/GoTo/abc", "bag": "f", "drone": "drone0", "behavior": "GoTo", "goal": "g-1", "t": T,
            "target": {"wp": "wp_0", "x": x, "y": y}}


def poses_around(x: float, y: float, z: float = 1.5) -> list[tuple[int, float, float, float]]:
    return [(T + k * 10_000_000, x, y, z) for k in range(-300, 301)]


class CheckLogic(unittest.TestCase):
    def test_the_goto_contract_is_the_planned_box_in_the_named_frame(self):
        c = co.contract(goto_claim())
        self.assertEqual(c["binding"], {"attempt": "g-1"})
        self.assertEqual(c["timing"]["default_window_ms"], [-50, 50])
        self.assertEqual({c["evidence"]["x"]["frame"], c["evidence"]["y"]["frame"]}, {"earth"})
        self.assertFalse(c["evidence"]["x"]["bound"])
        x = c["conditions"][0]
        self.assertAlmostEqual(x["max"]["value"] - x["min"]["value"], 0.40)
        self.assertAlmostEqual((x["max"]["value"] + x["min"]["value"]) / 2, 3.0)

    def test_takeoff_and_landing_contracts(self):
        take = co.contract({"behavior": "Takeoff", "goal": "g", "drone": "drone0"})
        self.assertEqual(take["conditions"][0]["min"]["value"], 1.25)
        land = co.contract({"behavior": "Land", "goal": "g", "drone": "drone0"})
        self.assertEqual((land["conditions"][0]["op"], land["conditions"][0]["value"]["value"]), ("le", 0.15))

    def test_labels_come_from_the_nearest_sample_and_say_when_there_is_none(self):
        self.assertEqual(co.label(goto_claim(), poses_around(3.1, 1.1))[0], "AT_TARGET")
        self.assertEqual(co.label(goto_claim(), poses_around(3.3, 1.0))[0], "NOT_AT_TARGET")
        far = [(T + 5 * S, 3.0, 1.0, 1.5)]
        self.assertEqual(co.label(goto_claim(), far), ("NO_REFERENCE", None))

    def test_tolerance_is_a_parameter_of_the_label_never_of_the_contract(self):
        c, p = goto_claim(), poses_around(3.25, 1.0)
        self.assertEqual(co.label(c, p)[0], "NOT_AT_TARGET")
        self.assertEqual(co.label(c, p, 0.30)[0], "AT_TARGET")
        self.assertEqual(co.contract(c)["conditions"][0]["max"]["value"], 3.2, "the contract stays at the planned 0.20 m")

    def test_the_script_reads_numbers_and_ignores_the_frame(self):
        c = goto_claim()
        records = co.evidence_records(c, poses_around(3.0, 1.0))
        self.assertEqual(co.script(c, records), "SATISFIED")
        for r in records:
            if r["signal"].startswith("pose."):
                r["frame"] = "odom"
        self.assertEqual(co.script(c, records), "SATISFIED", "a script that ignores frames cannot notice")

    def test_injected_faults_change_exactly_what_they_say(self):
        c = goto_claim()
        records = co.evidence_records(c, poses_around(3.0, 1.0))
        frame = co.inject("wrong_frame", c, records)
        self.assertEqual({r["frame"] for r in frame if r["signal"].startswith("pose.")}, {"odom"})
        self.assertNotEqual(co.script(c, frame), "SATISFIED", "the numbers were scrambled")
        clock = co.inject("other_clock", c, records)
        self.assertEqual({r["time"]["clock_domain"] for r in clock if r["signal"].startswith("pose.")}, {"other-logger"})
        gap = co.inject("reference_gap", c, records)
        self.assertEqual(co.script(c, gap), "NO_DATA")
        other = co.inject("other_attempt", c, records)
        self.assertEqual(other[0]["refs"]["attempt"], "goal-somebody-else")
        self.assertEqual(records[0]["refs"]["attempt"], "g-1", "faults work on copies")

    def test_a_frozen_pose_gives_the_same_answer_as_a_live_one_at_that_height(self):
        c = {"id": "f/drone2/Land/x", "bag": "f", "drone": "drone2", "behavior": "Land", "goal": "g", "t": T, "target": None}
        frozen = poses_around(1.0, 0.0, z=0.408)
        self.assertEqual(co.label(c, frozen)[0], "NOT_AT_TARGET")
        self.assertEqual(co.script(c, co.evidence_records(c, frozen)), "VIOLATED")


if __name__ == "__main__":
    unittest.main()
