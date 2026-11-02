"""Re-run the lab-automation research from one command, and summarize what happened.

    python research/lab-automation/reproduce.py --label my-run

Everything is simulated: no instrument is needed, and none can be reached.

An ecosystem runs in its own Python environment (requirements.lock.txt in each experiment), found
through an environment variable. One that is not configured, or that fails to start, is recorded
as NOT_TESTED with the reason. It is never left out, and never counted as a pass:

    OPENMHS_BIN            the gateway (cargo build --release -p openmhs-cli)
    LAB_PY_PYLABROBOT      python of the 01_pylabrobot environment
    LAB_PY_OPENTRONS       python of the 02_opentrons environment
    LAB_PY_MICROMANAGER    python of the 03_micromanager environment
    MM_DIR                 Micro-Manager's demo adapters (mmcore install --test-adapters)
    LAB_PY_SILA2           python of the 04_sila2 environment
    LAB_PY_MADSCI          python of the 05_madsci environment

Evidence goes to results/<label>/, and a label that already exists is added to, not replaced:
each run gets the next free number. The exit status is non-zero if anything FAILED.

    --stages    unit,conformance,gaps,scenarios (default: all)
    --ecosystems pylabrobot,opentrons,micromanager,sila2,madsci (default: all)
    --runs N    how many times to run each gap and scenario set (default 1)
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path[:0] = [str(HERE / "shared")]

from run_record import next_path  # noqa: E402

ALL = ("pylabrobot", "opentrons", "micromanager", "sila2", "madsci")
PY_VAR = {e: f"LAB_PY_{e.upper()}" for e in ALL}
GAPS = ("pylabrobot", "opentrons", "micromanager", "sila2")  # MADSci governs through PyLabRobot's adapter
EXPERIMENT = {"pylabrobot": "01_pylabrobot", "opentrons": "02_opentrons", "micromanager": "03_micromanager",
              "sila2": "04_sila2", "madsci": "05_madsci"}
#: The gateway stops tests that hang; this only stops a harness that hangs.
STEP_TIMEOUT_S = 1800


class Run:
    def __init__(self, label: str, work: Path) -> None:
        self.dir = HERE / "results" / label
        self.dir.mkdir(parents=True, exist_ok=True)
        self.work = work
        self.steps: list[dict] = []

    def python(self, eco: str) -> str | None:
        return os.environ.get(PY_VAR[eco])

    def env(self, eco: str) -> dict[str, str]:
        env = {**os.environ, "PYTHONPATH": str(REPO / "python"), "PYTHONIOENCODING": "utf-8"}
        if eco == "madsci":
            env["PLR_PYTHON"] = os.environ.get(PY_VAR["pylabrobot"]) or self.python("madsci") or ""
        return env

    def not_tested(self, stage: str, eco: str, why: str) -> None:
        print(f"NOT_TESTED {stage} {eco}: {why}", flush=True)
        self.steps.append({"stage": stage, "ecosystem": eco, "outcome": "NOT_TESTED", "why": why})

    def step(self, stage: str, eco: str, name: str, cmd: list[str], *, env: dict | None = None,
             cwd: Path | None = None) -> None:
        print(f"--- {stage} {eco} {name}", flush=True)
        log = self.work / f"{stage}-{eco}-{name}.log"
        try:
            with open(log, "wb") as f:
                p = subprocess.run(cmd, env=env, cwd=cwd or self.work, stdout=f, stderr=subprocess.STDOUT,
                                   timeout=STEP_TIMEOUT_S)
            code = p.returncode
        except subprocess.TimeoutExpired:
            code = "timed out"
        tail = log.read_text(encoding="utf-8", errors="replace").splitlines()[-40:]
        for line in tail:
            if line.startswith(("PASS", "FAIL", "NOT_TESTED", "     failed")) or "reached L" in line:
                print("   ", line[:200], flush=True)
        self.steps.append({"stage": stage, "ecosystem": eco, "name": name, "exit": code,
                           "outcome": "PASS" if code == 0 else "FAIL"})


def unit(run: Run) -> None:
    env = {**os.environ, "PYTHONPATH": str(REPO / "python")}
    run.step("unit", "madsci", "governed-step", [sys.executable, "-m", "unittest", "discover", "-s",
                                                 str(HERE / "05_madsci" / "tests")], env=env)


def conformance(run: Run, ecos: list[str]) -> None:
    for eco in ecos:
        if eco not in GAPS:
            run.not_tested("conformance", eco, "MADSci is an orchestrator: it has no adapter of its own to test")
            continue
        py = run.python(eco)
        if not py or (eco == "micromanager" and not os.environ.get("MM_DIR")):
            run.not_tested("conformance", eco, f"{PY_VAR[eco]} (and MM_DIR for Micro-Manager) is not set")
            continue
        out = next_path(run.dir / "conformance", eco).with_suffix("")
        run.step("conformance", eco, out.name, [py, str(HERE / "shared" / "run_conformance.py"), "--ecosystem", eco,
                                                "--report-dir", str(out), "--work", str(run.work / out.name)],
                 env=run.env(eco))


def gaps(run: Run, ecos: list[str], runs: int) -> None:
    for eco in ecos:
        if eco not in GAPS:
            run.not_tested("gaps", eco, "not run separately: MADSci governs through the PyLabRobot adapter "
                                        "(its approval and restart behaviour is in MAD-05, 06, 08, 09)")
            continue
        py = run.python(eco)
        if not py or (eco == "micromanager" and not os.environ.get("MM_DIR")):
            run.not_tested("gaps", eco, f"{PY_VAR[eco]} (and MM_DIR for Micro-Manager) is not set")
            continue
        for i in range(runs):
            run.step("gaps", eco, f"run-{i + 1}", [py, str(HERE / "06_contract_gaps" / "run_gaps.py"), "--ecosystem",
                                                   eco, "--out", str(run.dir / "gaps"),
                                                   "--work", str(run.work / f"gaps-{eco}-{i}")], env=run.env(eco))
        if eco == "sila2":  # the restart again, on the separately written server
            env = {**run.env(eco), "SILA_SERVER": "heater_b"}
            run.step("gaps", eco, "heater_b", [py, str(HERE / "06_contract_gaps" / "run_gaps.py"), "--ecosystem", eco,
                                               "--only", "GAP-05", "GAP-08", "--out", str(run.dir / "gaps-heater_b"),
                                               "--work", str(run.work / "gaps-sila-b")], env=env)


def scenarios(run: Run, ecos: list[str], runs: int) -> None:
    for eco in ecos:
        py = run.python(eco)
        if not py or (eco == "micromanager" and not os.environ.get("MM_DIR")):
            run.not_tested("scenarios", eco, f"{PY_VAR[eco]} (and MM_DIR for Micro-Manager) is not set")
            continue
        script = HERE / EXPERIMENT[eco] / "experiments" / "run_scenarios.py"
        variants = [("heater_a", ["--server", "heater_a"]), ("heater_b", ["--server", "heater_b"])] \
            if eco == "sila2" else [("", ["--mm", os.environ["MM_DIR"]] if eco == "micromanager" else [])]
        for tag, extra in variants:
            stem = f"{eco}-{tag}" if tag else eco
            for i in range(runs):
                out = next_path(run.dir / "scenarios", stem)
                run.step("scenarios", eco, out.stem, [py, str(script), *extra, "--out", str(out),
                                                      "--work", str(run.work / out.stem)], env=run.env(eco))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--label", required=True)
    ap.add_argument("--stages", default="unit,conformance,gaps,scenarios")
    ap.add_argument("--ecosystems", default=",".join(ALL))
    ap.add_argument("--runs", type=int, default=1)
    a = ap.parse_args()
    if not os.environ.get("OPENMHS_BIN") or not Path(os.environ["OPENMHS_BIN"]).exists():
        ap.error("set OPENMHS_BIN to an openmhs binary built from this repository")
    stages, ecos = a.stages.split(","), a.ecosystems.split(",")
    work = Path(tempfile.mkdtemp(prefix="lab-reproduce-"))
    run = Run(a.label, work)
    print(f"evidence: {run.dir}\nscratch:  {work}", flush=True)
    if "unit" in stages:
        unit(run)
    if "conformance" in stages:
        conformance(run, ecos)
    if "gaps" in stages:
        gaps(run, ecos, a.runs)
    if "scenarios" in stages:
        scenarios(run, ecos, a.runs)
    steps_file = next_path(run.dir / "steps", "steps")
    with open(steps_file, "x", encoding="utf-8") as f:
        json.dump(run.steps, f, indent=1)
    subprocess.run([sys.executable, str(HERE / "summarize.py"), str(run.dir)], check=False)
    failed = [s for s in run.steps if s["outcome"] == "FAIL"]
    if failed:
        print(f"{len(failed)} step(s) failed; their logs are in {work}", flush=True)
    else:
        shutil.rmtree(work, ignore_errors=True)
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
