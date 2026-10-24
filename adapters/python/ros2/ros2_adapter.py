"""A read-only ROS 2 (Jazzy) observation adapter (docs/adapters/ros2.md).

Each readable capability is one field of one topic: the adapter subscribes to the
topics in ``config.topics`` and a ``read`` returns the newest message's field, stamped
with the message's ``header.stamp`` when it has one. A topic that has published nothing
for ``stale_after_ms`` reads as disconnected rather than returning an old value as GOOD.

This adapter only observes. It publishes nothing, calls no services and sends no action
goals, so it cannot move a robot. It does not take part in any control loop: real-time
control stays in ros2_control and the robot's own safety controllers, and nothing here
claims or releases their resources. Supervisory services and actions are future work
(docs/adapters/ros2.md).

Runs inside a ROS 2 Jazzy environment (the pinned ``ros:jazzy-ros-base`` image in the
manifests). With ``config.simulate`` it also starts a publisher node of its own, for
conformance runs.
"""

from __future__ import annotations

import asyncio
import importlib
import math
import os
import sys
import threading
import time
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
for candidate in (HERE.parents[2] / "python", Path("/openmhs/python")):
    if (candidate / "openmhs" / "adapter.py").exists():
        sys.path.insert(0, str(candidate))
        break

import rclpy  # noqa: E402
from rclpy.executors import SingleThreadedExecutor  # noqa: E402
from rclpy.node import Node  # noqa: E402
from rclpy.qos import QoSPresetProfiles  # noqa: E402

from openmhs.adapter import Adapter, AdapterError, reading, run  # noqa: E402

READ_ONLY = {"read", "subscribe"}

#: How long ``connect`` waits for the first message on every topic.
FIRST_MESSAGE_S = 3.0


class ConfigError(Exception):
    pass


def message_type(name: str):
    """``sensor_msgs/msg/BatteryState`` -> the class."""
    package, sep, rest = name.partition("/msg/")
    if not sep or not package or not rest:
        raise ConfigError(f"message type {name!r}: expected <package>/msg/<Type>")
    try:
        return getattr(importlib.import_module(f"{package}.msg"), rest)
    except (ImportError, AttributeError) as e:
        raise ConfigError(f"message type {name!r} is not installed: {e}") from e


def field(msg, path: str):
    value = msg
    for part in path.split("."):
        value = value[int(part)] if part.isdigit() else getattr(value, part)
    return value


def stamp(msg) -> str | None:
    header = getattr(msg, "header", None)
    if header is None:
        return None
    s = header.stamp
    if s.sec == 0 and s.nanosec == 0:
        return None
    dt = datetime.fromtimestamp(s.sec + s.nanosec / 1e9, tz=timezone.utc)
    return dt.isoformat(timespec="milliseconds").replace("+00:00", "Z")


def convert(value, spec: dict, where: str):
    def bad(why):
        return AdapterError("malformed_response", f"{where}: {why} (got {value!r})")

    kind = spec.get("type")
    if kind == "number":
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise bad("expected a number")
        if not math.isfinite(value):
            # sensor_msgs uses NaN for "not measured".
            raise AdapterError("device_failure", f"{where}: the robot reports no value (NaN)")
        return {"value": float(value), "unit": spec["unit"]} if "unit" in spec else float(value)
    if kind == "integer":
        if isinstance(value, bool) or not isinstance(value, int):
            raise bad("expected an integer")
        return value
    if kind == "boolean":
        if not isinstance(value, bool):
            raise bad("expected a bool")
        return value
    if kind in ("string", "enum"):
        if not isinstance(value, str):
            raise bad("expected a string")
        if kind == "enum" and value not in spec.get("values", []):
            raise bad(f"expected one of {spec.get('values')}")
        return value
    raise bad(f"value type {kind!r} is not supported")


