"""The OpenAMRobot Gazebo run importer, on the committed results of the recorded runs (no ROS, no bag needed)."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RUNS = ROOT / "research" / "outcome-verifier" / "ros2-evaluation" / "results" / "gazebo_openamr"
sys.path.insert(0, str(ROOT / "python"))

from openmhs.importers import openamr_gazebo as imp  # noqa: E402


class ImporterTests(unittest.TestCase):
    def records(self, run: str) -> list[dict]:
        return imp.import_run(RUNS / run)

    def test_the_same_run_imports_to_the_same_bytes(self):
        a = [imp.canonical(r) for r in self.records("bullet1")]
        b = [imp.canonical(r) for r in self.records("bullet1")]
        self.assertEqual(a, b)
        ids = [r["event_id"] for r in self.records("bullet1") if "event_id" in r]
        self.assertEqual(len(ids), len(set(ids)), "event ids are unique")

    def test_tasks_actions_and_verdicts(self):
        events = [r for r in self.records("bullet1") if r.get("schema", "").startswith("openmhs.event/")]
        started = [e for e in events if e["type"] == "task.started"]
        self.assertEqual(sum(e["subject"]["task_type"] == "lift" for e in started), 10)
        self.assertEqual(sum(e["subject"]["task_type"] == "drive" for e in started), 4)
        lift_ended = [e for e in events if e["type"] == "action.ended"]
        self.assertTrue(all(e["outcome"]["reported"]["value"] == "succeeded" for e in lift_ended),
                        "every lift goal reported SUCCEEDED in bullet1")
        verdicts = [e for e in events if e["type"] == "outcome.verified" and "subject" in e]
        self.assertEqual(sum(e["outcome"]["verified"]["value"] == "violated" for e in verdicts), 5)
        # Lift verdicts name the goal; odometry verdicts name nothing and must be joined by time.
        self.assertTrue(all("goal_id" in e["subject"] for e in verdicts))
        odometry = [e for e in events if e["type"] == "outcome.verified" and "subject" not in e]
        self.assertGreater(len(odometry), 100)
        self.assertTrue(all(e["attributes"]["component"] == "base" for e in odometry))

    def test_one_manifest_per_run_referenced_by_digest(self):
        records = self.records("bullet1")
        manifests = [r for r in records if r.get("schema", "").startswith("openmhs.deployment/")]
        self.assertEqual(len(manifests), 1)
        digest = imp.digest(manifests[0])
        refs = {r["deployment"]["manifest"] for r in records if "deployment" in r}
        self.assertEqual(refs, {digest})
        names = {(c["kind"], c["name"]) for c in manifests[0]["components"]}
        self.assertIn(("config", "lift_controller"), names)
        self.assertIn(("config", "gazebo-physics"), names)
        self.assertEqual(manifests[0]["labels"]["release"], "engine-bullet")

    def test_runs_are_separate_robots_on_separate_clocks(self):
        a = {r["scope"]["robot"] for r in self.records("bullet1") if "scope" in r}
        b = {r["scope"]["robot"] for r in self.records("dart1") if "scope" in r}
        self.assertTrue(a.isdisjoint(b))
        domains = {r["time"]["clock_domain"].split("/")[1] for r in self.records("dart1") if "time" in r}
        self.assertEqual(domains, {"dart1"})

    def test_the_bag_is_referenced_never_copied(self):
        artifacts = [r for r in self.records("bullet1") if r.get("schema", "").startswith("openmhs.artifact/")]
        self.assertEqual(len(artifacts), 1)
        self.assertTrue(artifacts[0]["artifact_id"].startswith("sha256:"))
        self.assertTrue(artifacts[0]["uri"].endswith("bag_0.mcap"))

    def test_command_line_writes_json_lines(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "events.jsonl"
            subprocess.run([sys.executable, "-m", "openmhs.importers.openamr_gazebo", str(RUNS / "bullet1"),
                            "--out", str(out)], check=True, cwd=ROOT / "python", capture_output=True)
            lines = out.read_text(encoding="utf-8").splitlines()
            self.assertGreater(len(lines), 150)
            for line in lines:
                json.loads(line)


if __name__ == "__main__":
    unittest.main()
