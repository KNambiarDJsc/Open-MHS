"""The rules evidence depends on: nothing overwritten, nothing absent counted as a pass.

    python -m unittest discover -s research/lab-automation/shared -p "test_*.py"
"""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parent)]

import summarize  # noqa: E402
from run_record import finish_report, next_path, summarize as count, write_results  # noqa: E402
from to_bench import convert  # noqa: E402


def scenario(outcome: str) -> dict:
    return {"title": "t", "outcome": outcome, "checks": [], "detail": {}}


class Evidence(unittest.TestCase):
    def setUp(self) -> None:
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        self.dir = Path(d.name)

    def test_a_second_run_gets_a_new_file_and_the_first_is_untouched(self) -> None:
        first = write_results(self.dir, "gaps-x", {"ecosystem": "x"}, {"A": scenario("PASS")})
        before = first.read_bytes()
        second = write_results(self.dir, "gaps-x", {"ecosystem": "x"}, {"A": scenario("FAIL")})
        self.assertNotEqual(first, second)
        self.assertEqual(first.read_bytes(), before)

    def test_finish_report_will_not_overwrite_an_existing_result(self) -> None:
        out = self.dir / "run-1.json"
        out.write_text("earlier evidence")
        written = finish_report({"scenarios": {"A": {"passed": True, "run_dir": "C:/somewhere"}}}, out)
        self.assertEqual(out.read_text(), "earlier evidence")
        self.assertNotEqual(written, out)
        doc = json.loads(written.read_text())
        self.assertEqual(doc["scenarios"]["A"]["outcome"], "PASS")
        self.assertNotIn("run_dir", doc["scenarios"]["A"], "machine-specific paths are not evidence")

    def test_every_result_says_it_is_simulated(self) -> None:
        doc = json.loads(write_results(self.dir, "g", {}, {"A": scenario("PASS")}).read_text())
        self.assertIs(doc["simulated"], True)
        self.assertIs(doc["physical_effects"], False)
        self.assertIn("packages", doc["environment"])

    def test_a_result_has_the_same_bytes_on_every_platform(self) -> None:
        # Git stores LF. A file written with CRLF would not match a digest taken of it.
        path = write_results(self.dir, "g", {}, {"A": scenario("PASS")})
        self.assertNotIn(b"\r\n", path.read_bytes())

    def test_an_unknown_outcome_word_is_refused(self) -> None:
        with self.assertRaises(ValueError):
            write_results(self.dir, "g", {}, {"A": scenario("MOSTLY")})

    def test_counts_keep_the_three_outcomes_apart(self) -> None:
        self.assertEqual(count({"a": scenario("PASS"), "b": scenario("NOT_TESTED"), "c": scenario("FAIL")}),
                         {"PASS": 1, "FAIL": 1, "NOT_TESTED": 1})

    def test_next_path_counts_from_what_exists(self) -> None:
        (self.dir / "x-007.json").write_text("{}")
        self.assertEqual(next_path(self.dir, "x").name, "x-008.json")


class Summary(unittest.TestCase):
    def setUp(self) -> None:
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        self.root = Path(d.name)

    def gap_run(self, ecosystem: str, **outcomes: str) -> None:
        write_results(self.root / "gaps", f"gaps-{ecosystem}", {"ecosystem": ecosystem},
                      {k.replace("_", "-"): scenario(v) for k, v in outcomes.items()})

    def test_one_failed_run_makes_the_cell_fail(self) -> None:
        self.gap_run("a", GAP_01="PASS")
        self.gap_run("a", GAP_01="FAIL")
        data = summarize.scan(self.root)
        self.assertEqual(summarize.verdict(data["gaps"][("GAP-01", "a")]), "FAIL")

    def test_not_tested_is_not_a_pass(self) -> None:
        self.gap_run("a", GAP_01="NOT_TESTED")
        data = summarize.scan(self.root)
        self.assertEqual(summarize.verdict(data["gaps"][("GAP-01", "a")]), "NOT_TESTED")

    def test_a_combination_that_never_ran_is_a_dash(self) -> None:
        self.gap_run("a", GAP_01="PASS")
        self.gap_run("b", GAP_02="PASS")
        text = summarize.to_markdown(summarize.scan(self.root))
        self.assertIn("| GAP-01 | PASS 1/1 | — |", text)
        self.assertIn("| GAP-02 | — | PASS 1/1 |", text)

    def test_the_same_evidence_gives_the_same_bytes(self) -> None:
        self.gap_run("a", GAP_01="PASS")
        data = summarize.scan(self.root)
        self.assertEqual(summarize.to_markdown(data), summarize.to_markdown(summarize.scan(self.root)))


class BenchFormat(unittest.TestCase):
    def test_a_converted_gap_result_is_valid_and_has_no_score(self) -> None:
        try:
            import jsonschema
        except ImportError:
            self.skipTest("jsonschema is not installed")
        repo = HERE.parents[2]
        schema = json.loads((repo / "docs" / "bench" / "bench-result.schema.json").read_text(encoding="utf-8"))
        run = {"ecosystem": "pylabrobot", "source": {"commit": "abc"}, "environment": {"os": "x", "packages": {}},
               "scenarios": {"GAP-05": {"title": "t", "outcome": "PASS", "detail": {
                   "operation": {"target": {}}, "dispatch_count": 1,
                   "lost": {"transitions": ["CREATED", "UNKNOWN_OUTCOME"]},
                   "reconciled": {"state": "RECONCILED_APPLIED"}}}}}
        result = convert(run, "GAP-05", "9.9.9", [("x.json", "0" * 64)])
        jsonschema.validate(result, schema)
        self.assertEqual(result["target"]["kind"], "simulator")
        with self.assertRaises(jsonschema.ValidationError):
            jsonschema.validate({**result, "score": 97}, schema)
        no_reason = {**result, "outcome": "NOT_TESTED"}
        with self.assertRaises(jsonschema.ValidationError):
            jsonschema.validate(no_reason, schema)


if __name__ == "__main__":
    unittest.main()
