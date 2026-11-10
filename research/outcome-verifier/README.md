# Outcome verifier v0

Did the physical world end up in the state that was intended, and can the evidence show it? A command that ends
`VERIFIED` says the device read back what was asked. It does not say the outcome the request was for was achieved.

This is a first, offline answer to that question ([RFC 0004](../../docs/rfcs/0004-outcome-contracts-and-evidence.md)).
An **outcome contract** names the evidence a declared outcome needs and the conditions it must meet. Evidence records,
from OpenMHS or any other system, are checked against it, and the answer is `SATISFIED`, `VIOLATED` or `UNKNOWN`, with
the reasons. Deterministic, no model, never in the control path.

**Simulated and hand-written evidence only so far. Nothing has been checked against physical hardware.**

## Try it

```sh
cargo build -p openmhs-cli

# One corpus case: a DMM whose calibration had expired
openmhs outcome verify \
  --contract research/outcome-verifier/corpus/cases/psu/dmm_calibration_expired/contract.json \
  --evidence research/outcome-verifier/corpus/cases/psu/dmm_calibration_expired/evidence.jsonl

# OpenMHS's own evidence: an audit export, read directly
openmhs audit export --out export.json
openmhs outcome verify --contract my-contract.json --audit export.json --evidence my-instrument.jsonl

# The virtual-lab cases, end to end (about a minute)
python research/outcome-verifier/virtual_lab_run.py --out /tmp/outcome-vlab
```

`outcome verify` exits 0 for `SATISFIED`, 3 for `VIOLATED`, 4 for `UNKNOWN`, and 1 if the inputs cannot be checked.
Add `--json` for the full report, which is the same bytes for the same inputs.

## What is here

| | |
|---|---|
| [EVALUATION.md](EVALUATION.md) | The plan, committed before any code |
| [RESULTS.md](RESULTS.md) | What happened, including what it does not show |
| `corpus/make_corpus.py` | Writes the 27 fault cases; `--check` fails if they drift |
| `corpus/cases/` | Each case: `contract.json`, `evidence.jsonl`, `expected.json` |
| `virtual_lab_run.py` | The virtual-lab cases through the real gateway |
| `naive_baseline.py` | The simple script the strategy compares against |
| `results/` | Raw output |

The verifier is `crates/openmhs-outcome`; the command is `crates/openmhs-cli/src/outcome.rs`.

## What it is not

Not a workflow language, a planner, or anything that acts. Not a judge of whether a sensor is really independent:
`source.kind` and `provenance` are the producer's word. Not a safety system. And not yet evidence that it helps: that
needs a real device and a real second instrument.
