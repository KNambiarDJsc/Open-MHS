"""Run the conformance suite against the OctoPrint adapter and a throwaway OctoPrint.

    python run_conformance.py --octoprint <octoprint> --bin <openmhs> --report-dir <dir>

The suite starts the adapter in fixture mode; the adapter reads from the OctoPrint this script
starts (Virtual Printer, random port and key). Its only fault, link_down, is injected inside
the adapter, so a real printer disconnect is covered by test_octoprint.py instead.
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import octoprint_harness as h  # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--octoprint", default=os.environ.get("OPENMHS_OCTOPRINT"))
    ap.add_argument("--bin", default=os.environ.get("OPENMHS_BIN"))
    ap.add_argument("--report-dir", required=True)
    a = ap.parse_args()
    if not a.octoprint or not a.bin:
        print("need --octoprint and --bin (or OPENMHS_OCTOPRINT and OPENMHS_BIN)", file=sys.stderr)
        return 2
    with h.OctoPrintServer(a.octoprint) as octo, tempfile.TemporaryDirectory() as tmp:
        m = json.loads((HERE / "manifest.json").read_text(encoding="utf-8"))
        m["runtime"]["command"] = [sys.executable, str(HERE / "octoprint_adapter.py")]
        m["runtime"]["env"] = {"PYTHONPATH": str(HERE.parents[2] / "python")}
        m["endpoints"]["network"] = [octo.url]
        m["config"]["url"] = octo.url
        manifest = Path(tmp) / "manifest.json"
        manifest.write_text(json.dumps(m), encoding="utf-8")
        env = dict(os.environ, OCTOPRINT_API_KEY=octo.key, OPENMHS_HOME=str(Path(tmp) / "home"))
        env.pop("OPENMHS_TOKEN", None)
        return subprocess.run(
            [a.bin, "conformance", "--adapter-manifest", str(manifest), "--report-dir", a.report_dir], env=env
        ).returncode


if __name__ == "__main__":
    sys.exit(main())
