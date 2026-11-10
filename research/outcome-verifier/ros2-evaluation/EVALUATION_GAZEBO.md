# Evaluation plan: native ROS 2 action status against physics, in Gazebo

Written and committed before the measured runs. One development run (`dev1`) was made first, only to check that the
pipeline records and converts; its numbers are not results and were not used to choose anything below. Changes after
the measured runs go under Deviations in `RESULTS_GAZEBO.md`.

**SIMULATION ONLY.** Nothing here is evidence about a physical robot.

## Question

When a ROS 2 action reports `SUCCEEDED`, or odometry reports a position, did the physical outcome happen? Can OpenMHS
tell, from a ROS 2 recording, and does it tell more than a reasonable script would?

The Málaga experiment had no action status. This one does: `control_msgs/action/FollowJointTrajectory` on the
OpenAMRobot upper body's lift (`/lift_controller/follow_joint_trajectory`), plus the base's `/odom`.

## Why this setup

The public openamr-upperbody-sw simulation (MIT) is installed on this machine. Two facts were known before this plan:

- `lift_controller` is a `joint_trajectory_controller` with `constraints.lift_joint.goal: 0.0` and
  `constraints.goal_time: 0.0` (read from the running controller on 2026-11-10). With no goal tolerance the controller
  does not check where the joint ended: it reports `SUCCEEDED` when the trajectory's time has run out.
- The URDF limits `lift_joint` to 0.018 m/s. A goal faster than that cannot be followed if the simulator enforces it.
  Whether it does is not known in advance.
- Earlier sessions saw the base's `/odom` report 5.7 times its true travel under the Bullet engine, and the lift not
  move at all under DART while its goal succeeded. These are the reasons to expect natural failures, not results.

## Setup

