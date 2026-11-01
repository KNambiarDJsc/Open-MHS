# 02 · Opentrons: governed protocol simulation

Quickstart. Findings: [EXPERIMENT_REPORT.md](EXPERIMENT_REPORT.md). Raw results: [results/](results/).

Needs **Python 3.12** (opentrons 10.1.0 does not run on 3.11 or install on 3.13 here) and an
`openmhs` binary built from this repository. No robot.

```sh
uv venv --python 3.12 .venv
uv pip install --python .venv -r ../../../adapters/python/opentrons/requirements.txt
export OT_API_CONFIG_DIR=$PWD/.ot            # otherwise opentrons writes ~/.opentrons

.venv/bin/python native_baseline/baseline.py # the simulator alone, valid and bad parameters
export OPENMHS_BIN=$PWD/../../../target/release/openmhs
.venv/bin/python experiments/run_scenarios.py --out results/my-run.json
```

| Id | Scenario |
|---|---|
| OT-01 | A valid serial dilution, compared command for command with the simulator alone |
| OT-02 | Bad parameters, an unreviewed protocol name, a protocol file edited after review |
| OT-03 | A run held for an operator's approval; one approved, one cancelled |
| OT-04 | The run completes but its answer is lost |
| OT-05 | The same submission twice |
| OT-06 | Evidence export, offline verification, and replay of the simulation |
| OT-07 | The gateway is killed after the run, restarted and reconciled |
| PERF | 20 runs through OpenMHS against the simulator alone |
