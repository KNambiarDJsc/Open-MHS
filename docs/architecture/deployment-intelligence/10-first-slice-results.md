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
  12.8 s instead of 8.0. The comparison now reports task duration per arm: median 3.0 s for A and 4.0 s for B, 90th
  percentile 4.1 s and 5.1 s. Release B trades time for correctness.
- B's errors at the reported success were 9.2 to 9.9 mm, just inside the verifier's 10 mm contract, because the
  controller declares success as soon as the lift is within its own 10 mm tolerance, while still moving. A 5 mm
  contract would fail most of them.

## Limits, stated plainly

- **Replicates are not trials.** The importer tags each goal with a scenario id (`lift/L03` in every run), and the
  tool counts scenarios when attempts repeat them (ADR-0032). Without scenario ids it cannot tell, and says nothing.
- Simulation time restarts at zero in each run, so each run is its own simulated robot and clock domain; times print
  as 1970 dates.
- The "verified" outcome comes from Gazebo's own poses: independent of the controllers, not of the simulator.
- Demo 2 (LeRobot policy recovery and checkpoint comparison) is not built: the approved install has not completed over
  a connection measured at about 40 KB/s.

## Follow-up issues

Done after the first review round (this branch): scenario ids with scenario-level counting (26), duration in
comparisons (27), concurrent events marked on timelines (28).

| # | Title | Why |
|---|---|---|
| 29 | Demo 2 on LeRobot once installed; Isaac Lab-Arena results import | The chosen users run learned policies |
| 30 | Pair attempts across arms by scenario (a paired comparison) | Matched scenarios give much tighter intervals than two independent samples |

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