- Stack: `openamrobot_upperbody_bringup moveit.launch.py sim:=gazebo headless:=true`, world `empty`.
- Engine: `gz-physics-bullet-featherstone-plugin` (the engine the project's own Gazebo tests use). One attempt under
  the default DART engine, which earlier hung at start-up through this launch file; if it hangs again that is reported.
- Runs: three measured runs under Bullet (`bullet1` to `bullet3`), same task list.
- Driver (`gazebo/task_driver.py`, the experiment's operator, not OpenMHS): ten lift goals sent straight to the
  trajectory controller, each followed by 6 s of simulation time to settle, then four base drives on `/cmd_vel`, each
  followed by 3 s stopped. The list is fixed in the driver. Five of the ten lift goals ask for more than 0.018 m/s
  from the previous target (L03, L04, L06, L07, L09); the other five ask for 0.0125 m/s or less, or no motion (L10).
- Recording (`gazebo/run_experiment.sh`): `ros2 bag record -s mcap --include-hidden-topics` of the action status
  topic, `/lift_controller/controller_state`, `/arm/joint_states`, `/joint_states`, `/odom`, `/cmd_vel`, `/clock`,
  `/tf_static`, and `/gz/dynamic_pose`.
- `/gz/dynamic_pose` is Gazebo's own link pose message, republished into ROS 2 by `gazebo/gz_pose_republisher.py`,
  because `ros_gz_bridge` drops its names and stamps (checked). Stamps are Gazebo's simulation time.
- Labels come from `gz_raw.jsonl`: the same Gazebo topic, written straight from `gz topic` to a file, **not** through
  ROS 2, the republisher, the bag or the bridge.
- Bridge: `ros2_evidence.py convert` with `mappings/gazebo_openamr.json`. The status topic has no stamp; the bridge
  places it on the simulation clock from the recorded `/clock`, with a stated uncertainty.

## Claims, contracts and labels

**Lift.** Each goal whose status reaches `SUCCEEDED` is a claim: "the lift is at the target". Contract: Gazebo's
`lift_carriage_link` height in the model frame (`gz:openamrobot`) is within **±10 mm** of the target plus the URDF's
carriage offset at `lift_joint = 0`, for every sample from the moment of success to **200 ms** after it. The offset
is computed from the run's recorded URDF (`urdf.xml`), not from any observation, and recorded as calibration
`lift-carriage-offset`. Label: the first raw Gazebo sample at or after the success time, inside the ±10 mm box:
`AT_TARGET`; outside: `NOT_AT_TARGET`; none within 200 ms: `NO_REFERENCE`.

**Base.** One `/odom` message per second of simulation time over the whole run (standstill and motion) is a claim:
"the robot is where odometry says". The odometry pose is put in Gazebo's world frame with calibration `odom-origin`:
the model's Gazebo world pose and the odometry pose in the first odometry message, while the robot stands still before
any task. Contract: Gazebo's model position in `gz:world` is within **±50 mm** of that on both axes, within **±50 ms**
of the odometry stamp. Label: the nearest raw Gazebo sample within ±50 ms, inside the box or not.

## What is compared

| | Lift | Base |
|---|---|---|
| Native | The action status: `SUCCEEDED` means there | `/odom`: always "there" |
| Joint-state script | `lift_joint` on `/arm/joint_states`, the first sample at or after success within 200 ms, ±10 mm | none: odometry is the robot's state |
| Ground-truth script | The first Gazebo carriage sample at or after success within 200 ms in the converted evidence, ±10 mm; "no data" if none | The nearest Gazebo model sample within ±50 ms, ±50 mm box |
| Verifier | `openmhs outcome verify` on the contract | the same |

The ground-truth script reads the same evidence as the verifier. It is the strongest baseline that can be written in a
few lines, not a straw man.

## Predictions, before running

1. Some lift goals will report `SUCCEEDED` with the carriage more than 10 mm from the target, if the simulator enforces
   the velocity limit. How many is not known in advance. If none do, the native lift status is not misleading here,
   and that will be reported as such.
2. `/odom` will disagree with Gazebo's model pose by more than 50 mm during and after driving, if the over-report
   seen before recurs.
3. On clean data the ground-truth script and the verifier will agree except where samples inside the 200 ms lift
   window straddle the box edge (the verifier needs all of them inside; the script looks at one).
4. **No false `SATISFIED` from the verifier** against the labels. One would be a failure of this evaluation.
5. The joint-state script will agree with the labels under Bullet if the joint state follows the physics. Under DART,
   if the lift does not move, the joint state shows that too and the script catches it.

## Injected faults (on copies of real records; none of these happened in the runs)

Targets: every lift claim, and ten base claims spread over the base phase, in `bullet1`.

| Fault | Verifier expected | Ground-truth script expected |
|---|---|---|
| Self-confirming reference: the Gazebo records replaced by the robot's own state (odometry, or `lift_joint` plus the offset), with honest metadata (device and kind of the source) | `UNKNOWN` (`evidence.missing`): no record from the named independent device | Reads the values: agrees with the native status, so wrong wherever the native status is wrong |
| Reference on another simulation clock, 1.5 s late, no stated uncertainty | `UNKNOWN`, `time.clock_domain` | Picks a sample 1.5 s away: wrong where the pose changed |
| Reference removed within 1 s of the claim | `UNKNOWN` | "no data" |
| Claim carries another goal's id | `UNKNOWN` (anchor rejected, reason shown) | Ignores it |
| Frozen reference: the Gazebo samples near the claim replaced by copies of the sample 2 s earlier, with fresh stamps | **Cannot detect.** Same verdict as the frozen values imply | Same: wrong where the pose changed |
| Records reordered | As clean | As clean |

The frozen-reference fault is there because no rule in RFC 0004 can catch it: a stale value with a fresh stamp looks
valid. If the verifier "catches" it, something is wrong.

## Also measured

Byte-identical reports on a second verification of the same claims; conversion counts and time; the bridge's placement
uncertainty for status records; real-time factor.

## Failure conditions

Any false `SATISFIED` from the verifier against a label on clean data or under an injected fault other than the frozen
reference. Any non-deterministic report. Any fault whose verifier verdict differs from the table above.
