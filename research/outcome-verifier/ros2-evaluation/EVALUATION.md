# Evaluation plan: outcome verification on genuine ROS 2 data

Written and committed before the experiment below was run. Changes afterwards go under Deviations in `RESULTS.md`.

## Question

Can OpenMHS determine whether a robot actually reached the place it believes it reached, using recorded evidence,
when its own belief is wrong or the evidence is incomplete? And does it tell us more than a short script would?

## Why this data, and what it cannot show

The best experiment for the wider question (a native action status that says `SUCCEEDED` when the physical outcome did
not happen) needs a ROS 2 simulator or robot running. On this machine WSL cannot start while C: is full, so that
experiment waits. No public ROS 2 recording was found that contains a robot task's action status together with an
independent measurement of its outcome (see `DATASETS.md`).

What can run now is a genuine ROS 2 recording from a real vehicle: Málaga 2009 `parking_0L`, a ROS 2 MCAP converted in
2026 from the 2008 logs, CC BY 4.0, MD5-checked. It has a consumer-grade GPS, three RTK GPS receivers, and a 100 Hz
ground truth fused from the RTK receivers. It has **no action, goal or status topic**. So this experiment asks about a
robot's own **position belief**, not about an action result.

| What this tests | What it does not test |
|---|---|
| The bridge on real ROS 2 messages, real stamps, real frames | Any ROS 2 action status or goal |
| A native belief (consumer GPS) against independent evidence (RTK) | Manipulation, or a task with a physical object |
| Natural gaps in the reference (RTK dropouts) | Simulation time, `/clock` |
| Injected faults on real records | Whether a real robot's controller would have behaved this way |

**The reference and the evidence share a source.** The labels below come from the published ground-truth text file, and
the verifier's evidence is the same ground truth as a ROS 2 topic. On clean data the verifier and the labels will agree
by construction. That part only checks that the rules handle real timing and gaps. The interesting parts are the native
belief against the reference, and what the rules do with faults.

## The task

A robot steering by its consumer GPS believes, at each fix, that it is at the position the fix gives. To be fair to it,
the GPS's constant offset is estimated while the vehicle stands still at the start (the first 6 s), as a practitioner
would, and recorded as a calibration valid for this run. Each later fix is a claim: "I am at P_k".

The outcome contract for claim k: the RTK ground truth places `base_link` within ±3 m of P_k on both axes, in frame
`map`, within ±50 ms of the claim, and the GPS offset calibration was valid at the claim.

## Labels

From `malaga2009_parking_0L_gt_tum.txt`, parsed directly: the nearest ground-truth sample within ±50 ms of the claim.
Inside the box: `ARRIVED`. Outside: `NOT_ARRIVED`. No sample within ±50 ms: `NO_REFERENCE`.

## What is compared

| | Says |
|---|---|
| Native | The robot's belief: always "arrived" |
| Script | The nearest ground-truth record within ±50 ms in the converted evidence, box check; "no data" if none. A reasonable script, not a straw man |
| Verifier | `openmhs outcome verify` on the contract |

## Predictions, before running

1. The native belief will be wrong for a substantial share of claims, because consumer GPS error is metres. The number is
   unknown in advance.
2. On clean data the script and the verifier will agree on every claim except near the box edge, where the verifier needs
   all samples in its window to agree and may say `UNKNOWN` (`evidence.conflict`).
3. During the 17 s RTK gap and the shorter gaps, the verifier will say `UNKNOWN` and the script "no data".
4. **The verifier will have no false `SATISFIED` against the labels.** One would be a failure of this evaluation.
5. Injected faults, each on the same fixed set of claims:

| Fault (injected) | Verifier expected | Script expected |
|---|---|---|
| Ground truth relabelled as frame `odom` and rotated 90° | `UNKNOWN`, `evidence.frame` | Uses the rotated numbers: wrong where the label is `ARRIVED` |
| Ground truth near the claim removed (a gap) | `UNKNOWN` | "no data" |
| Ground truth on another clock, 1.5 s off, no stated uncertainty | `UNKNOWN`, `time.clock_domain` | Picks a sample 1.5 s away: sometimes wrong |
| GPS offset calibration expired before the claim | `UNKNOWN`, `calibration.expired` | Ignores it |
| Claim carries another attempt's id | `UNKNOWN` (no anchor) | Ignores it |
| A contradicting ground-truth sample 5 m away inside the window | `UNKNOWN`, `evidence.conflict` | Depends on which sample is nearest |
| Records reordered | Same report as clean | Same answer as clean |

## Also measured

Conversion time and size; verification time per claim; byte-identical reports across two runs; and that the verdict on a
claim is the same whether the verifier is given the whole evidence file or only the records within 2 s of the claim
(the experiment uses the latter for speed).

## Failure conditions

Any false `SATISFIED` from the verifier against a label. Any non-deterministic report. Any fault case whose verifier
verdict differs from the table above.
