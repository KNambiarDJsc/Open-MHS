# First slice: results (2026-11-15)

**SIMULATION ONLY.** Seven recorded runs of the OpenAMRobot upper body in Gazebo, imported as events. No physical
robot, no design partner data yet.

## What was built

| Part | Where | Tests |
|---|---|---|
| Envelope, manifests, artifacts; canonical JSON and digests | `crates/openmhs-events` | 15 unit tests |
| Store with numbered, checksummed migrations; idempotent ingest (accepted / duplicate / conflict / rejected per record); task attempts; correlation rules `corr-1`; incident timelines; release comparison with Wilson, Newcombe and Mantel-Haenszel; `insufficient_evidence` | `crates/openmhs-ops` | 12 tests incl. a property test (any order, any duplication → identical derived tables), 7 slice acceptance tests on imported Gazebo runs |
| `openmhs ops ingest / rebuild / status / incidents / show / compare` | `crates/openmhs-cli/src/ops.rs` | through the slice tests and the run below |
| Gazebo run importer (standard library only) | `python/openmhs/importers/openamr_gazebo.py` | 6 tests on the committed run results |
| Per-goal tolerance option for the task driver (release B) | `research/.../gazebo/task_driver.py` | three recorded runs |

Workspace: 657 Rust tests pass, `clippy -D warnings` and `cargo fmt --check` clean.

## The runs

| Release label | Runs | What differs |
|---|---|---|
| `engine-bullet` | bullet1-3 (2026-11-10) | The stack as shipped: the task client sends no goal tolerance, and `lift_controller`'s own goal tolerance is 0.0, which means "no check" |
| `engine-dartsim` | dart1 (2026-11-10) | Physics engine DART |
| `tol-0.01` | tol1-3 (2026-11-15) | The task client sends a 0.01 m goal tolerance with every lift goal. Nothing else changed (one manifest component differs) |

## Results

Imported into one store: 1,304 records → 1,290 events, 3 manifests (identical runs collapse to one), 7 artifacts
(bags, by reference), 98 task attempts, 51 incidents. Re-importing the same file: 1,304 duplicates, nothing changed.

