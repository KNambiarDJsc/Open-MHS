# Results: outcome verifier v0

**Simulation and hand-written evidence only. Nothing here was measured on a physical device.** The plan is
[EVALUATION.md](EVALUATION.md), committed (`1205749`) before any code. The design is
[RFC 0004](../../docs/rfcs/0004-outcome-contracts-and-evidence.md).

## Answers

| | Question | Result |
|---|---|---|
| Q1 | Does it apply its rules as written? | **Yes, 27 of 27.** Every corpus case got its expected verdict and reason ([corpus_verdicts.txt](results/corpus_verdicts.txt)). A negative control, two expectations deliberately broken, was caught by the test |
| Q2 | Is it deterministic? | **Yes.** Every case, with its records reordered 100 times, gave byte-identical reports (`tests/corpus.rs`) |
| Q3 | Any false `SATISFIED`? | **None** across the 27 cases |
| Q4 | Does it work on OpenMHS's own evidence? | **Yes, 7 of 7.** Virtual-lab runs, exported with `openmhs audit export` and read with `--audit`, with no hand-editing ([results/virtual_lab](results/virtual_lab/)) |
| Q5 | What does it add over the native status, here? | See below. Not a pass or fail, and not an answer to whether it helps on hardware |

## Q4 and Q5: the virtual lab

A fresh `openmhs serve --sim` per case, a scoped agent, the simulator's own faults, a 2 V setpoint write with a
correlation id, and reads of the supply's measured output. The contract asks for a measured output within
1.95 to 2.05 V, within 3 s after the command. Predictions were written in the script before the first run.

| Case | The command ended | Predicted | The verdict | Predicted | Why |
|---|---|---|---|---|---|
| Output on | `VERIFIED` | `VERIFIED` | `SATISFIED` | `SATISFIED` | Measured 1.999 and 2.001 V |
| **Output off** | **`VERIFIED`** | `VERIFIED` | **`VIOLATED`** | `VIOLATED` | The setpoint read back as 2 V, so the command verified; the output measured 0 V |
| **Setpoint readback off by 0.3 V** | **`VERIFICATION_FAILED`** | "FAILED" | **`SATISFIED`** | `SATISFIED` | The device misreports its setpoint; the output is 2 V |
| Readings stale by 5 s | `DENIED` | "not VERIFIED" | `UNKNOWN` | `UNKNOWN` | The gateway refused the write on stale readings; the measured values fell outside the window |
| Readings of bad quality | `DENIED` | "not VERIFIED" | `UNKNOWN` | `UNKNOWN` | `evidence.quality` |
| **Applied, then the reply was lost** | **`UNKNOWN_OUTCOME`** | `UNKNOWN_OUTCOME` | **`SATISFIED`** | `SATISFIED` | The output is 2 V, whatever happened to the reply |
| With a simulated DMM | `VERIFIED` | `VERIFIED` | `SATISFIED` | `SATISFIED` | Report notes the DMM is a simulator |

Every verdict matched its prediction. Two predicted receipt states were loose: I wrote "FAILED" for what is
`VERIFICATION_FAILED`, and "not VERIFIED" where the gateway in fact refused the command (`DENIED`) before it ran.

The three bold rows are the point of the exercise: a command's state and the outcome it was for can disagree in
both directions. **Within the simulator's model.** The supply's "measured" output is a formula in
`crates/openmhs-sim/src/psu.rs`: zero when the output is off, the setpoint plus noise when it is on. So the
"output off" row shows the verifier reading that formula correctly, not that a real supply behaves this way.

## Against a simple script

The strategy's 60-day gate asks whether the verifier beats "simple custom scripts". A deliberately naive script,
about fifteen lines (newest value per signal, conditions checked, no identity, time, clock, quality or calibration
rules, and no `UNKNOWN`), was run over the same corpus ([naive_baseline.txt](results/naive_baseline.txt)). It was
added after Q1 to Q4 had run; see Deviations.

| | Count of 27 |
|---|---|
| Agrees with the expected verdict | 12 |
| `SATISFIED` where the evidence could not establish it | **12** |
| `VIOLATED` where the evidence could not establish it | 3 |

The twelve false `SATISFIED` are every case where the value looked right but something else was wrong: expired or
missing calibration, a reading before the action or too late, another attempt's reading, another clock with no stated
uncertainty, too much clock uncertainty, a reading straddling the window, uncertain quality, no record of the command at all.

**What this does not show.** The corpus was written to exercise exactly these rules, so a script that ignores them must
fail on it; the result measures the rules, not the verifier's cleverness. A script written to the same rules would agree
with the verifier. The real question is whether those failures happen often enough, on real equipment, that people would
rather not write the rules themselves. Simulation cannot answer that.

## Evidence by kind

| Kind | What | Result |
|---|---|---|
| Unit, Rust | `tests/rules.rs`: input errors, anchors, operators, audit ingestion | 7 pass |
| Corpus, Rust | `tests/corpus.rs`: 27 cases, determinism, provenance | 4 pass |
| CLI | `openmhs outcome verify` on every corpus case | 27 of 27 as expected; exits 0, 3, 4 |
| Integration | Virtual lab through the real gateway and `openmhs audit export` | 7 of 7 verdicts as predicted |
| Regression | `cargo test --workspace`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings` | See the commit; clippy and fmt clean |

## Not tested

Any physical device or instrument. Any real independent sensor: the corpus DMM and camera are typed by hand, and the
virtual-lab DMM is the harness. ROS 2: the pick-and-place cases are ROS 2-shaped records, not ROS 2, because the
WSL environment did not start on a full C: drive. Evidence from another system's own export. Clocks that really differ.
Large evidence sets. Signed evidence. Whether a person reading an `UNKNOWN` report knows what to do next.

## Deviations from the plan

- **The RFC was refined while writing the code, before any result.** Rule 4 now treats an unknown clock uncertainty as a
  problem only across clocks, and items gained `bound: false` for a device's own readings, which cannot know their attempt.
- **`evidence.rejected` was split from `evidence.missing`** after the first corpus run showed "missing" reported for
  records that existed and failed a rule. Same verdicts; clearer reasons.
- **The virtual-lab contract does not require a DMM in six of seven cases.** EVALUATION.md planned harness-side DMM faults.
  On reflection a harness DMM in the virtual lab only echoes the simulator, so it would test nothing the corpus does not.
  The DMM faults are in the corpus instead, and one virtual-lab case keeps a DMM to show mixed sources.
- **An "output off" case was added**, after reading the simulator: a natural case where a command verifies and the outcome
  does not hold.
- **The naive-script comparison was not in the plan.** It was added after Q1 to Q4, before it was run.
