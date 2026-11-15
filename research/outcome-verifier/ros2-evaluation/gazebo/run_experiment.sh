#!/bin/bash
# One recorded run of the Gazebo outcome experiment (SIMULATION ONLY).
#
#   run_experiment.sh OUT_DIR [PHYSICS_ENGINE]
#
# Needs: ROS 2 Jazzy, Gazebo (gz sim), and the openamr-upperbody-sw workspace built and sourced through ENV_SH (a file
# that sources ROS and the workspace and sets ROS_DOMAIN_ID and GZ_PARTITION). Writes into OUT_DIR:
#   bag/            the rosbag2 MCAP recording (the evidence source)
#   gz_raw.jsonl    Gazebo's own pose messages, read straight from gz transport (the labels' source, not via ROS)
#   intent.jsonl    what the driver asked for
#   versions.txt, urdf.xml, lift_controller_params.yaml, stack.log
set -u
OUT=$(realpath -m "${1:?usage: run_experiment.sh OUT_DIR [ENGINE]}")
ENGINE=${2:-gz-physics-bullet-featherstone-plugin}
HERE=$(cd "$(dirname "$0")" && pwd)
ENV_SH=${ENV_SH:-/mnt/d/openmhs-tmp/gzeval/env.sh}
set +u; source "$ENV_SH"; set -u
mkdir -p "$OUT"
if pgrep -f "gz sim" >/dev/null || pgrep -f move_group >/dev/null; then echo "a stack is already running" >&2; exit 2; fi

echo "engine $ENGINE" | tee "$OUT/run.txt"
date -u +"wall_start %Y-%m-%dT%H:%M:%SZ" >> "$OUT/run.txt"
setsid nohup ros2 launch openamrobot_upperbody_bringup moveit.launch.py sim:=gazebo rviz:=false headless:=true \
  physics_engine:="$ENGINE" </dev/null >"$OUT/stack.log" 2>&1 &
STACK=$!

# Background jobs of a non-interactive script start with SIGINT ignored, so every stop here is SIGTERM (which ROS 2
# and rosbag2 treat as a clean shutdown), then SIGKILL if a process is still there.
stop() {
  local pid=$1 i
  kill -TERM "$pid" 2>/dev/null || return 0
  for i in $(seq 1 30); do kill -0 "$pid" 2>/dev/null || return 0; sleep 1; done
  kill -KILL "$pid" 2>/dev/null
}
cleanup() {
  [ -n "${REC:-}" ] && stop "$REC"
  [ -n "${REPUB:-}" ] && stop "$REPUB"
  [ -n "${RAW:-}" ] && stop "$RAW"
  kill -TERM -- -"$STACK" 2>/dev/null; sleep 5; kill -KILL -- -"$STACK" 2>/dev/null
  for p in "gz sim" "gz topic" move_group ros2_control_node robot_state_publisher parameter_bridge \
           joint_state_publisher gz_pose_republisher; do
    pkill -KILL -f "$p" 2>/dev/null
  done
  if pgrep -f "gz sim|move_group|ros2 bag record|gz_pose_republisher" >/dev/null; then echo "LEFTOVER processes" >&2; fi
}
trap cleanup EXIT

ready=0
for i in $(seq 1 120); do
  if ros2 control list_controllers 2>/dev/null | grep -E "^lift_controller .* active" >/dev/null \
     && ros2 topic list 2>/dev/null | grep -qx /odom; then ready=1; break; fi
  sleep 3
done
echo "ready $ready after $((i * 3)) s" | tee -a "$OUT/run.txt"
[ "$ready" = 1 ] || { echo "stack did not become ready" >&2; exit 3; }

ros2 param get /robot_state_publisher robot_description > "$OUT/urdf.xml" 2>&1
ros2 param dump /lift_controller > "$OUT/lift_controller_params.yaml" 2>&1
{
  echo "openamr-upperbody-sw $(git -C ~/openamr_main/src/openamr-upperbody-sw rev-parse HEAD 2>/dev/null)"
  dpkg-query -W -f '${Package} ${Version}\n' gz-harmonic libgz-sim8 ros-jazzy-gz-ros2-control \
    ros-jazzy-joint-trajectory-controller ros-jazzy-rosbag2 ros-jazzy-rosbag2-storage-mcap ros-jazzy-moveit-ros-move-group 2>/dev/null
  uname -r
} > "$OUT/versions.txt"

gz topic -e -t /world/empty/dynamic_pose/info --json-output > "$OUT/gz_raw.jsonl" 2>/dev/null &
RAW=$!
python3 "$HERE/gz_pose_republisher.py" --world empty --model openamrobot > "$OUT/republisher.log" 2>&1 &
REPUB=$!
ros2 bag record -s mcap --include-hidden-topics -o "$OUT/bag" \
  /lift_controller/follow_joint_trajectory/_action/status /lift_controller/controller_state \
  /arm/joint_states /joint_states /odom /cmd_vel /clock /gz/dynamic_pose /tf_static > "$OUT/record.log" 2>&1 &
REC=$!
sleep 5

# DRIVER_ARGS: optional task-client settings, e.g. "--goal-tolerance 0.01 --release tol-0.01"
timeout 1200 python3 "$HERE/task_driver.py" --intent "$OUT/intent.jsonl" ${DRIVER_ARGS:-} > "$OUT/driver.log" 2>&1
echo "driver exit $?" | tee -a "$OUT/run.txt"
sleep 2
date -u +"wall_end %Y-%m-%dT%H:%M:%SZ" >> "$OUT/run.txt"
