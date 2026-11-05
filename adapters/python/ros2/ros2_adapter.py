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
import re
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
from rclpy.parameter import Parameter  # noqa: E402
from rclpy.qos import QoSPresetProfiles  # noqa: E402

from openmhs.adapter import Adapter, AdapterError, reading, run  # noqa: E402

READ_ONLY = {"read", "subscribe"}

#: How long ``connect`` waits for the first message on every topic.
FIRST_MESSAGE_S = 3.0

#: A stamp further ahead of this host than this is not trusted as the topic's newest time. It is the
#: gateway's own allowance for a reading from the future (MAX_FUTURE_SKEW_MS).
FUTURE_TOLERANCE_NS = 1_000_000_000

#: ``position[name=elbow]``: the entry of a list that is called ``elbow``, not the one at some position.
SELECTOR = re.compile(r"([A-Za-z_]\w*)\[([A-Za-z_]\w*)=([^\]]+)\]")
PATH_PART = re.compile(r"\d+|[A-Za-z_]\w*")


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


def select(parent, name: str, key: str, wanted: str):
    """The one entry of list ``parent.name`` identified by ``key``. Position is not identity in these
    messages, so zero matches, several matches, or arrays of different lengths are refused, not guessed.

    Works for a list of messages (``status[name=driver]``) and for parallel arrays, where ``key`` is a
    sibling list (``position[name=elbow]`` in ``sensor_msgs/JointState``)."""
    items = getattr(parent, name)
    if len(items) > 0 and hasattr(items[0], key):
        hits = [item for item in items if getattr(item, key) == wanted]
        if len(hits) != 1:
            raise LookupError(f"{len(hits)} entries of {name} have {key}={wanted}")
        return hits[0]
    if not hasattr(parent, key):
        raise LookupError(f"no entry of {name} has {key}={wanted}")
    keys = list(getattr(parent, key))
    hits = [i for i, k in enumerate(keys) if k == wanted]
    if len(hits) != 1:
        raise LookupError(f"{len(hits)} entries of {key} are {wanted}")
    if len(items) != len(keys):
        raise LookupError(f"{name} has {len(items)} entries and {key} has {len(keys)}")
    return items[hits[0]]


def field(msg, path: str):
    value = msg
    for part in path.split("."):
        selector = SELECTOR.fullmatch(part)
        if selector:
            value = select(value, *selector.groups())
        elif part.isdigit():
            value = value[int(part)]
        else:
            value = getattr(value, part)
    # rclpy gives a ROS `byte` field as bytes of length one.
    if isinstance(value, bytes) and len(value) == 1:
        return value[0]
    return value


def stamp_ns(msg) -> int | None:
    """The message's ``header.stamp`` in nanoseconds, or None if it has no header or an unset stamp."""
    header = getattr(msg, "header", None)
    if header is None:
        return None
    s = header.stamp
    if s.sec == 0 and s.nanosec == 0:
        return None
    return s.sec * 1_000_000_000 + s.nanosec


def iso_time(ns: int) -> str:
    dt = datetime.fromtimestamp(ns / 1e9, tz=timezone.utc)
    return dt.isoformat(timespec="milliseconds").replace("+00:00", "Z")


def stamp(msg) -> str | None:
    ns = stamp_ns(msg)
    return None if ns is None else iso_time(ns)


def convert(value, spec: dict, where: str):
    def bad(why):
        return AdapterError("malformed_response", f"{where}: {why} (got {value!r})")

    def in_range():
        limits = spec.get("range") or {}
        if ("min" in limits and value < limits["min"]) or ("max" in limits and value > limits["max"]):
            raise bad(f"outside the declared range {limits}")

    kind = spec.get("type")
    if kind == "number":
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise bad("expected a number")
        if not math.isfinite(value):
            # sensor_msgs uses NaN for "not measured". Infinity is no measurement either.
            raise AdapterError("device_failure", f"{where}: the robot reports no value ({value})")
        in_range()
        return {"value": float(value), "unit": spec["unit"]} if "unit" in spec else float(value)
    if kind == "integer":
        if isinstance(value, bool) or not isinstance(value, int):
            raise bad("expected an integer")
        in_range()
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
        unknown = set(config) - {"descriptor", "topics", "simulate", "simulated_source"}
        if unknown:
            raise ConfigError(f"unknown config keys {sorted(unknown)}")
        self.descriptor = config.get("descriptor")
        if not isinstance(self.descriptor, dict):
            raise ConfigError("config.descriptor must be the device descriptor")
        self.simulate = bool(config.get("simulate", False))
        # The operator's word that the topics come from a simulator they run. Nothing checks it.
        self.simulated_source = bool(config.get("simulated_source", False))
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
                for part in path.split("."):
                    if not (SELECTOR.fullmatch(part) or PATH_PART.fullmatch(part)):
                        raise ConfigError(f"topics.{topic}.fields.{cap}: bad path part {part!r}")
                self.fields[cap] = (topic, path)
        for cap in caps.values():
            if cap.get("kind") == "property" and "read" in cap.get("operations", []) and cap["name"] not in self.fields:
                raise ConfigError(f"{cap['name']} is readable but no topic field maps to it")


