"""The examples run, end to end, against the real binary."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import unittest

from support import REPO, binary


class AgentLoop(unittest.TestCase):
    def test_mock_agent_replans_from_structured_denials(self) -> None:
        p = subprocess.run(
            [sys.executable, str(REPO / "examples" / "agent-loop" / "mock_agent.py"), "--json"],
            env={**os.environ, "OPENMHS_BIN": str(binary())},
            capture_output=True,
            timeout=180,
        )
        self.assertEqual(p.returncode, 0, p.stderr.decode("utf-8", "replace"))
        result = json.loads(p.stdout.decode("utf-8").strip().splitlines()[-1])
        self.assertTrue(result["ok"])
        self.assertEqual(result["outcomes"], {
            "psu_5v": "done", "psu_15v": "refused_out_of_range", "home_stage": "escalated"})
        self.assertTrue(result["audit_valid"])


class LearningAgent(unittest.TestCase):
    def test_a_second_session_learns_the_limit_from_the_journal(self) -> None:
        p = subprocess.run(
            [sys.executable, str(REPO / "examples" / "agent-loop" / "learning_agent.py"), "--json"],
            env={**os.environ, "OPENMHS_BIN": str(binary())},
            capture_output=True,
            timeout=180,
        )
        self.assertEqual(p.returncode, 0, p.stderr.decode("utf-8", "replace"))
        result = json.loads(p.stdout.decode("utf-8").strip().splitlines()[-1])
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["first"]["refused"], 1)
        self.assertEqual(result["second"]["refused"], 0)
        self.assertTrue(result["audit_valid"])


class LlmAgentWithoutKey(unittest.TestCase):
    """The optional LLM variant never needs a key to be tested, and never calls the API."""

    def run_example(self, *args: str) -> subprocess.CompletedProcess[bytes]:
        env = {k: v for k, v in os.environ.items()
               if k not in ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN")}
        env["OPENMHS_BIN"] = str(binary())
        return subprocess.run(
            [sys.executable, str(REPO / "examples" / "agent-loop" / "llm_agent.py"), *args],
            env=env, capture_output=True, timeout=120)

    def test_without_a_key_it_explains_and_runs_nothing(self) -> None:
        p = self.run_example()
        self.assertEqual(p.returncode, 0)
        self.assertIn(b"ANTHROPIC_API_KEY", p.stdout)
        self.assertIn(b"Nothing was run", p.stdout)

    def test_tools_convert_to_claude_definitions(self) -> None:
        p = self.run_example("--print-tools")
        self.assertEqual(p.returncode, 0, p.stderr.decode("utf-8", "replace"))
        tools = {t["name"]: t for t in json.loads(p.stdout)}
        self.assertEqual(len(tools), 18)
        for t in tools.values():
            self.assertEqual(set(t), {"name", "description", "input_schema"})
            self.assertEqual(t["input_schema"]["type"], "object")
            self.assertNotIn("$schema", t["input_schema"])
        self.assertTrue(tools["submit_command"]["description"].startswith("[CHANGES THE PHYSICAL WORLD]"))
        self.assertTrue(tools["submit_plan"]["description"].startswith("[CHANGES THE PHYSICAL WORLD]"))
        self.assertTrue(tools["read_state"]["description"].startswith("[read-only]"))
        self.assertTrue(tools["check_plan"]["description"].startswith("[read-only]"))


if __name__ == "__main__":
    unittest.main()
