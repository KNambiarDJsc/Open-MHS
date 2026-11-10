# Evaluation plan: outcome verifier v0

Committed before any result. Changes after this commit are listed under Deviations in `RESULTS.md`.

## What is being tested

The verifier in [RFC 0004](../../docs/rfcs/0004-outcome-contracts-and-evidence.md), and nothing about the physical
world. Simulation cannot show that independent evidence catches real failures (RFC 0004, "What simulation can and
cannot show"). This evaluation asks narrower questions, which simulation can answer.

| | Question | Pass if |
|---|---|---|
| Q1 | Does it apply its rules as written? | Every case in the fault corpus gets the expected verdict and the expected reason code |
| Q2 | Is it deterministic? | The same inputs, with records reordered, give byte-identical reports, 100 times |
| Q3 | Does it ever say `SATISFIED` when a rule should have stopped it? | Zero false `SATISFIED` across the corpus |
| Q4 | Does it work on OpenMHS's own evidence? | A virtual-lab run, exported with `openmhs audit export`, is verified without hand-editing |
| Q5 | What does it add over the native status, in these cases? | Reported per case, as a table. Not a pass or fail: the answer is only meaningful on hardware |

## The fault corpus

Each case is a contract, an evidence file and an expected verdict and reason, in `research/outcome-verifier/corpus/`.
Two domains, as RFC 0004 asks for at least two:

**Bench supply** (a setpoint, the device's readback, an independent DMM):

| Case | Expected |
|---|---|
| All agree, calibrated, in window | `SATISFIED` |
| DMM reads 0 V, device reads back 2 V | `VIOLATED` |
| Device reads back 2.3 V | `VIOLATED` |
| DMM calibration expired | `UNKNOWN`, `calibration.expired` |
| No DMM calibration record | `UNKNOWN`, `calibration.missing` |
| DMM reading taken before the command | `UNKNOWN`, `evidence.outside_window` |
| DMM reading 5 s after the command, window 2 s | `UNKNOWN`, `evidence.outside_window` |
| DMM on another clock, no uncertainty stated | `UNKNOWN`, `time.clock_domain` |
| DMM clock uncertainty 200 ms, limit 50 | `UNKNOWN`, `time.uncertainty` |
| DMM reading straddles the window edge given its uncertainty | `UNKNOWN`, `time.ambiguous` |
| DMM reading for a different attempt | `UNKNOWN`, `evidence.identity_mismatch` |
| DMM quality `UNCERTAIN` | `UNKNOWN`, `evidence.quality` |
| Two DMM readings in window disagree (1.99, 0.0) | `UNKNOWN`, `evidence.conflict` |
| DMM in mV, in range after conversion | `SATISFIED` |
| DMM in a unit with no conversion to V | `UNKNOWN`, `evidence.unit` |
| No command record (anchor missing) | `UNKNOWN`, `timing.anchor_missing` |
| Command `UNKNOWN_OUTCOME`, but readback and DMM show 2 V | `SATISFIED`: the outcome is established even though the command's is not |

**Pick and place** (synthetic records shaped like ROS 2: an action result, gripper state, an overhead camera pose):

| Case | Expected |
|---|---|
| Action `SUCCEEDED`, gripper open, pose in tray, calibrated | `SATISFIED` |
| Action `SUCCEEDED`, object never grasped: pose unchanged, outside tray | `VIOLATED` |
| Object dropped outside the tray | `VIOLATED` |
| Camera has no pose | `UNKNOWN`, `evidence.missing` |
| Camera pose taken before the action ended | `UNKNOWN`, `evidence.outside_window` |
| Camera calibration expired | `UNKNOWN`, `calibration.expired` |
| Camera reports a different object id | `VIOLATED` |
| Gripper still closed | `VIOLATED` |
| Action `ABORTED`, but camera shows the object in the tray | `SATISFIED` (the contract does not require the controller to agree) |
| Camera pose from a simulator, everything else fine | `SATISFIED`, with `provenance: simulator` visible in the report |

These synthetic records are typed by hand. They test the rules, not ROS 2.

## The virtual-lab run (Q4, Q5)

`openmhs serve --sim`, the simulated supply, a command with a correlation id, reads, and a DMM record written by the
harness. The audit export is converted and verified. Injected with the simulator's own faults (`readback-offset`,
`stale`, `bad-quality`, `apply-then-hang`) and with harness-side DMM faults. For each case the table reports: the
command's state, the verdict, and the reason. The DMM is simulated, so where it disagrees with the device it is
because the harness said so.

## What would change the plan

If a corpus case's expected verdict turns out to be wrong on reflection, it is changed and listed under Deviations,
with the reason. Expected verdicts are not changed to match what the code does.
