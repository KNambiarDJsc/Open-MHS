"""Republish Gazebo's own link poses into ROS 2 so they are recorded in the same bag as the robot's topics.

Why this exists: `ros_gz_bridge` converts `gz.msgs.Pose_V` to `tf2_msgs/TFMessage`, but for the world's
`dynamic_pose/info` topic it leaves every `child_frame_id` empty and every stamp zero (checked 2026-11-10, Jazzy), so
the poses cannot be told apart or placed in time. This node reads the same Gazebo topic through Gazebo's own command
line tool (`gz topic -e --json-output`, no extra packages) and publishes each message as one `tf2_msgs/TFMessage`:

- `header.stamp` is the Gazebo message's own simulation time, for every transform;
- the model's entry (`openamrobot`) is its pose in the world: `frame_id` `gz:world`;
- every link entry is relative to the model, as Gazebo publishes it: `frame_id` `gz:<model>`;
- `child_frame_id` is Gazebo's name for the entity. Missing position or orientation fields are zero in Gazebo's JSON
  (protobuf omits zero values), so they are filled with zero, which is what they are.

It publishes only on /gz/dynamic_pose. It never commands the robot or the simulator. It is part of the experiment's
recording harness, not of OpenMHS.

    python3 gz_pose_republisher.py --world empty --model openamrobot
"""

from __future__ import annotations

import argparse
import json
import subprocess
import threading

import rclpy
from geometry_msgs.msg import TransformStamped
from rclpy.node import Node
from tf2_msgs.msg import TFMessage


def to_msg(d: dict, model: str) -> TFMessage | None:
    stamp = d.get("header", {}).get("stamp", {})
    sec, nsec = int(stamp.get("sec", 0)), int(stamp.get("nsec", 0))
    if sec == 0 and nsec == 0:
        return None
    out = TFMessage()
    for p in d.get("pose", []):
        t = TransformStamped()
        t.header.stamp.sec, t.header.stamp.nanosec = sec, nsec
        t.header.frame_id = "gz:world" if p.get("name") == model else f"gz:{model}"
        t.child_frame_id = p.get("name", "")
        pos, ori = p.get("position", {}), p.get("orientation", {})
        t.transform.translation.x = float(pos.get("x", 0.0))
        t.transform.translation.y = float(pos.get("y", 0.0))
        t.transform.translation.z = float(pos.get("z", 0.0))
        t.transform.rotation.x = float(ori.get("x", 0.0))
        t.transform.rotation.y = float(ori.get("y", 0.0))
        t.transform.rotation.z = float(ori.get("z", 0.0))
        t.transform.rotation.w = float(ori.get("w", 1.0)) if ori else 1.0
        out.transforms.append(t)
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--world", default="empty")
    ap.add_argument("--model", default="openamrobot")
    args, ros_args = ap.parse_known_args()
    rclpy.init(args=ros_args)
    node = Node("gz_pose_republisher")
    pub = node.create_publisher(TFMessage, "/gz/dynamic_pose", 100)
    proc = subprocess.Popen(["gz", "topic", "-e", "-t", f"/world/{args.world}/dynamic_pose/info", "--json-output"],
                            stdout=subprocess.PIPE, text=True, bufsize=1)
    counts = {"published": 0, "unstamped": 0, "unparsed": 0}

    def pump() -> None:
        assert proc.stdout is not None
        for line in proc.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                msg = to_msg(json.loads(line), args.model)
            except (ValueError, TypeError):
                counts["unparsed"] += 1
                continue
            if msg is None:
                counts["unstamped"] += 1
                continue
            pub.publish(msg)
            counts["published"] += 1

    threading.Thread(target=pump, daemon=True).start()
    try:
        rclpy.spin(node)
    except (KeyboardInterrupt, rclpy.executors.ExternalShutdownException):
        pass
    finally:
        proc.terminate()
        node.get_logger().info(f"gz_pose_republisher counts: {counts}")


if __name__ == "__main__":
    main()
