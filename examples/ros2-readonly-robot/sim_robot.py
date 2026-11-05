"""A simulated robot for this example: a ROS 2 process, separate from OpenMHS.

SIMULATED. It is not a model of any real robot and has no physics: joints and base hold fixed values
so that any change a test sees comes from a fault it asked for. It runs inside a ROS 2 Jazzy
environment and is driven over stdin and stdout with one JSON object per line.

It publishes:

    /joint_states    sensor_msgs/JointState        20 Hz
    /odom            nav_msgs/Odometry             10 Hz
    /battery_state   sensor_msgs/BatteryState       5 Hz
    /diagnostics     diagnostic_msgs/DiagnosticArray 2 Hz
    /tf              tf2_msgs/TFMessage            10 Hz (no capability maps to it, on purpose)

It also hosts control surfaces. Nothing in this example should ever reach them, and each one reports
what does (``{"event": "tripwire", ...}``), so "zero commands reached ROS 2" is an observation:

    /cmd_vel                   geometry_msgs/Twist subscriber
    /joint_trajectory          trajectory_msgs/JointTrajectory subscriber
    /robot/set_mode            std_srvs/SetBool service
    /robot/run_task            action server (tf2_msgs/LookupTransform, used only as a carrier)
    max_speed                  a settable parameter on this node

Commands on stdin:

    {"cmd": "stop", "topic": "/odom"}    stop one topic           {"cmd": "start", "topic": "/odom"}
    {"cmd": "fault", "name": "nan_joint"}  (see FAULTS)            {"cmd": "clear"}

The harness simulates a crash by sending this process SIGKILL.
"""

from __future__ import annotations

import json
import math
import os
import sys
import threading
import time

import rclpy
from builtin_interfaces.msg import Time
from diagnostic_msgs.msg import DiagnosticArray, DiagnosticStatus
from geometry_msgs.msg import TransformStamped, Twist
from nav_msgs.msg import Odometry
from rcl_interfaces.msg import SetParametersResult
from rclpy.action import ActionServer
from rclpy.executors import MultiThreadedExecutor
from rclpy.node import Node
from rclpy.qos import QoSPresetProfiles
from sensor_msgs.msg import BatteryState, JointState
from std_srvs.srv import SetBool
from tf2_msgs.action import LookupTransform
from tf2_msgs.msg import TFMessage
from trajectory_msgs.msg import JointTrajectory

JOINTS = ["shoulder", "elbow", "wrist"]
POSITIONS = [0.5, -1.2, 0.3]

#: name -> what it does. Each is a fault a robot's software could plausibly produce.
FAULTS = {
    "nan_joint": "elbow position is NaN",
    "inf_odom": "base x is infinite",
    "range_joint": "elbow position is 1000 rad, outside the declared range",
    "range_battery": "battery charge is 7, outside the declared 0 to 1",
    "empty_positions": "joint names are present and the position array is empty",
    "mismatched_arrays": "three joint names, two positions",
    "missing_joint": "the elbow is not in the message at all",
    "reordered_joints": "the same joints and values in a different array order",
    "nan_battery_temperature": "battery temperature is NaN, the message's convention for unmeasured",
    "diag_error": "the driver reports level ERROR",
    "no_diag_status": "the diagnostics array is empty",
    "late_stamp": "every message stamped 5 s in the past, as if delivered late",
    "future_stamp": "every message stamped 1 h in the future",
    "zero_stamp": "every message has an unset (zero) stamp",
    "regress_small": "stamps alternate forward and 200 ms back, within the freshness limit",
    "regress_step": "the stamp steps back 60 s and stays behind",
}


def emit(**event) -> None:
    """One JSON line on stdout, the harness's only view of this process."""
    sys.stdout.write(json.dumps(event) + "\n")
    sys.stdout.flush()


def stamp_of(seconds: float) -> Time:
    t = Time()
    t.sec = int(seconds)
    t.nanosec = int((seconds - int(seconds)) * 1e9)
    return t


