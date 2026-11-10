# ROS 2 data sources considered

Searched on 2026-11-10. "Genuine ROS 2" below means ROS 2 messages in a rosbag2 file (MCAP or SQLite) or on a live ROS 2
graph. A CSV or hand-written JSON is not counted, and neither are this repository's own corpus records.

## Recordings

| Source | ROS 2? | Real or simulated | Size | Licence | Task outcome checkable? | Used |
|---|---|---|---|---|---|---|
| [Málaga 2009 Robotic Dataset Collection](https://zenodo.org/records/24021075), `parking_0L` | Yes: MCAP, converted in 2026 from the 2008 rawlog (`rawlog2rosbag`, `mrpt_ros_bridge`) | Real vehicle | 780 MB (smallest of six; 21.7 GB in all) | CC BY 4.0 | Only a position belief: no goal, action or status topic. RTK ground truth, consumer GPS, three RTK receivers, IMU, lasers, cameras | **Yes** |
| [turtlebot3_datasets](https://github.com/guilhermelawless/turtlebot3_datasets) | **No**: ROS 1 `.bag` | Real TurtleBot3 Waffle Pi, motion-capture ground truth on `/tf` | Not stated | GPL-3.0 | Odometry against mocap; no task. The author notes a fixed stamp synchronisation issue | No: ROS 1 |
| [Autoware sample rosbag](https://autowarefoundation.github.io/autoware-documentation/main/demos/rosbag-replay-simulation/) | Yes (SQLite) | Real vehicle | Not stated (gigabytes likely) | "© 2020 Tier IV"; no open licence found | Localisation replay; no task outcome | No: licence unclear, size |
| [IEEE DataPort rosbag2_2024_07_19](https://ieee-dataport.org/documents/rosbag220240719110137-dataset) | Yes (MCAP) | Autonomous vehicle | Not checked | Requires login | Odometry, no ground truth noted | No |
| [JKK Research dataset 02](https://jkk-research.github.io/dataset/jkk_dataset_02/) | Yes (MCAP) | Vehicle | Not checked | Not checked | Not a robot task | No |
| `rosbag2_storage_mcap_testdata` ([index.ros.org](https://index.ros.org/p/rosbag2_storage_mcap_testdata/)) | Yes | Test fixtures | Small | Apache-2.0 | No robot | No |

**Not found:** a public ROS 2 recording of a robot task (navigation goal, manipulation) that contains the goal, the action
status and an independent measurement of the outcome. Nav2 forum posts describe the status topic
(`/<action>/_action/status`, `action_msgs/GoalStatusArray`), but no published bag with it turned up. No OpenArm or
OpenAMRobot ROS 2 recording was found; the OpenArm dataset is parquet episodes, not rosbags.

## Live ROS 2

**No public live ROS 2 robot stream was found.** Foxglove Bridge and rosbridge serve live topics from a robot on the
viewer's own network, and the documentation warns against exposing rosbridge publicly without authentication. None of the
four kinds of live source (a local robot, an authorised remote bridge, a replayed bag, a simulator) is publicly available
to connect to. Nothing in this evaluation is described as live.

## Simulators already on this machine

The isolated WSL distro has ROS 2 Jazzy, `rosbag2` with MCAP, Gazebo with `ros_gz_bridge`, MoveIt, and the public
[openamr-upperbody-sw](https://github.com/openAMRobot/openamr-upperbody-sw) simulation (MIT). Not installed: Nav2,
TurtleBot3, turtlesim, Webots, Isaac Sim (no NVIDIA GPU; the GPU is Intel UHD 630).

| Option | Install | Runs here | Native status available | Natural failure known | New evidence |
|---|---|---|---|---|---|
| openamr-upperbody-sw, Gazebo + ros2_control | Installed | **Yes, used**: see RESULTS_GAZEBO.md | Yes: `follow_joint_trajectory` action status | Yes: earlier sessions saw a lift goal `SUCCEEDED` while the joint stayed at 0 (DART), and `/odom` over-reporting true base travel 5.7× (Bullet) | **High**: native status against the simulator's own physics pose |
| TurtleBot3 + Nav2 + Gazebo | Hundreds of MB of packages | When WSL can start | Yes: `navigate_to_pose` status | Not known | High, but needs approval to install |
| MoveIt 2 Panda / UR | Partly installed | When WSL can start | Yes | Not known | Medium |
| Webots | Not installed, large | Unknown | Depends | Not known | Medium |
| Isaac Sim | Not feasible: needs an NVIDIA RTX GPU | No | | | |
| Bag replay without a simulator | Done here | **Yes, on Windows** | Only what the bag recorded | Yes, in the data | Done: see RESULTS.md |

On 2026-11-10 WSL failed to start (`Wsl/Service/E_UNEXPECTED`) and logged three kernel panics while C: had 256 to 646 MB
free. That is why the bag replay ran first. Once C: had space again, WSL started and the Gazebo experiment ran.
