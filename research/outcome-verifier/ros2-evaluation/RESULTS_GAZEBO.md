# Results: native ROS 2 action status against physics, in Gazebo

**SIMULATION ONLY. Nothing here is evidence about a physical robot.** The plan is
[EVALUATION_GAZEBO.md](EVALUATION_GAZEBO.md), committed (`3c92a59`) before the measured runs; two pipeline fixes found
by the development run were committed (`b471ed9`) before them too. All tables: [tables.md](results/gazebo_openamr/tables.md).

## What was run

| | |
|---|---|
| Robot | OpenAMRobot upper body (mobile base, 350 mm lift, two OpenArm arms), public `openamr-upperbody-sw` (MIT). The workspace was at local commit `2c376a9`, which adds only test files and their CMake registration to upstream `2ca7392`; the launch files, URDF and controller settings are upstream's |
| Simulator | Gazebo (gz sim) with ROS 2 Jazzy, `moveit.launch.py sim:=gazebo headless:=true`, world `empty`. WSL2 on Windows 11, Intel UHD 630, no GPU physics |
| Runs | `bullet1`, `bullet2`, `bullet3` (Bullet Featherstone) and `dart1` (DART), 2026-11-10, same task list. Versions in each run's `versions.txt` |
| Native status | `/lift_controller/follow_joint_trajectory` (`control_msgs/action/FollowJointTrajectory`) status topic, and `/odom` |
| Independent evidence | Gazebo's own link poses (`/world/empty/dynamic_pose/info`), republished into the bag as `/gz/dynamic_pose` |
| Labels | The same Gazebo topic written straight to `gz_raw.jsonl`, not through ROS 2, the bag or the bridge |
| Bridge | `ros2_evidence.py` with `mappings/gazebo_openamr.json`; about 90,000 records per run in about 9 s. Bag SHA-256 in each run's `conversion.meta.json` (bags are 60 to 70 MB, kept outside the repository) |
| Verifier | `openmhs outcome verify`, branch `feat/outcome-verifier` |

The task list (`gazebo/task_driver.py`) sends ten lift goals straight to the trajectory controller, then drives the base
four times. The driver is the experiment's operator, in simulation; OpenMHS only reads the recording.

## Two facts about the stack, read before the runs

