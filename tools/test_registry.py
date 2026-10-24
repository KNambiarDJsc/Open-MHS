"""Tests for tools/check_registry.py and tools/gen_matrix.py.

Each test copies the registry and reports into a temporary tree, breaks something, and
runs the real scripts against it with OPENMHS_DATA_ROOT. Nothing is executed from the
entries: they are data.

Usage: python -m unittest discover -s tools -p "test_*.py" -v   (requires: pip install jsonschema)
"""
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

TOOLS = pathlib.Path(__file__).resolve().parent
REPO = TOOLS.parent


def run(script, root, *args):
    env = dict(os.environ, OPENMHS_DATA_ROOT=str(root))
    p = subprocess.run([sys.executable, str(TOOLS / script), *args], env=env, capture_output=True, text=True)
    return p.returncode, p.stdout + p.stderr


class Tree(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = pathlib.Path(self._tmp.name)
        for d in ("registry", "conformance"):
            shutil.copytree(REPO / d, self.root / d)
        (self.root / "docs").mkdir()
        shutil.copy(REPO / "README.md", self.root / "README.md")
        shutil.copy(REPO / "docs" / "compatibility.md", self.root / "docs" / "compatibility.md")
        # Entries point at files in the repository; the checker only needs them to exist.
        for f in (self.root / "registry" / "adapters").glob("*.json"):
            e = json.loads(f.read_text(encoding="utf-8"))
            for p in [e["source"], *e["profiles"]]:
                if not p.startswith("https://"):
                    (self.root / p).mkdir(parents=True, exist_ok=True)

    def entry_path(self, id_):
        return self.root / "registry" / "adapters" / f"{id_}.json"

    def edit_entry(self, id_, **changes):
        p = self.entry_path(id_)
        e = json.loads(p.read_text(encoding="utf-8"))
        e.update(changes)
        p.write_text(json.dumps(e, indent=2) + "\n", encoding="utf-8")
        return e

    def edit_report(self, id_, mutate, rehash=True):
        e = json.loads(self.entry_path(id_).read_text(encoding="utf-8"))
        rp = self.root / e["report"]["path"]
        r = json.loads(rp.read_text(encoding="utf-8"))
        mutate(r)
        rp.write_text(json.dumps(r, indent=2) + "\n", encoding="utf-8")
        if rehash:
            e["report"]["sha256"] = hashlib.sha256(rp.read_bytes()).hexdigest()
            self.entry_path(id_).write_text(json.dumps(e, indent=2) + "\n", encoding="utf-8")

    def assert_rejected(self, text):
        rc, out = run("check_registry.py", self.root)
        self.assertEqual(rc, 1, out)
        self.assertIn(text, out)


class Registry(Tree):
    def test_the_committed_registry_is_valid(self):
        rc, out = run("check_registry.py", self.root)
        self.assertEqual(rc, 0, out)

    def test_an_unknown_field_is_rejected(self):
        self.edit_entry("openmhs.http", endorsed=True)
        self.assert_rejected("endorsed")

    def test_a_malformed_id_is_rejected(self):
        self.edit_entry("openmhs.http", id="Not A Valid Id")
        self.assert_rejected("Not A Valid Id")

    def test_the_file_name_must_match_the_id(self):
        self.edit_entry("openmhs.http", id="openmhs.other")
        self.assert_rejected("file name must be openmhs.other.json")

    def test_a_missing_report_file_is_rejected(self):
        e = json.loads(self.entry_path("openmhs.http").read_text(encoding="utf-8"))
        (self.root / e["report"]["path"]).unlink()
        self.assert_rejected("does not exist")

    def test_a_report_outside_the_reports_directory_is_rejected(self):
        e = json.loads(self.entry_path("openmhs.http").read_text(encoding="utf-8"))
        e["report"]["path"] = "conformance/reports/../../README.md"
        self.entry_path("openmhs.http").write_text(json.dumps(e), encoding="utf-8")
        self.assert_rejected("openmhs.http.json")

    def test_a_changed_report_no_longer_matches_its_hash(self):
        self.edit_report("openmhs.http", lambda r: r.update(duration_ms=1), rehash=False)
        self.assert_rejected("sha256")

    def test_an_official_entry_without_a_report_is_rejected(self):
        self.edit_entry("openmhs.http", report=None)
        self.assert_rejected("needs a conformance report")

    def test_a_report_for_another_adapter_is_rejected(self):
        self.edit_report("openmhs.http", lambda r: r["subject"].update(adapter="openmhs.modbus"))
        self.assert_rejected("is for adapter 'openmhs.modbus'")

    def test_skipped_mandatory_checks_cannot_back_an_official_entry(self):
        def skip(r):
            c = next(c for c in r["checks"] if c["status"] == "pass")
            c["status"] = "untested"
            r["summary"]["pass"] -= 1
            r["summary"]["untested"] += 1

        self.edit_report("openmhs.http", skip)
        self.assert_rejected("nothing failed or untested")

    def test_a_report_whose_summary_does_not_match_its_checks_is_rejected(self):
        self.edit_report("openmhs.http", lambda r: r["summary"].update({"pass": 1}))
        self.assert_rejected("does not match its checks")

    def test_physical_validation_cannot_be_claimed_for_a_simulated_report(self):
        self.edit_entry("openmhs.http", peer="device")
        self.assert_rejected("peer is 'device'")

    def test_an_entry_with_a_report_must_name_its_peer(self):
        p = self.entry_path("openmhs.http")
        e = json.loads(p.read_text(encoding="utf-8"))
        del e["peer"]
        p.write_text(json.dumps(e), encoding="utf-8")
        self.assert_rejected("must say what it talked to")

    def test_a_missing_profile_path_is_rejected(self):
        self.edit_entry("openmhs.http", profiles=["adapters/nowhere/profile.json"])
        self.assert_rejected("does not exist")

    def test_a_malformed_report_file_is_rejected(self):
        e = json.loads(self.entry_path("openmhs.http").read_text(encoding="utf-8"))
        extra = self.root / "conformance" / "reports" / "http.led-01" / "broken.json"
        extra.write_text("{not json", encoding="utf-8")
        rc, out = run("check_registry.py", self.root)
        self.assertEqual(rc, 1, out)
        self.assertIn("broken.json", out)
        self.assertTrue(e)


class Matrix(Tree):
    def test_the_committed_matrix_is_current(self):
        rc, out = run("gen_matrix.py", self.root, "--check")
        self.assertEqual(rc, 0, out)

    def test_a_changed_entry_makes_the_matrix_stale_until_regenerated(self):
        self.edit_entry("openmhs.http", name="Renamed HTTP adapter", conformance_profile="L2")
        run("check_registry.py", self.root)
        # The name is not in the matrix; a change to the report is.
        self.edit_report("openmhs.http", lambda r: r.update(achieved="L2"))
        rc, out = run("gen_matrix.py", self.root, "--check")
        self.assertEqual(rc, 1, out)
        self.assertIn("stale", out)
        rc, out = run("gen_matrix.py", self.root)
        self.assertEqual(rc, 0, out)
        rc, out = run("gen_matrix.py", self.root, "--check")
        self.assertEqual(rc, 0, out)

    def test_an_edited_matrix_file_is_stale(self):
        doc = self.root / "docs" / "compatibility.md"
        doc.write_text(doc.read_text(encoding="utf-8") + "\nmanual edit\n", encoding="utf-8")
        rc, _ = run("gen_matrix.py", self.root, "--check")
        self.assertEqual(rc, 1)

    def test_no_row_claims_physical_validation_for_simulator_reports(self):
        text = (self.root / "docs" / "compatibility.md").read_text(encoding="utf-8")
        table = text.split("## Adapters", 1)[1].split("## Reports on file", 1)[0]
        self.assertNotIn("| Physically validated |", table)

    def test_an_entry_without_a_report_is_shown_as_not_verified(self):
        text = (self.root / "docs" / "compatibility.md").read_text(encoding="utf-8")
        row = next(line for line in text.splitlines() if line.startswith("| [openmhs.py.opcua]"))
        self.assertIn("Not verified", row)
        self.assertNotIn("| Pass |", row)

    def test_skipped_checks_show_as_incomplete_not_pass(self):
        # An experimental entry may carry an incomplete report; the matrix must not call it a pass.
        self.edit_entry("openmhs.http", category="experimental")

        def skip(r):
            c = next(c for c in r["checks"] if c["status"] == "pass")
            c["status"] = "untested"
            r["summary"]["pass"] -= 1
            r["summary"]["untested"] += 1

        self.edit_report("openmhs.http", skip)
        rc, out = run("gen_matrix.py", self.root)
        self.assertEqual(rc, 0, out)
        text = (self.root / "docs" / "compatibility.md").read_text(encoding="utf-8")
        row = next(line for line in text.splitlines() if line.startswith("| [openmhs.http]"))
        self.assertIn("| Incomplete |", row)
        self.assertNotIn("| Pass |", row)

    def test_old_reports_are_listed_even_when_no_entry_points_at_them(self):
        d = self.root / "conformance" / "reports" / "http.led-01"
        old = next(d.glob("*.json"))
        data = json.loads(old.read_text(encoding="utf-8"))
        data["started_at"] = "2026-01-02T03:04:05.000Z"
        (d / "2026-01-02-v0.0.9-linux-x86_64.json").write_text(json.dumps(data, indent=2), encoding="utf-8")
        rc, out = run("gen_matrix.py", self.root)
        self.assertEqual(rc, 0, out)
        text = (self.root / "docs" / "compatibility.md").read_text(encoding="utf-8")
        self.assertIn("2026-01-02-v0.0.9-linux-x86_64.json", text)
        self.assertIn(old.name, text)

    def test_it_refuses_to_generate_from_an_invalid_registry(self):
        self.edit_entry("openmhs.http", endorsed=True)
        rc, out = run("gen_matrix.py", self.root)
        self.assertEqual(rc, 1, out)
        self.assertIn("not generating", out)


if __name__ == "__main__":
    unittest.main()
