# 03 · Micro-Manager: a governed demo microscope

Quickstart. Findings: [EXPERIMENT_REPORT.md](EXPERIMENT_REPORT.md). Raw results: [results/](results/).

Needs Python 3.12 (3.10+ should work; only 3.12 was run) and an `openmhs` binary built from
this repository. No microscope.

```sh
uv venv --python 3.12 .venv
uv pip install --python .venv -r ../../../adapters/python/micromanager/requirements.txt
# Micro-Manager's test device adapters only (DemoCamera, Utilities): about 1 MB
.venv/bin/mmcore install --test-adapters --dest ./mm
MM=$(ls -d ./mm/Micro-Manager-*)

.venv/bin/python native_baseline/probe.py "$MM"         # Micro-Manager alone
export OPENMHS_BIN=$PWD/../../../target/release/openmhs
.venv/bin/python experiments/run_scenarios.py --mm "$MM" --out results/my-run.json
```

| Id | Scenario |
|---|---|
| MM-01 | A 2×2 grid in two channels, with provenance for every image |
| MM-02 | Out-of-envelope positions, exposure, channel and units |
| MM-03 | Two agents: without and with a lease |
| MM-04 | The camera goes away and comes back |
| MM-05 | A move happens but its answer is lost; and a newer move supersedes it |
| MM-06 | Image-to-command provenance, offline evidence, an edited image |
| MM-07 | The gateway is killed after a move and restarted |
| PERF | 20 positions (move and snap) through OpenMHS against Micro-Manager alone |
