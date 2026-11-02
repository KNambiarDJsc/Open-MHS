"""Run `openmhs conformance` on a lab adapter, with its experiment's own simulated setup.

    OPENMHS_BIN=... <venv python> run_conformance.py --ecosystem sila2 --report-dir DIR

For SiLA 2 this starts the simulated SiLA server first: the generic adapter needs a real server to
talk to, and injects the suite's faults between itself and that server. The report is the suite's
own; a level is reached only with no failed and no untested checks (spec/conformance.md §2).
"""

from __future__ import annotations

import argparse
import contextlib
import os
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve()
sys.path[:0] = [str(HERE.parents[1] / "06_contract_gaps"), str(HERE.parent)]

from ecosystems import ALL, unwrap  # noqa: E402
from harness import binary  # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--ecosystem", choices=sorted(ALL), required=True)
    ap.add_argument("--report-dir", required=True)
    ap.add_argument("--work")
    a = ap.parse_args()
    root = Path(a.work or tempfile.mkdtemp(prefix=f"conformance-{a.ecosystem}-"))
    eco = ALL[a.ecosystem]()
    lab = eco.make_lab(root / "lab")
    unwrap(lab)
    start_server = getattr(lab, "start_server", None)
    try:
        if start_server:
            start_server()
        Path(a.report_dir).mkdir(parents=True, exist_ok=True)
        done = subprocess.run([str(binary()), "--home", str(root / "home"), "conformance", "--adapter-manifest",
                               str(lab.manifest), "--report-dir", a.report_dir],
                              env={**os.environ}, text=True, capture_output=True, encoding="utf-8", errors="replace")
        print(done.stdout)
        print(done.stderr, file=sys.stderr)
        return done.returncode
    finally:
        kill_all = getattr(lab, "kill_all", None)
        if kill_all:
            with contextlib.suppress(Exception):
                kill_all()


if __name__ == "__main__":
    raise SystemExit(main())