class Settings:
    def __init__(self, config: dict):
        unknown = set(config) - {"descriptor", "topics", "simulate"}
        if unknown:
            raise ConfigError(f"unknown config keys {sorted(unknown)}")
        self.descriptor = config.get("descriptor")
        if not isinstance(self.descriptor, dict):
            raise ConfigError("config.descriptor must be the device descriptor")
        self.simulate = bool(config.get("simulate", False))
        caps = {c.get("name"): c for c in self.descriptor.get("capabilities", [])}
        for cap in caps.values():
            ops = set(cap.get("operations", []))
            if ops - READ_ONLY:
                raise ConfigError(
                    f"{cap['name']}: operations {sorted(ops - READ_ONLY)} are not supported; "
                    "the ROS 2 adapter only observes"
                )
        topics = config.get("topics")
        if not isinstance(topics, dict) or not topics:
            raise ConfigError("config.topics must map topic names to {type, stale_after_ms, fields}")
        self.topics = {}
        self.fields = {}
        for topic, t in topics.items():
            if not topic.startswith("/"):
                raise ConfigError(f"topic {topic!r} must be an absolute name")
            extra = set(t) - {"type", "stale_after_ms", "qos", "fields"}
            if extra:
                raise ConfigError(f"topics.{topic}: unknown keys {sorted(extra)}")
            qos = t.get("qos", "sensor_data")
            if qos not in ("sensor_data", "system_default"):
                raise ConfigError(f"topics.{topic}.qos must be sensor_data or system_default")
            self.topics[topic] = {
                "type": message_type(t["type"]),
                "type_name": t["type"],
                "stale_after_ms": int(t.get("stale_after_ms", 2000)),
                "qos": qos,
            }
            for cap, path in t.get("fields", {}).items():
                if cap not in caps:
                    raise ConfigError(f"topics.{topic}.fields.{cap}: no such capability in the descriptor")
                self.fields[cap] = (topic, path)
        for cap in caps.values():
            if cap.get("kind") == "property" and "read" in cap.get("operations", []) and cap["name"] not in self.fields:
                raise ConfigError(f"{cap['name']} is readable but no topic field maps to it")


class Observer:
    """A ROS 2 node, spun on its own thread, that keeps the newest message per topic."""

    def __init__(self, settings: Settings):
        self.settings = settings
        self.latest: dict = {}
        self.lock = threading.Lock()
        self.context = rclpy.Context()
        rclpy.init(context=self.context, domain_id=domain_id())
        self.node = Node("openmhs_observer", context=self.context)
        for topic, t in settings.topics.items():
            profile = QoSPresetProfiles.SENSOR_DATA.value if t["qos"] == "sensor_data" else QoSPresetProfiles.SYSTEM_DEFAULT.value
            self.node.create_subscription(t["type"], topic, self._keep(topic), profile)
        self.executor = SingleThreadedExecutor(context=self.context)
        self.executor.add_node(self.node)
        self.thread = threading.Thread(target=self._spin, daemon=True)
        self.thread.start()

    def _keep(self, topic):
        def callback(msg):
            with self.lock:
                self.latest[topic] = (msg, time.monotonic())

        return callback

    def _spin(self):
        try:
            self.executor.spin()
        except Exception:  # shut down
            pass

    def newest(self, topic):
        with self.lock:
            return self.latest.get(topic)

    def close(self):
        self.executor.shutdown(timeout_sec=1)
        self.node.destroy_node()
        if self.context.ok():
            rclpy.shutdown(context=self.context)


def domain_id() -> int:
    """ROS_DOMAIN_ID, which the manifest sets in ``runtime.env``."""
    return int(os.environ.get("ROS_DOMAIN_ID", "0"))