class SimRobot(Node):
    def __init__(self) -> None:
        super().__init__("sim_robot")
        self.stopped: set[str] = set()
        self.fault: str | None = None
        self.ticks = 0
        self.first_publish: set[str] = set()
        reliable = 10
        sensor = QoSPresetProfiles.SENSOR_DATA.value
        self.pubs = {
            "/joint_states": self.create_publisher(JointState, "/joint_states", reliable),
            "/odom": self.create_publisher(Odometry, "/odom", reliable),
            "/battery_state": self.create_publisher(BatteryState, "/battery_state", sensor),
            "/diagnostics": self.create_publisher(DiagnosticArray, "/diagnostics", reliable),
            "/tf": self.create_publisher(TFMessage, "/tf", reliable),
        }
        self.create_timer(0.05, self.publish_joints)
        self.create_timer(0.1, self.publish_odom)
        self.create_timer(0.2, self.publish_battery)
        self.create_timer(0.5, self.publish_diagnostics)
        self.create_timer(0.1, self.publish_tf)

        # Control surfaces. Nothing in the example should ever reach them.
        self.create_subscription(Twist, "/cmd_vel", lambda _m: self.tripped("/cmd_vel"), reliable)
        self.create_subscription(
            JointTrajectory, "/joint_trajectory", lambda _m: self.tripped("/joint_trajectory"), reliable)
        self.create_service(SetBool, "/robot/set_mode", self.on_set_mode)
        self.action = ActionServer(self, LookupTransform, "/robot/run_task", self.on_task)
        self.declare_parameter("max_speed", 0.5)
        self.add_on_set_parameters_callback(self.on_parameters)

    # ------------------------------------------------------------------- tripwires

    def tripped(self, surface: str) -> None:
        emit(event="tripwire", surface=surface)

    def on_set_mode(self, _request, response):
        self.tripped("/robot/set_mode")
        response.success = False
        response.message = "SIMULATED: refused"
        return response

    def on_task(self, goal_handle):
        self.tripped("/robot/run_task")
        goal_handle.abort()
        return LookupTransform.Result()

    def on_parameters(self, parameters):
        for p in parameters:
            self.tripped(f"parameter:{p.name}")
        return SetParametersResult(successful=True)

    # ------------------------------------------------------------------- publishing

    def stamp(self):
        now = time.time()
        if self.fault == "late_stamp":
            return stamp_of(now - 5.0)
        if self.fault == "future_stamp":
            return stamp_of(now + 3600.0)
        if self.fault == "zero_stamp":
            return Time()
        if self.fault == "regress_small":
            self.ticks += 1
            return stamp_of(now - (0.2 if self.ticks % 2 else 0.0))
        if self.fault == "regress_step":
            return stamp_of(now - 60.0)
        return stamp_of(now)

    def send(self, topic: str, msg) -> None:
        if topic in self.stopped:
            return
        self.pubs[topic].publish(msg)
        if topic not in self.first_publish:
            self.first_publish.add(topic)
            emit(event="first_publish", topic=topic, t=time.time())

    def publish_joints(self) -> None:
        msg = JointState()
        msg.header.stamp = self.stamp()
        names, positions = list(JOINTS), list(POSITIONS)
        if self.fault == "nan_joint":
            positions[1] = math.nan
        elif self.fault == "range_joint":
            positions[1] = 1000.0
        elif self.fault == "empty_positions":
            positions = []
        elif self.fault == "mismatched_arrays":
            positions = positions[:2]
        elif self.fault == "missing_joint":
            names, positions = [names[0], names[2]], [positions[0], positions[2]]
        elif self.fault == "reordered_joints":
            names, positions = names[::-1], positions[::-1]
        msg.name, msg.position = names, positions
        self.send("/joint_states", msg)

    def publish_odom(self) -> None:
        msg = Odometry()
        msg.header.stamp = self.stamp()
        msg.header.frame_id = "odom"
        msg.child_frame_id = "base_link"
        msg.pose.pose.position.x = math.inf if self.fault == "inf_odom" else 1.25
        msg.pose.pose.position.y = -0.5
        msg.pose.pose.orientation.z = 0.0
        msg.pose.pose.orientation.w = 1.0
        self.send("/odom", msg)

    def publish_battery(self) -> None:
        msg = BatteryState()
        msg.header.stamp = self.stamp()
        msg.voltage = 24.8
        msg.percentage = 7.0 if self.fault == "range_battery" else 0.72
        msg.temperature = math.nan if self.fault == "nan_battery_temperature" else 31.5
        msg.current = -1.8
        msg.present = True
        self.send("/battery_state", msg)

    def publish_diagnostics(self) -> None:
        msg = DiagnosticArray()
        msg.header.stamp = self.stamp()
        if self.fault != "no_diag_status":
            # A second status first, so that position 0 is not the driver.
            other = DiagnosticStatus(name="computer", level=DiagnosticStatus.OK, message="ok")
            error = self.fault == "diag_error"
            driver = DiagnosticStatus(
                name="driver",
                level=DiagnosticStatus.ERROR if error else DiagnosticStatus.OK,
                message="SIMULATED overcurrent" if error else "SIMULATED nominal")
            msg.status = [other, driver]
        self.send("/diagnostics", msg)

    def publish_tf(self) -> None:
        t = TransformStamped()
        t.header.stamp = self.stamp()
        t.header.frame_id = "odom"
        t.child_frame_id = "base_link"
        t.transform.translation.x = 1.25
        t.transform.translation.y = -0.5
        t.transform.rotation.w = 1.0
        self.send("/tf", TFMessage(transforms=[t]))

    # ------------------------------------------------------------------- commands

    def command(self, c: dict) -> None:
        name = c.get("cmd")
        if name == "stop":
            self.stopped.add(c["topic"])
        elif name == "start":
            self.stopped.discard(c["topic"])
        elif name == "fault":
            if c["name"] not in FAULTS:
                emit(event="error", message=f"unknown fault {c['name']}")
                return
            self.fault = c["name"]
        elif name == "clear":
            self.fault = None
        else:
            emit(event="error", message=f"unknown command {name}")
            return
        emit(event="ack", cmd=name, id=c.get("id"), t=time.time())


def read_commands(robot: SimRobot) -> None:
    """Commands from the harness. End of input means the harness has gone, so this process must too."""
    for line in sys.stdin:
        line = line.strip()
        if line:
            robot.command(json.loads(line))
    os._exit(0)


def main() -> None:
    rclpy.init()
    robot = SimRobot()
    threading.Thread(target=read_commands, args=(robot,), daemon=True).start()
    executor = MultiThreadedExecutor(num_threads=3)
    executor.add_node(robot)
    emit(event="ready", pid=os.getpid(), t=time.time())
    executor.spin()


if __name__ == "__main__":
    main()
