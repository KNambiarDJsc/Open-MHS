# Results: real drone flights, action status against motion capture

**Real hardware, recorded in October 2026, replayed offline. Not live. The failures in these flights were induced by the
experimenters; none is an organically observed field failure.** The plan is [EVALUATION_CORESENSE.md](EVALUATION_CORESENSE.md),
committed (`d153b1e`) before the measured run. Every number below is in [results/coresense_aerostack2/](results/coresense_aerostack2/).

## What was run

| | |
|---|---|
| Data | CoreSense Inspection Testbed ROSbag Dataset: three Crazyflie drones, Aerostack2, OptiTrack motion capture. Zenodo [10.5281/zenodo.23243295](https://doi.org/10.5281/zenodo.23243295), CC BY 4.0, MD5 verified. Three flights (`no_failure`, `panel_failure`, `double_failure`), rosbag2 SQLite, 41 to 47 goals each |
| Claims | 124 goals that reported `SUCCEEDED`: 107 `GoTo`, 9 `Takeoff`, 8 `Land`. `CANCELED` goals are not claims |
| Contracts | Fixed in the plan: `GoTo` x and y within ±0.20 m of the waypoint; `Takeoff` z within ±0.25 m of 1.5 m; `Land` z ≤ 0.15 m; all samples within ±50 ms of the claim, frame `earth` |
| Reference | `self_localization/pose`, from the motion-capture system **that also feeds the controller**. Independent of the claim, not of the sensor |
| Labels | A separate direct read of the bag: the pose sample nearest the claim, box check |
| Verifier | `openmhs outcome verify`, branch `main` at `98ef126` plus this work |

## Planned run

| Behavior | Claims | At target (label) | Script right | Verifier `SATISFIED` | `VIOLATED` | `UNKNOWN` | Error at claim (m): median / max |
|---|---|---|---|---|---|---|---|
| GoTo | 107 | 9 | 107 | 4 | 96 | 7 | 0.265 / 0.289 |
| Takeoff | 9 | 9 | 9 | 9 | 0 | 0 | 0.193 / 0.195 |
| Land | 8 | 6 | 8 | 6 | 2 | 0 | 0.067 / 0.408 |

| Flight | Claims | `SATISFIED` | `VIOLATED` | `UNKNOWN` |
|---|---|---|---|---|
| no_failure | 41 | 7 | 32 | 2 |
| panel_failure | 42 | 6 | 33 | 3 |
| double_failure | 41 | 6 | 33 | 2 |

- **No false `SATISFIED`** from the verifier. The verifier never contradicted a label, except on 7 `GoTo` claims where it said
  `UNKNOWN` (`evidence.conflict`, `evidence.outside_window`): the drone was moving and the samples in the ±50 ms window
  straddled the edge of the box. The plan predicted this.
- **The script agreed with the labels on all 124 claims.** On clean data the verifier added nothing a reasonable script
  did not, as in both earlier experiments.
- Reports were byte-identical when verified twice (13 claims checked). Median verification 0.11 s per claim.

## Prediction 1 was wrong

The plan predicted that most `SUCCEEDED` goals would be `SATISFIED`. For `GoTo` only 4 of 107 were. The tolerance was fixed
before the run, and it stays: **no verdict was changed afterwards.**

| GoTo tolerance (sensitivity, labels only) | Within tolerance at the claim |
|---|---|
| 0.10 m | 3 of 107 |
| 0.20 m (planned) | 9 of 107 |
| 0.30 m | 107 of 107 |

## Post-hoc: what is going on with GoTo (exploratory, not in the plan)

[post_hoc.json](results/coresense_aerostack2/post_hoc.json), from `coresense_post_hoc.py`:

- At `SUCCEEDED` the drone is 0.264 m from the waypoint (median; maximum 0.287 m). **0.5 s later it is 0.12 m away, and its
  closest approach within 2 s is 0.023 m (median).** The error vector has no fixed direction (spread 0.13 to 0.20 m on each
  axis), so this is not a calibration offset between the motion-capture frame and the waypoints. The drones arrive
  essentially exactly; the behavior reports success about a quarter of a metre early, while still closing in.
- Aerostack2's default `go_to_threshold` is 0.2 m (`config_default.yaml`, `main` branch, retrieved 2026-11-16). The
  reported successes sit up to 0.29 m away, so these flights either ran with a looser threshold or measure position
  differently from the motion-capture feed. **The dataset publishes neither the configuration nor the software revision,
  so this cannot be settled from the data.** That is the case for recording a configuration digest in the deployment
  manifest, and the manifest here says "revision not published by the dataset".
- What it means for the contract: the status and the contract disagree because they mean different things by "arrived".
  The contract was written for "at the waypoint when success is reported"; the behavior means "close enough, and still
  approaching". Neither is wrong, but a verifier is only as useful as its contract matches the requirement.

## Takeoff and landing

- All nine takeoffs were reported successful 0.19 m below the commanded 1.5 m (error 0.188 to 0.195 m), inside the planned
  ±0.25 m. The same early-success pattern.
- **Three of the eight landings have a frozen pose feed.** After the landing goal succeeded, `drone2` in two flights and
  `drone1` in the third repeated one single pose for 2.1 s, 14.8 s and 1.5 s while the header stamp kept advancing (250,
  1,779 and 182 samples, 1 distinct pose each). Two of them held a height of 0.262 m and 0.408 m, which is where the
  verifier said `VIOLATED`; the third held 0.044 m and the verifier said `SATISFIED`. **None of the three verdicts is
  evidence about the landing.** A stale value with a fresh stamp passes every rule in RFC 0004, as the plan said it would;
  this is the first time it happened on real data. Whether those drones sat at that height or the tracking was lost cannot
  be told from the pose alone. The other five landings are live feeds, all within 0.04 to 0.09 m of the floor.

## Injected faults (496 trials, all injected; none happened in the flights)

| Injected fault | Trials | Verifier | Reasons | Script right | Script false `SATISFIED` | Verifier false `SATISFIED` |
|---|---|---|---|---|---|---|
| wrong_frame | 124 | `UNKNOWN` 124 | evidence.frame, evidence.rejected | 115 | 0 | 0 |
| other_clock | 124 | `UNKNOWN` 124 | evidence.rejected, time.clock_domain | 100 | 1 | 0 |
| reference_gap | 124 | `UNKNOWN` 124 | evidence.outside_window, evidence.rejected | 0 | 0 | 0 |
| other_attempt | 124 | `UNKNOWN` 124 | evidence.identity_mismatch, timing.anchor_missing | 124 | 0 | 0 |

Every verifier verdict matched the plan. Read the script column with care: 100 of the 124 labels are "not at target",
so a script that answers `VIOLATED` is "right" by luck whenever the numbers are scrambled. The number that matters is
false `SATISFIED`: the script gave one, the verifier none. `other_attempt` changes metadata only, so the script was
right every time and the verifier's `UNKNOWN` cost an answer, as before.

## Against the research question

| Part | What this experiment shows |
|---|---|
| Native status misleading | **Depends on the requirement.** At the planned 0.20 m, 96 of 107 `GoTo` successes did not hold at the moment of the status; a moment later the drones were within a few centimetres. The status meant something looser than the contract |
| A reference that fails | **Yes, naturally:** a pose feed that froze on 3 of 8 landings, undetectable by timestamp rules |
| Better than a script | **No, on clean data** (the script matched all 124 labels). **Yes under injected faults** (one false `SATISFIED` from the script, none from the verifier) |
| Physical hardware | **Real drones, a real motion-capture system, replayed offline.** The reference shares its sensor with the controller, and the failures were induced |

## Not tested

Anything live. A reference independent of the controller's sensor. The drone failure itself: the swarm reported
`droneStatus failed` for `drone0` in the double-failure flight, and nothing here verifies that. Other swarms, sites or
software revisions. The three flights are one setup, repeated; no claim about field rates.

## Deviations from the plan

None to the planned run. The post-hoc analysis, the Aerostack2 default threshold and the frozen-feed finding were added
afterwards and are labelled as such. The one tolerance that proved too strict was not changed.