**Incidents** open exactly where the action reported `SUCCEEDED` and the verifier found the lift elsewhere: the same
five goals in each Bullet run (the ones faster than the URDF's 0.018 m/s limit), eight under DART, none in `tol-0.01`.
Each timeline joins the action and task events by task id, the verdict by the ROS 2 goal UUID, and the deployment by
time window, and links the bag cut to the incident's window. Odometry verdicts (component `base`) join drive tasks only,
by window, never lift tasks.

**Release comparison, lift, `engine-bullet` → `tol-0.01`.** The 30 attempts per release are 10 scenarios (the task
list's ten lift goals) run three times each, so the tool counts 10 scenarios per release, not 30 trials:

| Outcome used | A failed | B failed | Difference (95 % CI) | Verdict |
|---|---|---|---|---|
| Reported by the action | 0 / 10 | 0 / 10 | +0.0 (−27.8 to +27.8) | `insufficient_evidence` (no failures; 10 scenarios) |
| Verified (RFC 0004) | 5 / 10 | 0 / 10 | −50.0 points (−76.3 to −11.7) | `insufficient_evidence`: fewer than 30 scenarios per arm |

`--per-attempt` counts every attempt and gives the earlier, overconfident picture (15/30 against 0/30, −50 points,
−66.8 to −29.7, "improvement"), with the unit shown as `attempt`. The honest reading: the point estimate is large and
the interval excludes zero, but ten scenarios are too few for the tool to call it, and three deterministic replays add
no information beyond the first.

A release dashboard built on the action's own status would have seen no change at all. The fix only shows in verified
outcomes. That is the case for keeping the two apart (ADR-0033).

What the B runs also showed:

- With a tolerance, the controller waited for the lift: L06 reported success after 4.5 s instead of 2.0 s, L07 after
  12.8 s instead of 8.0. The comparison now reports task duration per arm: median 2.5 s for A and 3.5 s for B, 90th
  percentile 4.5 s and 5.9 s (these figures were first written down with a quantile that took the upper of the two middle
  values for an even count; the tool now interpolates). Release B trades time for correctness.
- B's errors at the reported success were 9.2 to 9.9 mm, just inside the verifier's 10 mm contract, because the
  controller declares success as soon as the lift is within its own 10 mm tolerance, while still moving. A 5 mm
  contract would fail most of them.

## Limits, stated plainly

- **Replicates are not trials.** The importer tags each goal with a scenario id (`lift/L03` in every run), and the
  tool counts scenarios when attempts repeat them (ADR-0032). Without scenario ids it cannot tell, and says nothing.
- Simulation time restarts at zero in each run, so each run is its own simulated robot and clock domain; times print
  as 1970 dates.
- The "verified" outcome comes from Gazebo's own poses: independent of the controllers, not of the simulator.
- Demo 2 was built later (see "A learned policy under two samplers" below): a sampler comparison on one checkpoint. Not
  done: recovery from a failed episode, and a comparison of two different checkpoints.

## Follow-up issues

Done after the first review round (this branch): scenario ids with scenario-level counting (26), duration in
comparisons (27), concurrent events marked on timelines (28).

| # | Title | Why |
|---|---|---|
| 29 | ~~Demo 2 on LeRobot~~ (done, sampler comparison); Isaac Lab-Arena results import | The chosen users run learned policies |
| 30 | ~~Pair attempts across arms by scenario~~ (done: `ops compare --paired`, 2026-11-16) | Matched scenarios give a tighter interval only when the same scenarios fail in both arms; see the policy comparison below |

## Real flights (2026-11-16)

The first non-simulated recording: three Crazyflie drones under motion capture, from the CoreSense dataset
([Zenodo 10.5281/zenodo.23243295](https://doi.org/10.5281/zenodo.23243295), CC BY 4.0). Three flights, one with no failure,
one with an induced panel failure, one with a panel and a drone failure. **The failures were induced by the experimenters.**

- The bags are rosbag2 SQLite, which the MCAP bridge cannot read. `python/openmhs/importers/cdr.py` decodes the three
  standard messages needed (checked against the reference encoder) and `coresense_aerostack2.py` imports them: one mission
  task per drone, every goal as an action keyed by its UUID, the swarm's own failure report as a fault, auctions as markers,
  one manifest for all flights (the dataset publishes no software revision, and the manifest says so).
- **Result:** 9 missions, 8 succeeded, 1 failed. Goal outcomes: 41 succeeded in the clean flight; 45 succeeded and 3 canceled
  with the panel failure; 47 and 6 with the double failure. **One incident**: `drone0` in the double-failure flight. Its timeline
  shows the swarm reporting `droneStatus failed` inside a go-to goal, and that goal `CANCELED` about 3.8 s later; auction markers
  follow, and the swarm's knowledge base (not imported as events) shows the failed drone's waypoints handed to the other two. The panel-failure flight cancelled goals on all three drones
  and every drone landed; cancellation alone is not a failure, so no incident opens for it.
- The same flights checked against motion capture are in
  [RESULTS_CORESENSE.md](../../../research/outcome-verifier/ros2-evaluation/RESULTS_CORESENSE.md).
- **Verified outcomes in the incident layer** (`--verify`, 2026-11-16). The importer can run the deterministic verifier on
  every succeeded Takeoff, GoTo and Land (`coresense_verify.py`, the code the evaluation used, which reproduces the
  published 124 claims and 496 fault trials exactly) and emit the verdicts as `outcome.verified` events joined to their
  goals by id. The thresholds are declared in the manifest, with their basis ([ADR-0033](../../adr/0033-outcome-provenance.md),
  amendment), so the verified import is a different release (`aerostack2-unversioned+thresholds`). Verdicts: GoTo 4
  satisfied, 7 unknown, 96 violated; Takeoff 9 satisfied; Land 6 satisfied, 2 violated. **Incidents go from 1 to 9**: eight
  missions that reported success now carry `reported_verified_disagree`, and the failed one `verified_violated` as well.
  Read this carefully. The GoTo threshold (0.20 m) is an *assumption* (the flights' configuration is not published), the
  claim is the success status at its instant, and the evaluation found the drones reach the waypoint to a median of 2.3 cm
  half a second later: the behavior reports success early, relative to that threshold. That is a real property of these
  flights and not a fault of the drones' landing; and an incident per mission is coarse (every long mission has some
  violated goal), so a rate of violated goals would say more than an incident count. The aggregation was not changed to
  make the result look better or worse.
- **The shape of each failure** (2026-11-17, [ADR-0035](../../adr/0035-failure-shape.md), rules committed before this
  run). The verdicts now carry the verifier's own values, so each incident says how it went wrong, from typed fields
  only. On these flights: every incident first goes wrong at a GoTo goal reported succeeded, 22 to 46 % into the mission,
  in an order the single recorder clock can establish. Each violated GoTo missed one axis, never both (35 on x, 61 on y),
  by at least 0.7 mm to 7.3 cm beyond the assumed 0.20 m at the instant of the success report: the early report above,
  now measured goal by goal. The two landings reported done were 11 and 26 cm above the 0.15 m limit. On the drone the
  swarm reported failed, the first miss is at 17.5 s (39 %), a GoTo is canceled at 28.0 s and the fault comes 22.3 s after
  the first miss. The shape names no cause. Its signature splits the early-report pattern in two by which axis missed
  first (5 and 4 incidents), a limit of `shape-1` recorded in the ADR and left as found.

## A learned policy under two samplers (2026-11-16, simulation)

A pretrained LeRobot diffusion policy on PushT, two releases that differ only in the sampler (DDPM with 100 steps as
published, DDIM with 10), the same 20 starting states. **Simulation only**: no robot, no human. Plan committed before the
run; results and the predictions that held and did not are in
[RESULTS.md](../../../research/outcome-verifier/policy-demo/RESULTS.md).

- 15 of 20 scenarios succeeded under the published sampler (inside the pre-registered window around the published
  figure), 12 of 20 under the 10-step sampler. `ops compare --paired`: difference +15.0 points, 95 % interval -13.4 to
  +40.4, McNemar exact p = 0.51, **`insufficient_evidence`** (20 matched pairs, 30 required). The minimum was not lowered.
- The faster sampler was about 6.5 times quicker per episode (wall clock, shared machine).
- **Pairing did not narrow the interval**: the two arms' failures overlapped no more than chance (2 scenarios against 2.0
  expected), so the paired and unpaired intervals are identical. The tighter-interval case needs the same scenarios to fail
  in both arms; this run does not show that they do.
- **How far each failure got** (2026-11-17, [ADR-0036](../../adr/0036-reported-measurements.md), rules committed before
  the run). The simulator's own measurement now travels with its reported outcome and the shape reads it, labelled
  `reported`: 11 of the 13 failed episodes peaked within 0.04 of the success threshold's reward, one (`ddim-10`, seed
  1014) 0.29 short, one (`ddpm-100`, seed 1001) 0.90 short, the block barely moved. Same signature for all 13; the
  amount is what separates a near miss from no movement.
- Found gaps: the comparison's duration was simulated time, so the compute cost that separates the releases was not in
  the report (closed the same day: events can carry `cost.wall_s`, and `ops compare` prints wall time per arm, here a
  median of 742 s against 128 s); incidents carried no failure shape, near miss against no movement (closed the next
  day, above).

## Reproduce

```sh
cargo build -p openmhs-cli
cd python
G=../research/outcome-verifier/ros2-evaluation/results/gazebo_openamr
python -m openmhs.importers.openamr_gazebo $G/bullet1 $G/bullet2 $G/bullet3 $G/dart1 $G/tol1 $G/tol2 $G/tol3 --out events.jsonl
export OPENMHS_OPS_STORE=ops.sqlite
../target/debug/openmhs ops ingest events.jsonl
../target/debug/openmhs ops incidents --task-type lift
../target/debug/openmhs ops compare --task-type lift --a engine-bullet --b tol-0.01 --verified
```

The real flights (after downloading and unpacking the three archives):

```sh
python -m openmhs.importers.coresense_aerostack2 BAGS/rosbag2_real_paths_2_OK_no_failure \
    BAGS/rosbag2_real_paths_2_OK_panel_failure BAGS/rosbag2_real_paths_2_OK_double_failure --out flights.jsonl
# add --verify (needs OPENMHS_BIN) to check every succeeded goal against motion capture
../target/debug/openmhs ops ingest flights.jsonl
../target/debug/openmhs ops incidents
```
