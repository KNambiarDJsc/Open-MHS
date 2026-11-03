"""Layer 1 action server: a real rclpy ActionServer with an independent effect ledger.

Uses example_interfaces/action/Fibonacci purely as a carrier: `order` is the logical operation
number. The "effect" is one fsync'd line appended to a ledger file at the effect time. The ledger is
the ground truth; clients never read it.

result_timeout is set explicitly. rcl_action's default is 10 s (used by rclcpp servers); rclpy's own
ActionServer default is 900 s. Pass --result-timeout 10 to match the C++ default.
"""

from __future__ import annotations

import argparse
import json
import os
import time

import rclpy
from example_interfaces.action import Fibonacci
from rclpy.action import ActionServer, GoalResponse
from rclpy.callback_groups import ReentrantCallbackGroup
from rclpy.executors import MultiThreadedExecutor
from rclpy.node import Node


def append(path: str, record: dict) -> None:
    with open(path, "a", encoding="utf-8") as fh:
        fh.write(json.dumps(record, sort_keys=True) + "\n")
        fh.flush()
        os.fsync(fh.fileno())


class DoseServer(Node):
    def __init__(self, args: argparse.Namespace) -> None:
        super().__init__("dose_server")
        self.args = args
        self.server = ActionServer(
            self, Fibonacci, args.action_name, self.execute,
            goal_callback=self.on_goal,
            callback_group=ReentrantCallbackGroup(),
            result_timeout=args.result_timeout,
        )

    def on_goal(self, request) -> GoalResponse:
        append(self.args.events, {"event": "goal_request", "order": request.order, "t": time.time()})
        return GoalResponse.ACCEPT

    def execute(self, goal_handle):
        order = goal_handle.request.order
        uuid = bytes(goal_handle.goal_id.uuid).hex()
        append(self.args.events, {"event": "executing", "order": order, "uuid": uuid, "t": time.time()})
        d = self.args.duration
        time.sleep(self.args.effect_frac * d)
        if not self.args.lie:
            append(self.args.ledger, {"order": order, "uuid": uuid, "t": time.time()})
        time.sleep((1.0 - self.args.effect_frac) * d)
        goal_handle.succeed()
        append(self.args.events, {"event": "succeeded", "order": order, "uuid": uuid, "t": time.time()})
        result = Fibonacci.Result()
        result.sequence = [order]
        return result


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--ledger", required=True)
    ap.add_argument("--events", required=True)
    ap.add_argument("--action-name", default="dose")
    ap.add_argument("--duration", type=float, default=3.0)
    ap.add_argument("--effect-frac", type=float, default=0.5)
    ap.add_argument("--result-timeout", type=int, default=10)
    ap.add_argument("--lie", action="store_true", help="report success but apply no effect")
    args = ap.parse_args()
    rclpy.init()
    node = DoseServer(args)
    ex = MultiThreadedExecutor(num_threads=4)
    ex.add_node(node)
    append(args.events, {"event": "server_up", "result_timeout": args.result_timeout, "t": time.time()})
    try:
        ex.spin()
    except KeyboardInterrupt:
        pass
    finally:
        ex.shutdown()
        node.destroy_node()
        if rclpy.ok():
            rclpy.shutdown()


if __name__ == "__main__":
    main()
