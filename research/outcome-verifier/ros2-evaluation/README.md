# Outcome verification on genuine ROS 2 data

Can OpenMHS tell whether a robot actually got where it believes it got, from recorded ROS 2 evidence, when its own belief
is wrong or the evidence is incomplete? And does that tell you more than a short script?

**Two experiments.**

1. **A real recording** (Málaga 2009, a real vehicle, replayed offline): the robot's own position belief was wrong 222
   times in 232, and the verifier caught every one, as did a reasonable script. Under injected faults that change the
   numbers, the script was wrong 18 times in 80 and the verifier refused every time with a reason. No action status.
   See [RESULTS.md](RESULTS.md).
2. **A real ROS 2 action in simulation** (OpenAMRobot in Gazebo): the lift's `FollowJointTrajectory` reported
   `SUCCEEDED` 40 times and the lift was where it said 17 times; `/odom` over-reported travel 5.4 times, and under DART
   the robot never moved at all. The verifier, a script reading Gazebo's poses, and even the robot's own joint state
   (for the lift) all caught it. The verifier added something only when the "ground truth" was secretly the robot's own
   odometry. See [RESULTS_GAZEBO.md](RESULTS_GAZEBO.md).

**Nothing here was run on physical hardware. The recording is replayed, not live; the simulation is a simulation.**

## What is here

| | |
|---|---|
| [DATASETS.md](DATASETS.md) | Every ROS 2 source considered, with sizes, licences and why it was or was not used |
| [EVALUATION.md](EVALUATION.md) | The plan, committed before the run |
| [RESULTS.md](RESULTS.md) | What happened, and what it does and does not show |
| [EVALUATION_GAZEBO.md](EVALUATION_GAZEBO.md), [RESULTS_GAZEBO.md](RESULTS_GAZEBO.md) | The Gazebo experiment's plan (committed first) and results |
| `gazebo/` | Recording harness for the simulation: `run_experiment.sh`, the task driver (simulation only), Gazebo pose republisher |
| `gazebo_outcomes.py`, `gazebo_tables.py` | The Gazebo experiment's claims, contracts, labels, baselines, faults, and tables |
| `mappings/gazebo_openamr.json` | The simulation bag's mapping, with the status-time assumption stated |
| `results/gazebo_openamr/` | Four runs: claims, faults, summaries, versions, example contracts, evidence and reports |
| `ros2_evidence.py` | The bridge: a rosbag2 MCAP to RFC 0004 evidence records. Read-only |
| `mappings/malaga2009_parking_0L.json` | What each topic becomes, plus provenance and the clock assumption, stated |
| `malaga_checkpoints.py` | The experiment: claims, contracts, labels, script baseline, injected faults |
| `gps_belief_check.py` | Checks that the GPS error is the receiver's, not the calibration's |
| `test_ros2_evidence.py` | Bridge tests, on small MCAP files written with the ROS 2 MCAP writer |
| `results/malaga2009_parking_0L/` | Every claim, every fault trial, example contracts, evidence and reports |

## Reproduce

Needs Python 3.10+, about 800 MB on disk, and the `openmhs` binary (`cargo build -p openmhs-cli`). No ROS 2 install: the
MCAP carries its own message definitions.

```sh
python -m venv .venv && .venv/bin/pip install mcap==1.6.0 mcap-ros2-support==0.6.0   # Windows: .venv\Scripts\pip

# Data: CC BY 4.0, Blanco, Moreno, González, Autonomous Robots 27(4), 2009. https://doi.org/10.5281/zenodo.24021075
curl -L -o malaga2009_parking_0L.mcap "https://zenodo.org/records/24021075/files/malaga2009_parking_0L.mcap?download=1"
curl -L -o malaga2009_parking_0L_gt_tum.txt "https://zenodo.org/records/24021075/files/malaga2009_parking_0L_gt_tum.txt?download=1"
md5sum malaga2009_parking_0L.mcap        # 308bb33977ffb12f76303158e1072b38

.venv/bin/python ros2_evidence.py inspect malaga2009_parking_0L.mcap
.venv/bin/python ros2_evidence.py convert malaga2009_parking_0L.mcap \
    --mapping mappings/malaga2009_parking_0L.json --out evidence.jsonl
OPENMHS_BIN=../../../target/debug/openmhs .venv/bin/python malaga_checkpoints.py \
    --evidence evidence.jsonl --tum malaga2009_parking_0L_gt_tum.txt --out run     # about 3 minutes
.venv/bin/python gps_belief_check.py --evidence evidence.jsonl --tum malaga2009_parking_0L_gt_tum.txt
.venv/bin/python -m unittest discover -p "test_*.py" -v
```

