# ROS 2 (observation only)

`adapters/python/ros2/ros2_adapter.py` lets agents read a ROS 2 robot's state through
OpenMHS: battery, joint states, diagnostics, any field of any topic. It is an
out-of-process adapter ([Python SDK](python-sdk.md)) built on `rclpy`, for **ROS 2
Jazzy**, and runs in the pinned `ros:jazzy-ros-base` container image.

## What it does not do

- **It does not move the robot.** It publishes nothing, calls no services and sends no
  action goals. A descriptor that declares `write` or `invoke` is refused.
- **It is not in any control loop.** No OpenMHS request, and no LLM, sits between a ROS
  controller and its actuators. Real-time control stays in `ros2_control` and the
  robot's own safety controllers. The adapter claims and releases no controller
  resources and never bypasses ros2_control's resource ownership.
- Supervisory **services and actions** (start a mission, dock, switch a controller with
  `controller_manager`) are future work. They will go through OpenMHS's approval and
  lifecycle like any other state-changing command, and stay outside real-time loops.

> **A conformance level is not a safety certification.** Connect the adapter to a real
> robot's ROS graph only with the operator's approval.

## Try it against the simulator

Needs Docker. The first run pulls the image (about 300 MB):

```sh
openmhs conformance --adapter-manifest adapters/python/ros2/manifest.sim.json
```

`config.simulate` starts a second node in the adapter process that publishes
`sensor_msgs/msg/BatteryState` on `/battery_state` at 10 Hz. The adapter reaches L1,
including `link_down` (the publisher goes silent, then resumes). The container runs with
`--network none --read-only`, so the simulator's DDS traffic never leaves it.

The image is pinned by digest:
`ros@sha256:066420e07f60aa18262f2479981def87ebcfcec42eefb0c0c57c4a46098348ca` is
`ros:jazzy-ros-base` as built on 2026-10-02 (from docker-library/repo-info). The newest
build of that tag was under two weeks old when this was pinned, so it was not used.

## Configure a real robot

```json
"runtime": {
  "command": ["docker", "run", "--rm", "-i", "--network", "host", "--read-only", "--tmpfs", "/tmp",
              "-e", "ROS_DOMAIN_ID", "-e", "ROS_LOG_DIR=/tmp/ros",
              "-v", "{manifest_dir}:/openmhs/adapters/python/ros2:ro",
              "-v", "/opt/openmhs/python:/openmhs/python:ro",
              "ros@sha256:066420e07f60aa18262f2479981def87ebcfcec42eefb0c0c57c4a46098348ca",
              "python3", "-u", "/openmhs/adapters/python/ros2/ros2_adapter.py"],
  "env": { "ROS_DOMAIN_ID": "7" }
},
"config": {
  "descriptor": { "...": "the robot's descriptor" },
  "topics": {
    "/battery_state": {
      "type": "sensor_msgs/msg/BatteryState",
      "stale_after_ms": 1000,
      "fields": { "battery.voltage": "voltage", "battery.charge": "percentage" }
    },
    "/joint_states": {
      "type": "sensor_msgs/msg/JointState",
      "qos": "sensor_data",
      "stale_after_ms": 200,
      "fields": { "arm.joint1.position": "position.0" }
    }
  }
}
```

- `--network host` lets DDS reach the robot (Linux). Set `ROS_DOMAIN_ID` to the robot's.
  With DDS security (SROS 2) enabled on the robot, mount the keystore read-only and set
  `ROS_SECURITY_*` in `runtime.env`; that setup is not tested by OpenMHS yet.
- `type` is `<package>/msg/<Type>`; the package must be installed in the image
  (`ros-base` has the common message packages; build your own image for others).
- `fields` maps a capability to a dotted path into the message; numbers index arrays.
- `qos`: `sensor_data` (best effort, the default) or `system_default` (reliable).

## Readings

- A `read` returns the newest message's field. Its timestamp is the message's
  `header.stamp` when it has a non-zero one, else when the adapter received it. Stamps
  must be wall-clock time (no `use_sim_time`), and the robot's clock synchronized with the
  gateway's (NTP or PTP); the gateway checks `freshness.max_age_ms` against the stamp and
  never treats a reading stamped in the future as fresh.
- **Silence is disconnection.** A topic with no message for `stale_after_ms` reads as
  `adapter.disconnected` ("nothing on /battery_state for 1250 ms"), never as an old value
  marked `GOOD`. Detecting an outage therefore takes `stale_after_ms`; set it a few
  periods above the topic's publish rate.
- `connect` waits up to 3 s for a first message on every topic (DDS discovery takes a
  moment). Topics still silent after that show up in `health`.
- NaN (the `sensor_msgs` convention for "not measured") is `adapter.device_failure`.
- Health: healthy when every topic is fresh, degraded when some are stale, disconnected
  when all are.
- There is no subscription stream to the gateway yet (the adapter contract has none);
  the adapter's own ROS subscriptions only keep the newest message per topic.

## Containers and supervision

The host supervises the `docker` client process. When the adapter exits, `--rm` removes
the container. If the host kills an adapter that has hung (missed heartbeats), the
client's death closes the container's stdin; an adapter that is still reading exits, but
one hung outside Python's control may leave its container running. Add `--name` to the
command to find and remove it (`docker rm -f`).
