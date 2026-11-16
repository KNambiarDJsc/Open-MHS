"""The CDR decoder, against vectors produced by the reference ROS 2 MCAP encoder (mcap-ros2-support)."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from openmhs.importers import cdr  # noqa: E402

STRING_EMPTY = "000100000100000000"
STRING_TEXT = "000100001a00000064726f6e65302064726f6e65537461747573206661696c656400"
GSA_EMPTY = "0001000000000000"
GSA_THREE = (
    "0001000003000000000102030405060708090a0b0c0d0e0f6236c66a0500000002101112131415161718191a1b1c1d1e1f"
    "0000006636c66a0500000004202122232425262728292a2b2c2d2e2f000000070000000500000005"
)
POSE = (
    "000100007036c66a80b2e60e060000006561727468000000000000000000000000000840000000000000f4bf000000000000f83f"
    "00000000000000000000000000000000cd3b7f669ea0e63fcd3b7f669ea0e63f"
)


class CdrTests(unittest.TestCase):
    def test_strings(self):
        self.assertEqual(cdr.decode_string(bytes.fromhex(STRING_EMPTY)), "")
        self.assertEqual(cdr.decode_string(bytes.fromhex(STRING_TEXT)), "drone0 droneStatus failed")

    def test_goal_status_arrays_including_the_padding_after_each_status_byte(self):
        self.assertEqual(cdr.decode_goal_status_array(bytes.fromhex(GSA_EMPTY)), [])
        goals = cdr.decode_goal_status_array(bytes.fromhex(GSA_THREE))
        self.assertEqual([g.status for g in goals], ["EXECUTING", "SUCCEEDED", "CANCELED"])
        self.assertEqual(goals[0].goal_id, "00010203-0405-0607-0809-0a0b0c0d0e0f")
        self.assertEqual(goals[1].goal_id, "10111213-1415-1617-1819-1a1b1c1d1e1f")
        self.assertEqual(goals[2].goal_id, "20212223-2425-2627-2829-2a2b2c2d2e2f")
        self.assertEqual((goals[0].accepted_sec, goals[0].accepted_nanosec), (1791374946, 5))
        self.assertEqual(goals[2].accepted_sec, 7)

    def test_pose_stamped(self):
        p = cdr.decode_pose_stamped(bytes.fromhex(POSE))
        self.assertEqual((p.sec, p.nanosec, p.frame_id), (1791374960, 250000000, "earth"))
        self.assertEqual(p.position, (3.0, -1.25, 1.5))
        self.assertAlmostEqual(p.orientation[2], 0.7071067811865476)

    def test_refusals_say_what_is_wrong(self):
        cases = [
            (cdr.decode_string, b"\x00", "shorter"),
            (cdr.decode_string, bytes.fromhex("00000000") + bytes(8), "little-endian"),
            (cdr.decode_string, bytes.fromhex(STRING_TEXT)[:-3], "ends early"),
            (cdr.decode_string, bytes.fromhex("000100000000000000"), "terminating NUL"),
            (cdr.decode_string, bytes.fromhex("0001000002000000" + "4142"), "NUL terminated"),
            (cdr.decode_goal_status_array, bytes.fromhex(GSA_THREE)[:20], "ends early"),
            (cdr.decode_pose_stamped, bytes.fromhex(POSE)[:30], "ends early"),
        ]
        for decode, data, why in cases:
            with self.subTest(why=why), self.assertRaisesRegex(cdr.CdrError, why):
                decode(data)


if __name__ == "__main__":
    unittest.main()
