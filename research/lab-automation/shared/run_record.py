"""What every experiment records about a run, and how it keeps earlier runs.

A result file is evidence. It says what ran, on what, and what happened, and a later run never
replaces it: each run gets a new file, and the file refuses to be overwritten.

Simulated is stated in every result, not implied. ``physical_effects`` is always ``false`` here,
because nothing in these experiments can produce one.

Outcomes are exactly ``PASS``, ``FAIL`` or ``NOT_TESTED``. A summary counts them and never turns
one into another.
"""

from __future__ import annotations

import datetime as dt
import importlib.metadata as metadata
import json
import os
import platform
import subprocess
import sys
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[3]
RESULT_FORMAT = "openmhs-lab-result/1"
OUTCOMES = ("PASS", "FAIL", "NOT_TESTED")


def _run(*cmd: str) -> str | None:
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=30, check=True).stdout.strip() or None
    except (OSError, subprocess.SubprocessError):
        return None


def installed_packages() -> dict[str, str]:
    """Every package in this environment at its exact version: the run's real pin set."""
    found = {}
    for dist in metadata.distributions():
        name = dist.metadata["Name"]
        if name:
            found[name.lower()] = dist.version
    return dict(sorted(found.items()))


def environment() -> dict[str, Any]:
    """Machine facts, with no user or path information beyond what identifies the build."""
    bin_path = os.environ.get("OPENMHS_BIN")
    return {
        "os": platform.system(),
        "os_release": platform.release(),
        "machine": platform.machine(),
        "python": sys.version.split()[0],
        "python_implementation": platform.python_implementation(),
        "rustc": _run("rustc", "--version"),
        "openmhs_version": _run(bin_path, "--version") if bin_path else None,
        "packages": installed_packages(),
    }


def source() -> dict[str, Any]:
    git = ("git", "-C", str(REPO))
    # Whether the code that ran is exactly that commit. Reports and results do not count.
    status = _run(*git, "status", "--porcelain", "--", "crates", "adapters", "python", "research/lab-automation",
                  ":!research/lab-automation/results", ":!*.md", ":!*/results/*")
    return {"commit": _run(*git, "rev-parse", "HEAD"), "dirty": bool(status)}


def summarize(results: dict[str, dict]) -> dict[str, int]:
    counts = dict.fromkeys(OUTCOMES, 0)
    for r in results.values():
        counts[r["outcome"]] += 1
    return counts


def next_path(directory: Path, stem: str) -> Path:
    """The next free ``<stem>-NNN.json``. Earlier runs are never touched."""
    directory.mkdir(parents=True, exist_ok=True)
    taken = [int(p.stem.rsplit("-", 1)[1]) for p in directory.glob(f"{stem}-[0-9][0-9][0-9].json")]
    return directory / f"{stem}-{max(taken, default=0) + 1:03d}.json"


def finish_report(report: dict[str, Any], out: str | Path) -> Path:
    """Complete an experiment's report and write it to ``out``, which must not exist yet.

    Each scenario gains an ``outcome``; machine-specific paths are removed, since they say
    nothing about the result and do not reproduce."""
    path = Path(out)
    for scenario in report["scenarios"].values():
        scenario["outcome"] = "PASS" if scenario["passed"] else "FAIL"
        scenario.pop("run_dir", None)
    report.pop("openmhs_bin", None)
    doc = {"format": RESULT_FORMAT, "simulated": True, "physical_effects": False, "source": source(),
           "environment": environment(), "summary": summarize(report["scenarios"]), **report}
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        with open(path, "x", encoding="utf-8", newline="
") as f:
            json.dump(doc, f, indent=1, default=str)
            f.write("\n")
    except FileExistsError:
        fallback = next_path(path.parent, path.stem)
        with open(fallback, "x", encoding="utf-8", newline="
") as f:
            json.dump(doc, f, indent=1, default=str)
            f.write("\n")
        print(f"{path} already exists and is evidence: this run was written to {fallback} instead", flush=True)
        return fallback
    return path


def write_results(directory: Path, stem: str, meta: dict[str, Any], results: dict[str, dict]) -> Path:
    """Write one run. ``x`` mode: if the name was taken in the meantime, this fails loudly."""
    for sid, r in results.items():
        if r["outcome"] not in OUTCOMES:
            raise ValueError(f"{sid}: outcome {r['outcome']!r} is not one of {OUTCOMES}")
    doc = {
        "format": RESULT_FORMAT,
        "finished_at": dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds"),
        "simulated": True,
        "physical_effects": False,
        "source": source(),
        "environment": environment(),
        **meta,
        "summary": summarize(results),
        "scenarios": results,
    }
    path = next_path(directory, stem)
    with open(path, "x", encoding="utf-8", newline="
") as f:
        json.dump(doc, f, indent=1, default=str)
        f.write("\n")
    return path