class Observer:
    """A ROS 2 node, spun on its own thread, that keeps the newest message per topic."""

    def __init__(self, settings: Settings):
        self.settings = settings
        self.latest: dict = {}
        self.newest_stamp: dict = {}  # topic -> the highest trusted stamp accepted, in ns
        self.older: dict = {}  # topic -> messages refused since the last accepted one
        self.lock = threading.Lock()
        self.context = rclpy.Context()
        rclpy.init(context=self.context, domain_id=domain_id())
        # An observer offers nothing to the graph: no parameter services, no log topic, no type service.
        self.node = Node(
            "openmhs_observer",
            context=self.context,
            enable_rosout=False,
            start_parameter_services=False,
            parameter_overrides=[Parameter("start_type_description_service", value=False)],
        )
        for topic, t in settings.topics.items():
            profile = QoSPresetProfiles.SENSOR_DATA.value if t["qos"] == "sensor_data" else QoSPresetProfiles.SYSTEM_DEFAULT.value
            self.node.create_subscription(t["type"], topic, self._keep(topic), profile)
        self.executor = SingleThreadedExecutor(context=self.context)
        self.executor.add_node(self.node)
        self.thread = threading.Thread(target=self._spin, daemon=True)
        self.thread.start()

    def _keep(self, topic):
        def callback(msg):
            ns = stamp_ns(msg)
            now = time.time_ns()
            with self.lock:
                # An older message never replaces a newer one (late delivery, a second publisher, a
                # clock that stepped back). A stamp from the future is kept, for the gateway to refuse
                # as fresh, but cannot raise the mark, or one bad stamp would hide everything after it.
                if ns is not None and ns < self.newest_stamp.get(topic, 0):
                    self.older[topic] = self.older.get(topic, 0) + 1
                    return
                if ns is not None and ns <= now + FUTURE_TOLERANCE_NS:
                    self.newest_stamp[topic] = ns
                self.older[topic] = 0
                self.latest[topic] = (msg, time.monotonic(), now)

        return callback

    def _spin(self):
        try:
            self.executor.spin()
        except Exception:  # shut down
            pass

    def newest(self, topic):
        with self.lock:
            return self.latest.get(topic)

    def refused(self, topic) -> int:
        with self.lock:
            return self.older.get(topic, 0)

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
        msg, received, received_wall = newest
        age_ms = (time.monotonic() - received) * 1000
        if age_ms > t["stale_after_ms"]:
            refused = self.observer.refused(topic)
            if refused:
                # Messages are arriving, so this is not silence: the stamps are going backwards.
                raise AdapterError(
                    "malformed_response",
                    f"{topic}: nothing acceptable for {age_ms:.0f} ms; {refused} messages stamped "
                    "older than the newest one accepted",
                )
            raise AdapterError(
                "disconnected", f"nothing on {topic} for {age_ms:.0f} ms (stale_after_ms {t['stale_after_ms']})"
            )
        return msg, received_wall

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
        msg, received_wall = self._fresh(topic)
        try:
            raw = field(msg, path)
        except (AttributeError, LookupError, ValueError) as e:
            raise AdapterError("malformed_response", f"{topic}: no field {path} ({e})") from e
        value = convert(raw, cap["value"], f"{topic}.{path}")
        return reading(
            value,
            # With no stamp, when the message arrived: never the time of this read.
            timestamp=stamp(msg) or iso_time(received_wall),
            source="simulator" if s.simulate or s.simulated_source else "device",
        )

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
