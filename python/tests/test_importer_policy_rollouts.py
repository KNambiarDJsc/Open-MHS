"""The policy-rollouts importer, on small synthetic result files (no model, no simulator)."""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from openmhs.importers import policy_rollouts as imp  # noqa: E402

REVISION = "84a7c23178445c6bbf7e1a884ff497017910f653"


def header(arm: str, **over) -> dict:
    h = {"arm": arm, "model": "m", "model_sha256": "ab" * 32, "n_action_steps": 8, "horizon": 16, "n_obs_steps": 2,
         "num_train_timesteps": 100, "noise_scheduler": "DDPM", "num_inference_steps": None, "max_steps": 300,
         "torch": "2.15.0+cpu", "device": "cpu", "started": "2026-11-16T00:00:00Z"}
    h.update(over)
    return {"header": h}


def episode(arm: str, seed: int, success: bool, steps: int = 120) -> dict:
    return {"arm": arm, "seed": seed, "success": success, "steps": steps, "max_reward": 1.0 if success else 0.9,
            "final_reward": 1.0 if success else 0.8, "rewards": [0.1], "seconds": 12.5}


def write(path: Path, rows: list[dict]) -> None:
    path.write_text("\n".join(json.dumps(r) for r in rows) + "\n", encoding="utf-8")


class PolicyRolloutTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)

    def records(self, arm: str = "ddpm-100", **kw):
        files = sorted(self.dir.glob(f"{arm}.*.jsonl"))
        return imp.import_arm(files, REVISION, "0.7.0", **kw)

    def test_each_episode_is_a_task_attempt_whose_scenario_is_its_seed(self):
        write(self.dir / "ddpm-100.1000-1003.jsonl",
              [header("ddpm-100"), episode("ddpm-100", 1000, True), episode("ddpm-100", 1001, False), episode("ddpm-100", 1002, True)])
        ev = [r for r in self.records() if r.get("schema", "").startswith("openmhs.event/")]
        ended = [e for e in ev if e["type"] == "task.ended"]
        self.assertEqual([e["subject"]["scenario_id"] for e in ended], ["pusht/seed-1000", "pusht/seed-1001", "pusht/seed-1002"])
        self.assertEqual([e["outcome"]["reported"]["value"] for e in ended], ["succeeded", "failed", "succeeded"])
        self.assertEqual(ended[1]["outcome"]["reported"]["source"], "gym_pusht.is_success")
        self.assertEqual([e["cost"]["wall_s"] for e in ended], [12.5, 12.5, 12.5], "the compute time is recorded apart from the simulated clock")
        self.assertEqual({e["subject"]["task_type"] for e in ev if "subject" in e}, {"pusht"})

    def test_the_outcome_carries_the_value_the_harness_judged_on_and_its_own_flag(self):
        failed = episode("ddpm-100", 1001, False)
        failed["max_reward"] = 0.0963
        write(self.dir / "ddpm-100.1000-1002.jsonl", [header("ddpm-100"), episode("ddpm-100", 1000, True), failed])
        ended = [e for e in self.records() if e.get("type") == "task.ended"]
        conds = [e["outcome"]["reported"]["conditions"] for e in ended]
        self.assertEqual([c[0]["result"] for c in conds], ["true", "false"], "the harness's flag, copied")
        self.assertEqual([c[0]["observed"] for c in conds], [[1.0], [0.0963]])
        self.assertEqual({(c[0]["evidence"], c[0]["op"], c[0]["value"]) for c in conds}, {("reward", "ge", 1.0)})
        self.assertNotIn("verified", ended[1]["outcome"], "a self-judgement is never a verdict")

    def test_the_same_seed_has_the_same_scenario_in_every_arm_and_a_different_task(self):
        write(self.dir / "ddpm-100.1000-1001.jsonl", [header("ddpm-100"), episode("ddpm-100", 1000, True)])
        write(self.dir / "ddim-10.1000-1001.jsonl", [header("ddim-10", noise_scheduler="DDIM", num_inference_steps=10), episode("ddim-10", 1000, False)])
        a = [r for r in self.records("ddpm-100") if r.get("type") == "task.ended"][0]
        b = [r for r in self.records("ddim-10") if r.get("type") == "task.ended"][0]
        self.assertEqual(a["subject"]["scenario_id"], b["subject"]["scenario_id"])
        self.assertNotEqual(a["subject"]["task_id"], b["subject"]["task_id"])

    def test_the_manifest_names_the_model_exactly_and_a_sampler_change_is_a_different_manifest(self):
        write(self.dir / "ddpm-100.1000-1001.jsonl", [header("ddpm-100"), episode("ddpm-100", 1000, True)])
        write(self.dir / "ddim-10.1000-1001.jsonl", [header("ddim-10", noise_scheduler="DDIM", num_inference_steps=10), episode("ddim-10", 1000, True)])
        ma = next(r for r in self.records("ddpm-100") if r.get("schema", "").startswith("openmhs.deployment/"))
        mb = next(r for r in self.records("ddim-10") if r.get("schema", "").startswith("openmhs.deployment/"))
        model = next(c for c in ma["components"] if c["kind"] == "model")
        self.assertEqual((model["revision"], model["checkpoint_digest"]), (REVISION, "sha256:" + "ab" * 32))
        self.assertNotEqual(imp.digest(ma), imp.digest(mb))
        names = lambda m, kind: {c["name"]: c for c in m["components"] if c["kind"] == kind}  # noqa: E731
        self.assertEqual(names(ma, "model"), names(mb, "model"), "the weights are the same")
        self.assertNotEqual(names(ma, "config")["sampler"]["config_digest"], names(mb, "config")["sampler"]["config_digest"])
        self.assertEqual(mb["labels"]["release"], "ddim-10")

    def test_shards_are_merged_and_a_repeated_seed_counts_once(self):
        write(self.dir / "ddpm-100.1000-1002.jsonl", [header("ddpm-100"), episode("ddpm-100", 1000, True), episode("ddpm-100", 1001, False)])
        write(self.dir / "ddpm-100.1002-1004.jsonl", [header("ddpm-100"), episode("ddpm-100", 1002, True), episode("ddpm-100", 1001, False)])
        seeds = [e["subject"]["scenario_id"] for e in self.records() if e.get("type") == "task.ended"]
        self.assertEqual(seeds, ["pusht/seed-1000", "pusht/seed-1001", "pusht/seed-1002"])

    def test_shards_with_different_configurations_are_refused(self):
        write(self.dir / "ddpm-100.1000-1001.jsonl", [header("ddpm-100"), episode("ddpm-100", 1000, True)])
        write(self.dir / "ddpm-100.1001-1002.jsonl", [header("ddpm-100", n_action_steps=4), episode("ddpm-100", 1001, True)])
        with self.assertRaisesRegex(ValueError, "differs"):
            self.records()

    def test_no_intervention_is_invented_and_time_is_simulated(self):
        write(self.dir / "ddpm-100.1000-1001.jsonl", [header("ddpm-100"), episode("ddpm-100", 1000, False)])
        ev = [r for r in self.records() if r.get("schema", "").startswith("openmhs.event/")]
        self.assertFalse([e for e in ev if e["type"].startswith("intervention")])
        self.assertTrue(all(e["time"]["clock_domain"] == "sim:pusht/ddpm-100" for e in ev))
        ended = next(e for e in ev if e["type"] == "task.ended")
        started = next(e for e in ev if e["type"] == "task.started")
        self.assertGreater(ended["time"]["observed"], started["time"]["observed"])

    def test_byte_identical_on_a_second_import(self):
        write(self.dir / "ddpm-100.1000-1002.jsonl", [header("ddpm-100"), episode("ddpm-100", 1000, True), episode("ddpm-100", 1001, False)])
        a = [imp.canonical(r) for r in self.records()]
        b = [imp.canonical(r) for r in self.records()]
        self.assertEqual(a, b)
        ids = [r["event_id"] for r in self.records() if "event_id" in r]
        self.assertEqual(len(ids), len(set(ids)))


if __name__ == "__main__":
    unittest.main()
