# Reproducing the lab-automation research

Everything here is simulated. You need no instrument and no account, and nothing connects to
hardware. A `VERIFIED` means a simulator's software state agreed, and every result file says
`"simulated": true, "physical_effects": false`.

## Setup

Python 3.12 for every environment, and a gateway built from this repository:

```sh
cargo build --release -p openmhs-cli
export OPENMHS_BIN=$PWD/target/release/openmhs        # Windows: $env:OPENMHS_BIN = "...\openmhs.exe"

cd research/lab-automation
for e in 01_pylabrobot 02_opentrons 03_micromanager 04_sila2 05_madsci; do
  python3.12 -m venv .venv-$e
  .venv-$e/bin/pip install -r $e/requirements.lock.txt     # Windows: .venv-$e\Scripts\pip
done

# Micro-Manager's demo adapters: no hardware, but a download
.venv-03_micromanager/bin/mmcore install --test-adapters --dest ./mm
export MM_DIR=$(ls -d "$PWD"/mm/*/)

export LAB_PY_PYLABROBOT=$PWD/.venv-01_pylabrobot/bin/python
export LAB_PY_OPENTRONS=$PWD/.venv-02_opentrons/bin/python
export LAB_PY_MICROMANAGER=$PWD/.venv-03_micromanager/bin/python
export LAB_PY_SILA2=$PWD/.venv-04_sila2/bin/python
export LAB_PY_MADSCI=$PWD/.venv-05_madsci/bin/python
```

The locks were resolved for Python 3.12 across platforms, constrained to the exact versions the
experiments ran with. Each adapter's `requirements.txt` holds the direct pins, and each result file
lists every package actually installed.

## Run

```sh
python reproduce.py --label my-run                                    # everything, once
python reproduce.py --label my-run --runs 3 --ecosystems sila2,madsci  # some of it, three times
python reproduce.py --label my-run --stages conformance               # unit,conformance,gaps,scenarios
```

An ecosystem whose variable is not set is recorded as `NOT_TESTED`, with the reason. It does not
disappear from the summary, and it does not count as a pass. A run with any failure exits non-zero
and keeps its logs.

## What you get

```
results/<label>/
  SUMMARY.md, SUMMARY.json     PASS / FAIL / NOT_TESTED per property and ecosystem, from the files below
  gaps/                        06_contract_gaps, one file per run
  scenarios/                   each experiment's scenarios, one file per run
  conformance/<ecosystem>-NNN/ the conformance suite's own report
  steps/                       what ran, and its exit status
```

- **Nothing is overwritten.** A run takes the next free number in its label, and files are opened
  so that an existing one cannot be replaced.
- **The summary is derived, and deterministic.** `python summarize.py results/<label>` gives the same
  bytes for the same files. A scenario that passed twice and failed once is `FAIL`, shown as 2/3.
- **Each result names what ran:** the commit, whether the code differed from it, the operating system,
  Python, Rust, the gateway's version and every installed package. Paths on your machine are left out.

## Where it has run

| Platform | Status | Evidence |
|---|---|---|
| Windows 11 desktop (one machine) | All stages, 3 runs | [results/2026-11-02/SUMMARY.md](results/2026-11-02/SUMMARY.md) |
| Linux (GitHub `ubuntu-latest`) | See the CI results recorded in [results/README.md](results/README.md) | CI artifacts on PR #10 |
| macOS | **Not run.** CI covers the Rust workspace and Python SDK there, not these experiments | |
| Raspberry Pi, ARM | **Not run** | |
| Anyone else's machine | **Not run.** Nobody else has reproduced this | |

## Known differences between runs

- Latency and timing numbers depend on the machine; nothing here makes a claim from a debug build.
- Several scenarios use real time (timeouts of a few seconds). A heavily loaded machine can turn a
  check into a `FAIL`. Read the check before concluding anything from one.
- A `FAIL` on your machine is worth reporting: it may be the first evidence from anywhere but here.