class Ros2Adapter(Adapter):
    adapter_id = "openmhs.py.ros2"
    version = "0.1.0"

    def __init__(self):
        self.settings: Settings | None = None
        self.observer: Observer | None = None
        self.sim: RobotSim | None = None

    async def initialize(self, config, permissions):
        await super().initialize(config, permissions)
        try:
            self.settings = Settings(config)
        except ConfigError as e:
            raise AdapterError("internal", f"config: {e}") from e
        if self.settings.simulate:
            self.fixture_faults = ("link_down",)

    async def describe(self):
        return self.settings.descriptor

    async def connect(self):
        if self.observer is None:
            self.observer = await asyncio.to_thread(Observer, self.settings)
        if self.settings.simulate and self.sim is None:
            self.sim = await asyncio.to_thread(RobotSim)
        # Discovery takes a moment. Wait (bounded) for every topic's first message, so that
        # "connected" means "observing"; topics still silent then show up in health.
        deadline = time.monotonic() + FIRST_MESSAGE_S
        while time.monotonic() < deadline:
            if all(self.observer.newest(t) is not None for t in self.settings.topics):
                return
            await asyncio.sleep(0.05)

    def _fresh(self, topic):
        if self.observer is None:
            raise AdapterError("disconnected", "not connected")
        t = self.settings.topics[topic]
        newest = self.observer.newest(topic)
        if newest is None:
            raise AdapterError("disconnected", f"no message on {topic} yet")
        msg, received = newest
        age_ms = (time.monotonic() - received) * 1000
        if age_ms > t["stale_after_ms"]:
            raise AdapterError(
                "disconnected", f"nothing on {topic} for {age_ms:.0f} ms (stale_after_ms {t['stale_after_ms']})"
            )
        return msg

    async def health(self):
        if self.observer is None:
            return ("disconnected", "not connected")
        stale = []
        for topic in self.settings.topics:
            try:
                self._fresh(topic)
            except AdapterError as e:
                stale.append(e.message)
        if not stale:
            return "healthy"
        if len(stale) == len(self.settings.topics):
            return ("disconnected", "; ".join(stale))
        return ("degraded", "; ".join(stale))

    async def read(self, capability):
        s = self.settings
        cap = next((c for c in s.descriptor.get("capabilities", []) if c.get("name") == capability), None)
        if capability not in s.fields or cap is None or "read" not in cap.get("operations", []):
            raise AdapterError("unsupported", f"no readable capability {capability!r}")
        topic, path = s.fields[capability]
        msg = self._fresh(topic)
        try:
            raw = field(msg, path)
        except (AttributeError, IndexError, ValueError) as e:
            raise AdapterError("malformed_response", f"{topic}: no field {path} ({e})") from e
        value = convert(raw, cap["value"], f"{topic}.{path}")
        return reading(value, timestamp=stamp(msg), source="simulator" if s.simulate else "device")

    async def execute(self, command, progress):
        raise AdapterError("unsupported", "the ROS 2 adapter only observes")

    async def fixture_inject(self, fault):
        if fault != "link_down" or self.sim is None:
            raise AdapterError("unsupported", f"cannot inject {fault}")
        self.sim.paused = True
        # A silent topic is only noticed once it has been silent for stale_after_ms. Return
        # when the outage is that old, so the suite observes what a real outage looks like
        # by then; the observer is not told about the fault.
        longest = max(t["stale_after_ms"] for t in self.settings.topics.values())
        await asyncio.sleep(longest / 1000 + 0.2)

    async def fixture_clear(self):
        if self.sim is not None:
            self.sim.paused = False

    async def shutdown(self):
        if self.sim is not None:
            await asyncio.to_thread(self.sim.close)
            self.sim = None
        if self.observer is not None:
            await asyncio.to_thread(self.observer.close)
            self.observer = None


class RobotSim:
    """Publishes a mobile robot's battery state at 10 Hz on its own node (conformance only).
    While ``paused`` it publishes nothing, as if the robot had dropped off the network."""

    def __init__(self):
        from sensor_msgs.msg import BatteryState

        self.BatteryState = BatteryState
        self.paused = False
        self.context = rclpy.Context()
        rclpy.init(context=self.context, domain_id=domain_id())
        self.node = Node("openmhs_sim_robot", context=self.context)
        self.pub = self.node.create_publisher(BatteryState, "/battery_state", QoSPresetProfiles.SENSOR_DATA.value)
        self.charge = 0.9
        self.node.create_timer(0.1, self._tick)
        self.executor = SingleThreadedExecutor(context=self.context)
        self.executor.add_node(self.node)
        self.thread = threading.Thread(target=self._spin, daemon=True)
        self.thread.start()

    def _spin(self):
        try:
            self.executor.spin()
        except Exception:
            pass

    def _tick(self):
        if self.paused:
            return
        self.charge = max(0.2, self.charge - 0.0001)
        msg = self.BatteryState()
        msg.header.stamp = self.node.get_clock().now().to_msg()
        msg.header.frame_id = "base_link"
        msg.voltage = 24.0 + 1.6 * self.charge
        msg.current = -1.8
        msg.temperature = 31.5
        msg.percentage = self.charge
        msg.present = True
        msg.power_supply_status = self.BatteryState.POWER_SUPPLY_STATUS_DISCHARGING
        self.pub.publish(msg)

    def close(self):
        self.executor.shutdown(timeout_sec=1)
        self.node.destroy_node()
        if self.context.ok():
            rclpy.shutdown(context=self.context)


if __name__ == "__main__":
    run(Ros2Adapter())
