# ros2-action-faults: unknown outcomes of ROS 2 actions (Layer 0)

A pre-registered fault-injection study of what happens when the outcome of a ROS 2 action goal is
lost, and whether a client can tell which outcome occurred. This directory is **Layer 0**: a
pure-Python harness that runs a *model of ROS 2 action semantics* in virtual time.

**Limits first.**

- It is a model written from reading ROS 2 sources, not ROS 2. Nothing here has been run on a real
  `rclpy` server, a robot or a real sensor.
- Simulation is not physical validation. OpenMHS is not a functional-safety system.
- Arm D (the OpenMHS-governed client) is not built in this layer, so no claim about OpenMHS follows.
- Results depend on fault distributions chosen by the author and are not field incidence rates.

## Files

| File | Purpose |
|---|---|
| `PREREGISTRATION.md` | Hypotheses, arms, faults, trial counts and falsifiers, committed before any code (`9a29ea1`) |
| `DEVIATIONS.md` | Everything that differs from, or clarifies, the pre-registration |
| `RESULTS.md` | The confirmatory results and what they do and do not show |
| `model.py` | Plant with ground-truth ledger, action-server model, RPC and fault injection |
| `clients.py` | Arms A (naive), B (careful in-memory), C (careful durable), each without or with measurement |
| `channel.py` | Noisy independent measurement channel and the Bayesian resolution rule |
| `checker.py` | History checker with indeterminate operations (V1 to V4) |
| `stats.py` | Exact Clopper-Pearson, exact McNemar, Holm |
| `trial.py`, `run_study.py` | One trial; the full matrix and the summary |
| `test_harness.py` | 36 tests, including checker mutation tests |
| `results/confirmatory/` | `summary.json`, `run_record.json`, `trials.jsonl.gz` (14,000 trials) |

## Run it

Standard library only. Tested with Python 3.13.4 on Windows 11; other versions are untested.

```
python -I test_harness.py
python -I run_study.py --n 200 --base-seed 20261103 --out results/confirmatory
```

The run takes under a second and writes a byte-identical `trials.jsonl.gz` for the same seed.
