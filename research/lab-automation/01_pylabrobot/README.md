# 01 · PyLabRobot: governed, simulated liquid transfers

Quickstart. The findings are in [EXPERIMENT_REPORT.md](EXPERIMENT_REPORT.md); the raw results
of each run are in [results/](results/).

Needs: Python 3.11+ (run with 3.13.4), and an `openmhs` binary built from this repository.
No hardware, no network beyond installing the package.

```sh
# 1. PyLabRobot, pinned
python -m venv .venv
.venv/bin/pip install -r ../../../adapters/python/pylabrobot/requirements.txt   # pylabrobot==0.2.2

# 2. The native baseline: PyLabRobot alone, one transfer, prints a JSON record
.venv/bin/python native_baseline/baseline.py

# 3. The gateway
cargo build --release -p openmhs-cli
export OPENMHS_BIN=$PWD/../../../target/release/openmhs      # Windows: $env:OPENMHS_BIN = "...\openmhs.exe"

# 4. Every scenario, each on a fresh gateway, with the adapter as its child process
.venv/bin/python experiments/run_scenarios.py --out results/my-run.json
.venv/bin/python experiments/run_scenarios.py --only PLR-04 PLR-07   # some of them
```

Each scenario prints `PASS` or `FAIL` with the checks that failed, and the JSON keeps every
check, the receipts it judged and the simulator state it compared them with. A run takes
about 15 s. `--work DIR` keeps each scenario's gateway home, journal, log and simulator state
for inspection.

| Id | Scenario |
|---|---|
| PLR-01 | Transfer 50 uL from source A1 to destination B1 |
| PLR-02 | Out-of-contract volumes and wells: denied before dispatch |
| PLR-03 | No tip left: which layer refuses |
| PLR-04 | The transfer happens but its answer is lost |
| PLR-05 | The same request twice, with and without an idempotency key |
| PLR-06 | Two agents want more liquid from one well than it holds |
| PLR-07 | The gateway is killed mid-transfer, after and before the effect, and restarted |
| PERF | 30 transfers through OpenMHS against the same transfers with PyLabRobot alone |