To replay the bag in ROS 2 itself (Humble or newer), the dataset README gives
`ros2 bag play malaga2009_parking_0L.mcap --qos-profile-overrides-path tf_static_latched.yaml`.

### The Gazebo experiment

Needs ROS 2 Jazzy, Gazebo, and the [openamr-upperbody-sw](https://github.com/openAMRobot/openamr-upperbody-sw) workspace
built; `ENV_SH` names a file that sources them and sets `ROS_DOMAIN_ID` and `GZ_PARTITION`. Each run takes about 5
minutes and writes a 60 to 70 MB bag.

```sh
ENV_SH=~/gz_env.sh gazebo/run_experiment.sh runs/bullet1 gz-physics-bullet-featherstone-plugin   # in the ROS 2 machine
.venv/bin/python ros2_evidence.py convert runs/bullet1/bag --mapping mappings/gazebo_openamr.json     --out runs/bullet1/evidence.jsonl
OPENMHS_BIN=../../../target/debug/openmhs .venv/bin/python gazebo_outcomes.py --run runs/bullet1     --evidence runs/bullet1/evidence.jsonl --out analysis/bullet1 --faults
.venv/bin/python gazebo_tables.py analysis/bullet1
```

## The bridge, in one paragraph

It reads the bag file and writes evidence. It never subscribes, publishes or sends anything. Each record keeps the
message's own `header.stamp` as its source time and the recorder's receipt time separately. A message with no stamp gets
no source time unless the mapping asks for receipt time, which is then labelled as a different clock. A bag that carries
`/clock` is refused unless its clock is declared as simulation time. Frames are kept from `header.frame_id`. NaN values and
missing fields are skipped and counted, never filled. Provenance (`device`, `simulator`, or unknown), sensor independence,
and the clock assumption come only from the mapping file, where a reader can see and dispute them. List messages
(`TFMessage`, an action's `GoalStatusArray`) are read element by element; an action status becomes one record per
transition, keyed by goal id. An unstamped message in a simulation bag can be placed on the simulation clock from the
recorded `/clock`, with an uncertainty the record carries. When a bag recorded a schema empty, the bridge supplies a
standard definition only for `action_msgs/msg/GoalStatusArray`, and says so in the conversion metadata.

## Demo storyboard (about 4 minutes)

1. **The claim** (20 s). One sentence: the robot says it did it; did it? Two sources, labelled on screen: a real vehicle
   recorded in 2008 and replayed, and a simulated OpenAMRobot.
2. **A real ROS 2 action lying, in simulation** (60 s). `lift_controller_params.yaml`: goal tolerance 0. The driver sends
   L03 (0.10 m in 2 s; the lift is limited to 0.018 m/s). The action says `SUCCEEDED`, "Goal successfully reached!".
   `results/gazebo_openamr/example_reports/bullet1_succeeded_but_short_L03.*`: the carriage is 15 mm short; `VIOLATED`,
   with the Gazebo samples it used and their stamps.
3. **Never moved** (30 s). `dart1_succeeded_never_moved_L05`: under DART, ten successes, a lift that never left the
   floor.
4. **Odometry** (30 s). The drive table: `/odom` 0.60 m, Gazebo 0.11 m. `bullet1_odometry_wrong_O0110`.
5. **Where a script is fooled** (40 s, labelled INJECTED). `bullet1_injected_self_confirming_O0111`: the "ground truth"
   topic secretly carries the robot's own odometry. The script says "there"; the verifier says `UNKNOWN`,
   `evidence.missing`, because the contract names the independent source. Say plainly: this needs honest metadata.
6. **The real data** (30 s). Málaga: the consumer GPS 260 m off, `violated_cp-18`, the 17 s RTK gap as `UNKNOWN`.
7. **What it does not show** (30 s). The script and the robot's own joint state did as well on clean data; the frozen
   reference fools everything; a goal tolerance in the controller fixes the lift natively; nothing physical yet.

## Limits

No physical robot or hardware in either experiment. Málaga: no action status, no `/clock`, one assumed logging clock,
ground truth and RTK share receivers. Gazebo: the "independent" reference is the simulator's own pose; goals went straight
to the controller, not through MoveIt; everything was verified from recordings afterwards, not live; no OpenMHS gateway
in the loop. Example evidence files from Málaga carry the dataset's CC BY 4.0 licence.

## Next

The same contracts against a physical outcome measured by something that is not the robot: a bench supply read by a
separate meter, an ESP32 output read by a second sensor, or a real mobile base against a fixed marker or motion capture.
That is the gate this work has not passed.
