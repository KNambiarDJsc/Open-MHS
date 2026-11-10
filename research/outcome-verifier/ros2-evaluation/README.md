# Outcome verification on genuine ROS 2 data

Can OpenMHS tell whether a robot actually got where it believes it got, from recorded ROS 2 evidence, when its own belief
is wrong or the evidence is incomplete? And does that tell you more than a short script?

**Short answer from this data:** the robot's own position belief was wrong 222 times in 232, and the verifier caught every
one, as did a reasonable script. Under injected faults that change the numbers, the script was wrong 18 times in 80 and the
verifier refused every time with a reason. On clean data the verifier added nothing. No ROS 2 action status was tested yet.
See [RESULTS.md](RESULTS.md).

**Real hardware data, recorded in 2008, replayed offline. Not live, not a simulator, and not a task with an action result.**

## What is here

| | |
|---|---|
| [DATASETS.md](DATASETS.md) | Every ROS 2 source considered, with sizes, licences and why it was or was not used |
| [EVALUATION.md](EVALUATION.md) | The plan, committed before the run |
| [RESULTS.md](RESULTS.md) | What happened, and what it does and does not show |
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

## The bridge, in one paragraph

It reads the bag file and writes evidence. It never subscribes, publishes or sends anything. Each record keeps the
message's own `header.stamp` as its source time and the recorder's receipt time separately. A message with no stamp gets
no source time unless the mapping asks for receipt time, which is then labelled as a different clock. A bag that carries
`/clock` is refused unless its clock is declared as simulation time. Frames are kept from `header.frame_id`. NaN values and
missing fields are skipped and counted, never filled. Provenance (`device`, `simulator`, or unknown), sensor independence,
and the clock assumption come only from the mapping file, where a reader can see and dispute them.

## Demo storyboard (about 3 minutes)

1. **The data** (20 s). The Zenodo page and licence, the MD5 check, `ros2_evidence.py inspect`: 17 topics, 255 s, a real
   vehicle in 2008. Say plainly: recorded, converted to ROS 2 in 2026, replayed now.
2. **The surprise in the data** (30 s). The consumer GPS 260 m away from the RTK at the start; the free-text comment where
   the dataset explains its receiver offsets; `NavSatFix` saying nothing about it.
3. **One claim** (40 s). `results/.../violated_cp-18.*`: the robot believes it is at P; the contract; `openmhs outcome verify`
   says `VIOLATED`, with the ground truth it used and its stamps. Then `satisfied_cp-8`.
4. **The gap** (20 s). `unknown_rtk_gap_cp-145`: the RTK dropped out for 17 s. The robot's belief says "arrived"; the
   verifier says `UNKNOWN`, `evidence.missing`.
5. **A fault the native status cannot reveal** (40 s, labelled INJECTED on screen). `injected_wrong_frame_offset_cp-8`: the
   reference in `odom`, not `map`. A script that ignores frames reads the numbers and gets it wrong; the verifier says
   `UNKNOWN`, `evidence.frame`.
6. **What it does not show** (30 s). The results table, including where the script did as well, the metadata faults where
   the verifier only cost an answer, and that no action status was tested.

## Limits

No ROS 2 action, goal or result; no manipulation; no `/clock`; nothing live; no OpenMHS gateway in the loop. All records are
assumed to come from one logging clock, which the dataset does not confirm. The ground truth and the RTK fix share
receivers. Example evidence files in `results/` are derived from the dataset and carry its CC BY 4.0 licence.

## Next

The experiment this data cannot do: a ROS 2 action that reports `SUCCEEDED` when the physical outcome did not happen, checked
against the simulator's own physics pose. The openamr-upperbody-sw Gazebo stack on this machine has shown that natural
failure before. It needs WSL, which needs free space on C:.
