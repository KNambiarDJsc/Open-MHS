# Results: outcome verification on a genuine ROS 2 recording

**Real hardware, recorded in 2008, converted to ROS 2 MCAP in 2026, replayed offline. Not live. No ROS 2 action status
was available.** The plan is [EVALUATION.md](EVALUATION.md), committed (`a8e9296`) before the run.

## What was run

| | |
|---|---|
| Data | Málaga 2009 `parking_0L`, `malaga2009_parking_0L.mcap`, MD5 `308bb33977ffb12f76303158e1072b38` (matches Zenodo), CC BY 4.0. 255 s of an electric vehicle in a car park |
| Bridge | `ros2_evidence.py` with `mappings/malaga2009_parking_0L.json`: 47,104 records (100 Hz ground truth x and y, consumer GPS and rear RTK latitude and longitude) in about 5 s, 17 MB. Bag SHA-256 in [conversion.meta.json](results/malaga2009_parking_0L/conversion.meta.json) |
| Verifier | `openmhs outcome verify`, OpenMHS branch `feat/outcome-verifier` |
| Tools | Windows 11, Python 3.13.4, `mcap` 1.6.0, `mcap-ros2-support` 0.6.0. No ROS 2 installation was used: the MCAP carries its message definitions |

## Checks on the data before using it

| Check | Result |
|---|---|
| Is the ground truth's `map` frame east-north-up at the published GPS origin? | **Yes.** The rear RTK antenna, which `/tf_static` puts directly above `base_link`, converted to local east/north at the origin in `/rawlog_comment`, matches the ground truth to 9 mm median and 42 mm worst over 239 fixes |
| Do the receivers agree on where the vehicle is? | **No, naturally.** At the start the consumer GPS is 259.7 m east and 19.3 m south of the RTK position. The dataset's own comment lists similar offsets of about 260 m for two of the three RTK receivers, corrected when its ground truth was computed. Every `NavSatFix` says frame `gps_*` and covariance type "unknown": the messages do not say any of this |
| Is the consumer GPS error real or my calibration's? | **Real.** Its north offset drifts from -19 m at the start to about -49 m within a minute. No time lag explains it (0 to 5 s tried). The best constant offset fitted over the whole run in hindsight still leaves a median error of 5.9 m, p90 15.5 m ([gps_belief_check.txt](results/malaga2009_parking_0L/gps_belief_check.txt)) |

## The task results (249 claims)

A robot steering by its consumer GPS, calibrated during the 6 s standstill at the start, claims "I am at P" at each fix.
Labels come from the published ground-truth text file. The verifier's evidence is the same ground truth, recorded as a ROS 2
topic, so on clean data **agreement with the labels is expected by construction**.

| Label | Count | Native belief | Script | Verifier |
|---|---|---|---|---|
| `ARRIVED` (within ±3 m) | 10 | arrived | `SATISFIED` 10 | `SATISFIED` 10 |
| `NOT_ARRIVED` | 222 | **arrived (wrong)** | `VIOLATED` 222 | `VIOLATED` 222 |
| `NO_REFERENCE` (natural RTK gaps) | 17 | **arrived (unchecked)** | "no data" 17 | `UNKNOWN` 17 |

- **The native belief was wrong for 222 of 232 checkable claims (95.7%)**, median error 27.8 m with the start-of-run
  calibration. With the hindsight best offset the median would be 5.9 m, still outside a 3 m box. That is a finding about
  this 2008 receiver, not about OpenMHS.
- **Script and verifier agreed on all 249.** On clean data the verifier added nothing a reasonable script did not.
- **No false `SATISFIED`** from the verifier.
- `UNKNOWN` reasons in the gaps: `evidence.missing` (12) or `evidence.outside_window` (5).
- Report determinism: 21 claims verified twice, byte-identical. Using only records within 2 s of a claim gave the same
  verdict as the whole file on all 9 claims checked. Median verification time 0.09 s per claim, including process start.

## Injected faults (160 trials)

On copies of real records, every `ARRIVED` claim (10) and ten `NOT_ARRIVED` claims spread over the run (20 claims), eight
faults each. **All injected; none of these happened in the data.**

