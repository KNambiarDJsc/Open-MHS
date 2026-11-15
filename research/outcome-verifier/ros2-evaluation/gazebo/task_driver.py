"""Drive the simulated robot through a fixed task list and log what was asked for (intent), nothing else.

SIMULATION ONLY. This is the experiment's operator, standing in for a user or a task planner: it sends lift goals to
`/lift_controller/follow_joint_trajectory` and velocity commands to `/cmd_vel` in Gazebo. OpenMHS does not do this;
OpenMHS only reads the recording afterwards. Never point this at a robot.

The task list is fixed in TASKS below and in EVALUATION_GAZEBO.md, written before the measured runs. Lift goals go
straight to the trajectory controller, as a script or a planner that skips MoveIt's time parameterisation would send
them; some are faster than the lift's URDF velocity limit (0.018 m/s) on purpose, and some are not.

Writes one JSON line per task to --intent, with simulation times from the node's clock (use_sim_time).
"""

from __future__ import annotations

import argparse
import json
import threading
import time

import rclpy
from builtin_interfaces.msg import Duration
from control_msgs.action import FollowJointTrajectory
from control_msgs.msg import JointTolerance
from geometry_msgs.msg import Twist
from rclpy.action import ActionClient
from rclpy.executors import MultiThreadedExecutor
from rclpy.node import Node
from rclpy.parameter import Parameter
from trajectory_msgs.msg import JointTrajectoryPoint

SETTLE_S = 6.0
# (task id, target lift_joint position in m, trajectory duration in s)
LIFT = [("L01", 0.02, 2.0), ("L02", 0.05, 3.0), ("L03", 0.10, 2.0), ("L04", 0.20, 4.0), ("L05", 0.25, 4.0),
        ("L06", 0.34, 2.0), ("L07", 0.10, 8.0), ("L08", 0.05, 4.0), ("L09", 0.0, 1.0), ("L10", 0.0, 2.0)]
# (task id, linear x in m/s, angular z in rad/s, duration in s)
BASE = [("B01", 0.2, 0.0, 3.0), ("B02", 0.0, 0.5, 2.0), ("B03", 0.2, 0.0, 3.0), ("B04", -0.1, 0.0, 2.0)]


def uuid_text(raw) -> str:
    h = bytes(raw).hex()
    return f"{h[:8]}-{h[8:12]}-{h[12:16]}-{h[16:20]}-{h[20:]}"


class Driver(Node):
    goal_tolerance: float | None = None
    goal_time_tolerance: float | None = None

    def __init__(self) -> None:
        super().__init__("outcome_task_driver", parameter_overrides=[Parameter("use_sim_time", value=True)])
        self.lift = ActionClient(self, FollowJointTrajectory, "/lift_controller/follow_joint_trajectory")
        self.cmd = self.create_publisher(Twist, "/cmd_vel", 10)

    def sim_now(self) -> float:
        return self.get_clock().now().nanoseconds / 1e9

    def wait_sim(self, seconds: float) -> None:
        t0 = self.sim_now()
        while self.sim_now() - t0 < seconds:
            time.sleep(0.01)

    def wait_future(self, fut, timeout_wall: float):
        t0 = time.monotonic()
        while not fut.done():
            if time.monotonic() - t0 > timeout_wall:
                return None
            time.sleep(0.01)
        return fut.result()

    def lift_goal(self, tid: str, target: float, duration: float) -> dict:
        goal = FollowJointTrajectory.Goal()
        goal.trajectory.joint_names = ["lift_joint"]
        p = JointTrajectoryPoint()
        p.positions = [target]
        p.time_from_start = Duration(sec=int(duration), nanosec=int(round((duration % 1) * 1e9)))
        goal.trajectory.points = [p]
        # Optional per-goal tolerances (a different "release" of the task client). Without them the controller's own
        # constraints apply, which for this stack are zero: no position check at all.
        if self.goal_tolerance is not None:
            goal.goal_tolerance = [JointTolerance(name="lift_joint", position=self.goal_tolerance)]
        if self.goal_time_tolerance is not None:
            t = self.goal_time_tolerance
            goal.goal_time_tolerance = Duration(sec=int(t), nanosec=int(round((t % 1) * 1e9)))
        rec = {"task": tid, "kind": "lift", "target_m": target, "duration_s": duration, "sent_sim": self.sim_now()}
        handle = self.wait_future(self.lift.send_goal_async(goal), 30.0)
        if handle is None or not handle.accepted:
            rec.update(accepted=False)
            return rec
        rec.update(accepted=True, goal_id=uuid_text(handle.goal_id.uuid), accepted_sim=self.sim_now())
        res = self.wait_future(handle.get_result_async(), 300.0)
        if res is None:
            rec.update(result_status=None)
            return rec
        rec.update(result_status=int(res.status), result_error_code=int(res.result.error_code),
                   result_error_string=res.result.error_string, result_sim=self.sim_now())
        return rec

    def drive(self, tid: str, vx: float, wz: float, duration: float) -> dict:
        rec = {"task": tid, "kind": "base", "vx": vx, "wz": wz, "duration_s": duration, "start_sim": self.sim_now()}
        t = Twist()
        t.linear.x, t.angular.z = vx, wz
        t0 = self.sim_now()
        while self.sim_now() - t0 < duration:
            self.cmd.publish(t)
            time.sleep(0.05)
        rec["stop_sim"] = self.sim_now()
        zero = Twist()
        for _ in range(5):
            self.cmd.publish(zero)
            time.sleep(0.02)
        return rec


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--intent", required=True)
    ap.add_argument("--goal-tolerance", type=float, default=None, help="per-goal lift_joint position tolerance, m")
    ap.add_argument("--goal-time-tolerance", type=float, default=None, help="per-goal goal_time_tolerance, s")
    ap.add_argument("--release", default=None, help="label for this task-client configuration")
    args, ros_args = ap.parse_known_args()
    import os

    with open(os.path.join(os.path.dirname(os.path.abspath(args.intent)), "driver_config.json"), "w",
              encoding="utf-8") as f:
        json.dump({"goal_tolerance_m": args.goal_tolerance, "goal_time_tolerance_s": args.goal_time_tolerance,
                   "release": args.release}, f, sort_keys=True)
    rclpy.init(args=ros_args)
    node = Driver()
    node.goal_tolerance = args.goal_tolerance
    node.goal_time_tolerance = args.goal_time_tolerance
    ex = MultiThreadedExecutor()
    ex.add_node(node)
    threading.Thread(target=ex.spin, daemon=True).start()
    with open(args.intent, "w", encoding="utf-8") as out:
        def log(rec: dict) -> None:
            out.write(json.dumps(rec, sort_keys=True) + "\n")
            out.flush()
            print(json.dumps(rec, sort_keys=True), flush=True)

        if not node.lift.wait_for_server(timeout_sec=60.0):
            raise SystemExit("lift action server not available")
        while node.sim_now() == 0.0:
            time.sleep(0.1)
        log({"task": "start", "sim": node.sim_now()})
        node.wait_sim(2.0)
        for tid, target, duration in LIFT:
            log(node.lift_goal(tid, target, duration))
            node.wait_sim(SETTLE_S)
        for tid, vx, wz, duration in BASE:
            log(node.drive(tid, vx, wz, duration))
            node.wait_sim(3.0)
        log({"task": "end", "sim": node.sim_now()})
    ex.shutdown()
    node.destroy_node()
    rclpy.shutdown()


if __name__ == "__main__":
    main()