- `lift_controller` has `constraints.lift_joint.goal: 0.0` and `constraints.goal_time: 0.0`
  ([lift_controller_params.yaml](results/gazebo_openamr/bullet1/lift_controller_params.yaml)). With no goal tolerance a
  `joint_trajectory_controller` does not check where the joint ended. It reports `SUCCEEDED` ("Goal successfully
  reached!") when the trajectory's time has run out.
- The URDF limits `lift_joint` to 0.018 m/s.

## Results on clean data (526 claims over four runs)

| Run | Lift goals `SUCCEEDED` | Lift actually within 10 mm | Base claims | Odometry actually within 50 mm |
|---|---|---|---|---|
| bullet1 | 10 of 10 | **5** | 122 | 99 |
| bullet2 | 10 of 10 | **5** | 120 | 97 |
| bullet3 | 10 of 10 | **5** | 122 | 99 |
| dart1 | 10 of 10 | **2** | 122 | 98 (only while it stood still) |

**The native action status said `SUCCEEDED` 40 times; the lift was where it said 17 times.**

- **Under Bullet** exactly the five goals that asked for more than 0.018 m/s ended short or past the target, the same
  way in all three runs, by 15 to 97 mm. One example: L03 asked for 0.10 m in 2 s and stopped about 15 mm short. The
  simulator enforced the URDF velocity limit; the controller did not check the result.
- **Under DART the lift never moved.** The carriage stayed at its start height for every goal. All ten reported success;
  the two "at target" claims are the goals whose target was zero.
- **Odometry over-reported travel.** Under Bullet a 3 s forward drive at 0.2 m/s gave 0.60 m on `/odom` and 0.11 m in
  Gazebo (5.3 to 5.5 times). A reverse drive gave 0.20 m on `/odom` and 0.002 m in Gazebo. Under DART the base never
  moved; `/odom` said 0.60 m. The largest gap between `/odom` and Gazebo at a claim was 0.87 m under Bullet and 1.06 m
  under DART. Turning on the spot (B02) agreed.

| | Lift: right of 40 | Base: right of 486 | False "there" |
|---|---|---|---|
| Native status (action, `/odom`) | 17 | 393 | **116** |
| Joint-state script (`lift_joint` on `/arm/joint_states`) | **40** | n/a | 0 |
| Ground-truth script (Gazebo poses in the evidence) | 40 | 486 | 0 |
| Verifier | 40 | 486 | 0 |

- **No false `SATISFIED`** from the verifier, and it agreed with the labels on every claim.
- **The ground-truth script and the verifier agreed on all 526 claims.** On clean data the verifier added nothing a
  reasonable script with the same evidence did not.
- **The robot's own joint state caught every lift failure too**, under both engines: `lift_joint` followed the physics,
  including staying at zero under DART. So the lift's wrong status was detectable without the simulator; it was the
  action status that ignored it. The base has no such internal cross-check: odometry *is* its state.
- Reports were byte-identical when verified twice (49 claims checked). Using only records within 2 s of a claim gave the
  same verdict as the whole file (16 checked). Median verification 0.13 s per claim, including process start.
- The status topic has no timestamp. The bridge placed every status message on the simulation clock with an uncertainty
  of 50.5 ms: 0.5 ms from the 1 kHz `/clock`, plus the 50 ms receipt allowance stated in the mapping.

## Injected faults (120 trials, `bullet1`)

Every lift claim and ten base claims over the drives, six faults each. **All injected; none happened in the runs.**

| Fault | What it changes | Verifier | Script right | Script false "there" | Verifier false `SATISFIED` |
|---|---|---|---|---|---|
| Self-confirming reference: Gazebo poses replaced by the robot's own odometry or joint state, metadata honest | The source | `UNKNOWN` 20, `evidence.missing` | 11 of 20 | **9** | 0 |
| Reference on another simulation clock, 1.5 s late | The numbers | `UNKNOWN` 20, `time.clock_domain` | 15 of 20 | 0 | 0 |
| Reference removed within 1 s | Availability | `UNKNOWN` 20 | "no data" 20 | 0 | 0 |
| Claim carries another goal's id | Metadata only | `UNKNOWN` 20, anchor rejected with the reason | **20 of 20** | 0 | 0 |
| Frozen reference: values from 2 s earlier, fresh stamps | The numbers, undetectably | Follows the frozen values: right 15 of 20 | 15 of 20 | 0 | 0 |
| Records reordered | Nothing | As clean | 20 of 20 | 0 | 0 |

Every verifier verdict matched the plan's table.

- **Self-confirming evidence is where the verifier earned its place here.** When the "ground truth" signal was in fact
  the robot's own odometry, the script read it and said "there" 9 times falsely; the verifier refused, because the
  contract names the independent device and kind. **This works only because the metadata is honest.** If a mapping
  labels odometry as `gz-physics`, nothing in RFC 0004 can tell.
- **The frozen reference is undetectable, as the plan said.** A stale value with a fresh stamp passes every rule. The
  verifier gave 5 wrong verdicts there, all `VIOLATED` where the truth was "there". None was a false `SATISFIED` among
  these targets, but that is luck, not a property: a frozen value inside the box would give one.
- **Another goal's id changes only metadata.** The script was right 20 of 20; the verifier's `UNKNOWN` cost an answer,
  as in the Málaga experiment.

## Against the research question

| Part | Shown here |
|---|---|
| Native status misleading | **Yes, with a real ROS 2 action.** `FollowJointTrajectory` reported `SUCCEEDED` for 23 lift goals that did not reach the target, and `/odom` was wrong at 93 claims, in simulation |
| Native status incomplete | Not tested beyond the injected gap |
| Better than a script | **No, on clean data**: a script reading the same evidence, or even the robot's own joint state for the lift, did as well. **Yes for self-confirming evidence**, only because metadata names the source |
| Physical hardware | **Not tested.** Gazebo's physics is a model; its "ground truth" is the simulator's own number |

## What native ROS 2 already offers

- **The lift's false `SUCCEEDED` has a native fix:** set `constraints.lift_joint.goal` (for example 0.01 m) and
  `goal_time` in the controller configuration. The controller would then abort a goal that ends outside tolerance. That
  is a configuration gap in the simulated stack, not something OpenMHS is needed for.
- **Odometry against physics has no native check.** Nothing in ROS 2 compares `/odom` with an independent pose, except
  what a user builds; on a real robot that needs an external reference (motion capture, a fixed marker, a second
  sensor).

## Deviations from the plan

- **DART did not hang** at start-up as it had before through this launch file. The planned single attempt ran fully.
- **`dart1`'s URDF was not saved**: the parameter request to `robot_state_publisher` timed out. Its carriage offset was
  taken from `bullet1`'s URDF (same package, same files; the engine does not change joint origins), with
  `gazebo_outcomes.py --urdf`. Gazebo's first sample in `dart1` put the carriage at 0.8340001 m, the offset computed.
- **Real-time factor was not recorded** for the measured runs. A spot reading during development was 0.74.
- **Added after the plan, descriptive only:** the odometry-versus-Gazebo travel per drive segment in each `summary.json`,
  and the `--urdf` option.
- **The development run (`dev1`) found two pipeline faults**, fixed before the measured runs. rosbag2 recorded the action
  status topic with an empty schema, so the bridge now supplies that one standard definition and says so. The run
  script's SIGINT never reached the recorder, because background jobs of a non-interactive shell ignore it.

## Not tested

A physical robot, or any hardware. A real independent sensor (here the reference is the simulator itself). MoveIt's
own execution path (the goals went straight to the controller). Arm or gripper outcomes. Live verification while the
robot runs (everything was verified from the recording afterwards). An OpenMHS gateway in the loop. Whether the 50 ms
receipt allowance holds under heavier load.
