# 05 · MADSci: OpenMHS as the governed boundary of one device in an orchestrated closed loop

Findings: [EXPERIMENT_REPORT.md](EXPERIMENT_REPORT.md). Raw results: [results/](results/).

MADSci stays the orchestrator (workcell, workflows, nodes, data). OpenMHS governs one node's
physical action: the PyLabRobot liquid handler. The synthetic experiment finds the dose with
half the maximal response (a known EC50 of 42 uL) by bisection.

No Docker, Redis or MongoDB: MADSci's own in-memory backends run its workcell, location and
data managers (`stack/workcell.py`, `stack/location.py`, `stack/data.py`).

```sh
uv venv --python 3.12 .venv && uv pip install --python .venv -r requirements.txt
export OPENMHS_BIN=$PWD/../../../target/release/openmhs
export PLR_PYTHON=$PWD/.venv/bin/python        # a Python with pylabrobot
.venv/bin/python experiments/run_scenarios.py --out results/my-run.json
```

| Path | What it is |
|---|---|
| [stack/nodes.py](stack/nodes.py) | MADSci nodes: `liquid_handler` (governed through OpenMHS, or native) and `plate_reader` (measures the dose actually in the well, from the simulator's state) |
| [stack/experiment.py](stack/experiment.py) | The bisection loop and its two-step workflow |
| [experiments/run_scenarios.py](experiments/run_scenarios.py) | MAD-01..MAD-07, plus native baselines of MAD-01 and MAD-04 |

| Id | Scenario |
|---|---|
| MAD-01 | A six-iteration closed loop, governed, against the same loop without OpenMHS |
| MAD-02 | A device unavailable: the gateway, then the plate reader |
| MAD-03 | Two experiments at once on one liquid handler |
| MAD-04 | A transfer happens but its answer is lost; the workflow is retried in MADSci |
| MAD-05 | A transfer that needs an operator's approval |
| MAD-06 | The gateway is killed mid-step, restarted, reconciled, and the workflow retried |
| MAD-07 | The same experiment twice with a fixed seed |
