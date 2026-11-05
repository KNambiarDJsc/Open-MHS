"""Look at the ROS 2 graph from outside OpenMHS. Runs in the ROS 2 environment; prints one JSON line.

    python3 ros_probe.py graph openmhs_observer      what a node publishes, serves and calls
    python3 ros_probe.py poke                        reach each control surface directly

``graph`` is the check that the observer has no way to command anything, from the point of view of the
ROS graph and not of the adapter's own source. ``poke`` is the positive control: it is **not** part of
the agent's path. It shows the robot's tripwires work, so that a count of zero means something.
"""

from __future__ import annotations

import json
import sys
import time

import rclpy
from rclpy.action import ActionClient
from rclpy.action.graph import get_action_client_names_and_types_by_node, get_action_server_names_and_types_by_node
from rclpy.node import Node


def graph(node: Node, target: str, wait_s: float = 15.0) -> dict:
    deadline = time.monotonic() + wait_s
    while time.monotonic() < deadline:
        found = [n for n, _ns in node.get_node_names_and_namespaces() if n == target]
        if found:
            break
        rclpy.spin_once(node, timeout_sec=0.2)
    else:
        return {"found": False, "node": target}
    # Discovery is incremental: wait a moment for the node's endpoints to settle.
    end = time.monotonic() + 3.0
    while time.monotonic() < end:
        rclpy.spin_once(node, timeout_sec=0.2)
    names = lambda pairs: sorted(name for name, _types in pairs)  # noqa: E731
    return {
        "found": True,
        "node": target,
        "publishers": names(node.get_publisher_names_and_types_by_node(target, "/")),
        "subscriptions": names(node.get_subscriber_names_and_types_by_node(target, "/")),
        "services": names(node.get_service_names_and_types_by_node(target, "/")),
        "service_clients": names(node.get_client_names_and_types_by_node(target, "/")),
        "action_servers": names(get_action_server_names_and_types_by_node(node, target, "/")),
        "action_clients": names(get_action_client_names_and_types_by_node(node, target, "/")),
    }


def poke(node: Node) -> dict:
    from geometry_msgs.msg import Twist
    from rcl_interfaces.msg import Parameter, ParameterType, ParameterValue
    from rcl_interfaces.srv import SetParameters
    from std_srvs.srv import SetBool
    from tf2_msgs.action import LookupTransform
    from trajectory_msgs.msg import JointTrajectory

    done: dict[str, bool] = {}
    for topic, kind in (("/cmd_vel", Twist), ("/joint_trajectory", JointTrajectory)):
        pub = node.create_publisher(kind, topic, 10)
        deadline = time.monotonic() + 10
        while pub.get_subscription_count() == 0 and time.monotonic() < deadline:
            rclpy.spin_once(node, timeout_sec=0.1)
        pub.publish(kind())
        rclpy.spin_once(node, timeout_sec=0.3)
        done[topic] = pub.get_subscription_count() > 0
    client = node.create_client(SetBool, "/robot/set_mode")
    done["/robot/set_mode"] = client.wait_for_service(timeout_sec=10)
    if done["/robot/set_mode"]:
        rclpy.spin_until_future_complete(node, client.call_async(SetBool.Request()), timeout_sec=5)
    action = ActionClient(node, LookupTransform, "/robot/run_task")
    done["/robot/run_task"] = action.wait_for_server(timeout_sec=10)
    if done["/robot/run_task"]:
        rclpy.spin_until_future_complete(node, action.send_goal_async(LookupTransform.Goal()), timeout_sec=5)
    params = node.create_client(SetParameters, "/sim_robot/set_parameters")
    done["parameter:max_speed"] = params.wait_for_service(timeout_sec=10)
    if done["parameter:max_speed"]:
        request = SetParameters.Request()
        request.parameters = [Parameter(name="max_speed", value=ParameterValue(
            type=ParameterType.PARAMETER_DOUBLE, double_value=9.0))]
        rclpy.spin_until_future_complete(node, params.call_async(request), timeout_sec=5)
    return {"reached": done}


def main() -> None:
    rclpy.init()
    node = Node("example_probe", enable_rosout=False)
    if sys.argv[1] == "graph":
        result = graph(node, sys.argv[2])
    elif sys.argv[1] == "poke":
        result = poke(node)
    else:
        raise SystemExit(f"unknown probe {sys.argv[1]}")
    print(json.dumps(result), flush=True)
    node.destroy_node()
    rclpy.shutdown()


if __name__ == "__main__":
    main()