| Fault | What it changes | Verifier | Script right | Script false "arrived" | Verifier false `SATISFIED` |
|---|---|---|---|---|---|
| Reference in frame `odom`, rotated 90° about the origin | The numbers | `UNKNOWN` 20, `evidence.frame` | 14 of 20 | 0 | 0 |
| Reference in `odom`, rotated 30° and shifted (added after run 1) | The numbers | `UNKNOWN` 20, `evidence.frame` | 10 of 20 | **1** | 0 |
| Reference on another clock, 1.5 s off, no stated uncertainty | The numbers | `UNKNOWN` 20, `time.clock_domain` | 19 of 20 | **1** | 0 |
| A contradicting reference sample 5 m away | The numbers | `UNKNOWN` 10, `VIOLATED` 10 | 20 of 20 | 0 | 0 |
| Reference removed within 1 s of the claim | Availability | `UNKNOWN` 20 | "no data" 20 | 0 | 0 |
| GPS offset calibration expired | Metadata only | `UNKNOWN` 20, `calibration.expired` | **20 of 20** | 0 | 0 |
| Claim carries another attempt's id | Metadata only | `UNKNOWN` 20, `evidence.identity_mismatch` | **20 of 20** | 0 | 0 |
| Records reordered | Nothing | as clean, 20 | 20 of 20 | 0 | 0 |

**Reading this honestly:**

- Where a fault changes the numbers a script reads, the script was wrong 18 times in 80 and said "arrived" falsely twice.
  The verifier refused each time and said why. That is the value shown here.
- Where a fault changes only metadata, the script was right 40 of 40 and the verifier said `UNKNOWN`. The numbers were
  untouched, so ignoring the metadata happened to work. The verifier's refusal there is caution, and it costs an answer.
  Whether that caution is worth it depends on how often metadata problems hide value problems in practice, which this
  data cannot say.
- The frame faults are only meaningful because the bridge keeps `header.frame_id` and the contract names `map`. A script
  that ignores frames cannot notice; it can only be lucky.

## Against the research question

> Can OpenMHS determine whether a robotic task actually achieved its intended physical outcome, using observable evidence,
> even when the native command or action status is misleading, incomplete, or unavailable?

| Part | What this experiment shows |
|---|---|
| Native status misleading | **Yes, for a position belief.** The robot's own belief was wrong 222 of 232 times; the verifier, given independent RTK evidence, caught every one. A reasonable script caught them too |
| Native status incomplete | **Yes.** Through 17 natural ground-truth gaps the verifier said `UNKNOWN` with a reason, never "arrived" |
| Native status unavailable | **Not tested.** There was no action status to lose |
| An actual task with an action result | **Not tested.** This data has no goal or action. That is the next experiment |
| Better than a short script | **Only under faults that change the numbers, and only by refusing.** On clean real data, no |

## Not tested

A ROS 2 action status, goal, or result. Manipulation or any object. Simulation time and `/clock`. A live ROS 2 graph.
Any OpenMHS gateway in the loop (this is the evidence-only mode). Clocks that really differ: all records here are assumed to
come from one logging host, which the dataset README does not confirm. Independence beyond what the mapping file states:
the ground truth and the rear RTK fix come from the same receivers.

## Deviations from the plan

- **Fault targets** were one claim in the first run (the plan's selection, every tenth `ARRIVED` claim, left one). The second
  run uses all ten `ARRIVED` claims and ten `NOT_ARRIVED` claims. Both runs' clean results are identical.
- **A second frame fault was added** after the first run, because a rotation about the origin barely moves positions near
  the origin, where most `ARRIVED` claims are. The planned rotation was kept.
- **Conflicting reference:** the plan expected `UNKNOWN` (`evidence.conflict`). For the ten `NOT_ARRIVED` claims, both
  readings fall outside the box, so the rules give `VIOLATED`, which is correct. The plan's expectation had been written
  for `ARRIVED` targets only. This is a mismatch with the plan as written.
- **A reporting bug was found and fixed during the run.** When the claim itself was rejected (expired calibration, another
  attempt), the report said only `timing.anchor_missing`. It now includes why the anchor was rejected. The verdicts did not
  change; a unit test covers it.
- **The verifier gained an optional `frame`** on records and items, so case 6 (wrong coordinate frame) could be expressed at
  all ([RFC 0004](../../../docs/rfcs/0004-outcome-contracts-and-evidence.md), rule 2).
