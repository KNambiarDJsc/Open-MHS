# J. First vertical slice, and the three demonstrations

## J. The slice: task → execution event → deployment version → failure → incident timeline → artifact reference

### Why this slice

It needs no robot-side process, no server, no new infrastructure and no new dependency beyond what the workspace and
the research venv already use. The input already exists in this repository's world: the four recorded Gazebo runs
(`research/outcome-verifier/ros2-evaluation/results/gazebo_openamr/`, bags on the D: drive with SHA-256 recorded),
which contain real ROS 2 `FollowJointTrajectory` goal statuses, the task driver's intent with goal UUIDs, version
files, and Gazebo ground truth from which `outcome.verified` events already exist.

### Scope

| Part | Contents | Issues |
|---|---|---|
| `crates/openmhs-events` | Envelope, manifest, artifact types; canonical JSON; digests; schema export | 3, 4, 5 |
| `crates/openmhs-ops` | Migrations; ingest with duplicate/conflict; task assembly; correlation rules v1 (triggers: reported failure/unknown, verified `violated`, reported-vs-verified disagreement; id joins by goal/task, window joins); timeline | 6-10 |
| `openmhs-cli` | `openmhs ops ingest | rebuild | incidents | show` | 11 |
| Python | `openmhs.importers.rosbag` (moved from research) and a Gazebo run adapter: lift goals → `task.*` + `action.*` keyed by goal UUID; drive segments → `task.*`; `versions.txt` and engine → manifest; the bag → `artifact.registered`; verifier verdicts → `outcome.verified` | 12-15 |

Not in the slice: release comparison, SDK, service, collector, MCP, LeRobot.

### Acceptance (all automated)

1. Importing `bullet1` yields 10 lift tasks and 4 drive tasks, each with its manifest digest.
2. Incidents open for exactly the lift goals whose reported outcome (`succeeded`) disagrees with the verified one
   (`violated`): 5 in `bullet1`, the same goal ids as `RESULTS_GAZEBO.md`. Drive tasks have no goal id and their
   verified results are per odometry message, so they join a drive task by time window (basis `window`); a drive task
   containing a `violated` odometry claim opens an incident.
3. `openmhs ops show <incident>` prints the status transitions with their placement uncertainty (50.5 ms), the verified
   verdict with its contract id, the manifest (openamr revision, `gz_ros2_control` and `joint_trajectory_controller`
   versions, physics engine, `lift_controller` goal tolerance 0.0 from the recorded parameters), and an artifact link
   with the incident's time window and topics.
4. Every timeline member states its basis (`id_match` or `window`).
5. Importing the same files twice changes nothing; importing them in shuffled order gives byte-identical derived
   tables (property test).
6. Importing `dart1` gives a different manifest digest (engine differs) and 8 lift incidents.
7. CI runs the slice on a trimmed MCAP fixture under 5 MB.

### Estimate

About 2,000-2,500 lines of Rust and 600 of Python including tests; two to three weeks for one engineer. The research
bridge already covers the hardest decoding problems.

## Demo 1: ROS 2 failure investigation

**Workload:** the OpenAMRobot upper body in Gazebo (public, MIT), already scripted in
`research/outcome-verifier/ros2-evaluation/gazebo/`. The natural failure: the lift controller has no goal tolerance,
so `FollowJointTrajectory` reports `SUCCEEDED` while the lift is short; under DART the lift never moves.

**Shows:** task identity (driver intent), ROS goal and result (status transitions per goal UUID), software release
(manifest), fault timeline (status, verified violation, timing), artifact links (bag window and topics), classification
(`manipulation.lift`, trigger `reported_verified_disagree`).

**Runnable:** `research/.../gazebo/run_experiment.sh` (WSL, ROS 2 Jazzy) → import → `openmhs ops show`. **Testable:**
the slice's acceptance list on the committed fixture.

A Nav2 localization-failure variant (the brief's Scenario A) needs Nav2 and TurtleBot3 packages installed in the WSL
distro (hundreds of MB), which needs approval.

## Demo 2: Physical AI policy recovery

**Workload:** LeRobot in simulation, a pretrained public policy (e.g. a diffusion policy for the PushT environment),
no training. A scripted "operator" takes over when the policy has made no progress for N seconds and finishes the task
with a scripted controller; the takeover frames are recorded in the same LeRobot episode.

**Shows:** policy attempt (episode, model revision from the Hugging Face commit), failure (no progress, task not done),
`intervention.started`/`.ended` with frame indices, recovery (`outcome.annotated recovered`), episode export (a
selection manifest of episodes and frame ranges tagged as intervention, loadable with LeRobot's tools), model version
association (manifest `model` component).

**Needs approval:** `lerobot` with PyTorch is a multi-gigabyte install. **Testable:** a recorded fixture of the
events (not the video) checked in; the selection manifest validated against the dataset revision.

**Honest limit:** a scripted operator is not a human; this demo shows the data path, not intervention behaviour.

## Demo 3: Release regression

Two parts, because each alone is weak:

1. **Synthetic fleet with known truth.** A generator (`openmhs ops synth`) emits task attempts for two releases at
   three sites with declared failure probabilities, a staggered rollout (release B reaches site A first) and some
   unknown outcomes. Shows outcome counts, the denominators, Wilson and Newcombe intervals, stratification revealing
   the site confound, `insufficient_evidence` at small n, and representative incidents. Testable: the declared rates
   fall inside the reported intervals at the advertised coverage over 1,000 seeds.
2. **A real configuration A/B in Gazebo.** Release A: `lift_controller` goal tolerance 0.0 (as shipped). Release B:
   goal tolerance 0.01 m. Three runs each with the same task list. Expected, to be measured: B's action status reports
   `ABORTED` (or reaches the target) instead of a false `SUCCEEDED`, so reported success rate *falls* while verified
   success is unchanged or rises. The demo's point: a release dashboard built on reported status would call B a
   regression. Open-MHS compares both outcome kinds (ADR-0033).

## What the demos cannot show, and the physical path

| Simulation shows | Simulation cannot show |
|---|---|
| The data path, idempotence, ordering, statistics on known truth | Real failure distributions, real operator behaviour, real clock skew between robot computers, real network partitions, real data volumes, real deployment practices |

Physical path, in order: (1) import one design partner's real recording with their deployment metadata; (2) run the
SDK on a partner robot in shadow mode (spool only, nothing uploaded automatically) for two weeks; (3) measure the
collector budgets on a Raspberry Pi 5 / Jetson bench before any claim about embedded use. No claim of production
readiness is made from Demos 1-3.
