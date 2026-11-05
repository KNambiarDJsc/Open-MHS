"""A scripted, read-only agent for the simulated robot. No model, no key.

It talks to the gateway through MCP tools with a credential that can only read, and it answers from
what comes back. Each statement cites the observations behind it. It adds no freshness rule of its
own: "current" means the gateway said ``usable``. It never fills a missing value and it does not
interpret one: it reports the odometry pose in its frame, not where the robot is on any map.

States of one observation, each taken from what the gateway returned:

    CURRENT      the read succeeded and the gateway says it is usable
    NOT_CURRENT  the read succeeded and the gateway says it is not usable (too old, or from the future)
    UNAVAILABLE  the source cannot be read now (silence, disconnection, timeout); no value
    INVALID      the adapter refused what it received (bad shape, outside the declared range, stamps
                 going backwards); no value
    NOT_MEASURED the robot reports no measurement (NaN or infinity); no value
    DENIED       the credential or the policy does not allow it; no value
"""

from __future__ import annotations

import datetime as dt
import re
import time
from dataclasses import dataclass, field
from typing import Any, Protocol

DEVICE = "ros2.sim-robot-01"

CURRENT, NOT_CURRENT, UNAVAILABLE, INVALID, NOT_MEASURED, DENIED, ERROR = (
    "CURRENT", "NOT_CURRENT", "UNAVAILABLE", "INVALID", "NOT_MEASURED", "DENIED", "ERROR")

_STATE_OF_ERROR = {
    "adapter.disconnected": UNAVAILABLE,
    "adapter.timeout": UNAVAILABLE,
    "device.unavailable": UNAVAILABLE,
    "adapter.not_sent": UNAVAILABLE,
    "adapter.malformed_response": INVALID,
    "adapter.device_failure": NOT_MEASURED,
}
_LEVELS = {0: "OK", 1: "WARN", 2: "ERROR", 3: "STALE"}  # diagnostic_msgs/DiagnosticStatus


class Tools(Protocol):
    def call(self, name: str, arguments: dict[str, Any] | None = None) -> Any: ...


@dataclass
class Observation:
    """One capability as the gateway returned it. ``value`` is None when there is none."""

    capability: str
    state: str
    value: Any = None
    unit: str | None = None
    timestamp: str | None = None
    age_ms: int | None = None
    max_age_ms: int | None = None
    quality: str | None = None
    source: str | None = None
    error: str | None = None

    @property
    def text(self) -> str:
        if self.value is None:
            return f"no value ({self.state})"
        if isinstance(self.value, float):
            return f"{self.value:g} {self.unit}" if self.unit not in (None, "1") else f"{self.value:g}"
        return str(self.value)

    def evidence(self) -> str:
        return (f"{self.capability}: {self.text}; state={self.state}; time={self.timestamp or '-'}; "
                f"age_ms={'-' if self.age_ms is None else self.age_ms}"
                f"/{'-' if self.max_age_ms is None else self.max_age_ms}; quality={self.quality or '-'}; "
                f"source={self.source or '-'}" + (f"; error={self.error}" if self.error else ""))


class ReadOnlyAgent:
    def __init__(self, tools: Tools, device: str = DEVICE) -> None:
        self.tools = tools
        self.device = device

    def read(self, capability: str) -> Observation:
        result = self.tools.call("read_state", {"device": self.device, "capability": capability})
        if result.is_error:
            m = re.match(r"\[([\w.]+)\]", result.text or "")
            code = m.group(1) if m else "unknown"
            refused = code.startswith(("auth.", "policy.", "command."))
            return Observation(capability, DENIED if refused else _STATE_OF_ERROR.get(code, ERROR), error=code)
        s = result.structured
        raw = s["value"]
        value, unit = (raw["value"], raw.get("unit")) if isinstance(raw, dict) else (raw, None)
        return Observation(capability, CURRENT if s["usable"] else NOT_CURRENT, value, unit, s["timestamp"],
                           s["age_ms"], s.get("max_age_ms"), s["quality"], s["source"])

    def attempt_control(self, capability: str, value: Any = None) -> dict[str, Any]:
        """Ask for a command. It has no authority to, so this is how the example shows the refusal."""
        args: dict[str, Any] = {"device": self.device, "capability": capability,
                                "idempotency_key": f"example-{capability}-{time.monotonic_ns()}"}
        if value is not None:
            args["value"] = value
        r = self.tools.call("submit_command", args)
        s = r.structured if isinstance(r.structured, dict) else {}
        reasons = [x.get("code") for x in s.get("reasons", [])]
        if r.is_error and not reasons:
            m = re.match(r"\[([\w.]+)\]", r.text or "")
            reasons = [m.group(1)] if m else []
        return {"capability": capability, "is_error": r.is_error, "outcome": s.get("outcome"),
                "state": s.get("state"), "reached_device": s.get("reached_device"), "reasons": reasons}

    def observe_robot(self) -> dict[str, Observation]:
        names = ["base.frame", "base.child_frame", "base.position.x", "base.position.y",
                 "base.orientation.z", "base.orientation.w",
                 "joint.shoulder.position", "joint.elbow.position", "joint.wrist.position",
                 "battery.voltage", "battery.charge", "diagnostics.driver.level"]
        return {n: self.read(n) for n in names}


# ------------------------------------------------------------------------- answers


@dataclass
class Answer:
    """Statements, each citing observations by number."""

    lines: list[tuple[str, list[int]]] = field(default_factory=list)
    evidence: list[Observation] = field(default_factory=list)

    def say(self, text: str, *observations: Observation) -> None:
        cites = []
        for o in observations:
            if o not in self.evidence:
                self.evidence.append(o)
            cites.append(self.evidence.index(o) + 1)
        self.lines.append((text, cites))

    def render(self) -> str:
        out = ["SIMULATED DATA (a simulated robot; not a real robot)"]
        out += [f"- {text} " + "".join(f"[{c}]" for c in cites) for text, cites in self.lines]
        out.append("Evidence:")
        out += [f"  [{i}] {o.evidence()}" for i, o in enumerate(self.evidence, 1)]
        return "\n".join(out)


def _gateway_seconds(text: str) -> float:
    """``2026-11-05T07:13:52.83Z`` as Unix seconds. Written out because Python 3.10's ``fromisoformat``
    refuses the trailing Z and fractions the gateway has trimmed."""
    whole, _, fraction = text.rstrip("Z").partition(".")
    base = dt.datetime.strptime(whole, "%Y-%m-%dT%H:%M:%S").replace(tzinfo=dt.timezone.utc)
    return base.timestamp() + (float(f"0.{fraction}") if fraction else 0.0)


def _spread_ms(observations: list[Observation]) -> int | None:
    times = [_gateway_seconds(o.timestamp) for o in observations if o.timestamp]
    return round((max(times) - min(times)) * 1000) if len(times) > 1 else None


def _describe(o: Observation) -> str:
    if o.state == CURRENT:
        return f"{o.text}, current"
    if o.state == NOT_CURRENT:
        return f"{o.text}, NOT current (the gateway says not usable, age {o.age_ms} ms, limit {o.max_age_ms} ms)"
    return f"no value ({o.state})"


def answer_robot(obs: dict[str, Observation]) -> Answer:
    """Where is the robot, what are the joint positions, is the battery reading current, and what is
    unavailable? Everything here comes from ``obs``."""
    a = Answer()
    frame, child = obs["base.frame"], obs["base.child_frame"]
    x, y, qz, qw = (obs[f"base.{n}"] for n in ("position.x", "position.y", "orientation.z", "orientation.w"))
    pose = [frame, child, x, y, qz, qw]
    if frame.value is not None and child.value is not None:
        a.say(f"The pose below is odometry: {child.value} in frame {frame.value}, not a position on any map.",
              frame, child)
    a.say(f"Odometry position: x {_describe(x)}; y {_describe(y)}.", x, y)
    a.say(f"Odometry orientation quaternion: z {_describe(qz)}; w {_describe(qw)}.", qz, qw)
    spread = _spread_ms([o for o in pose if o.timestamp])
    if spread is not None:
        a.say(f"These pose values were read one at a time; their timestamps span {spread} ms.", *pose)
    for name in ("shoulder", "elbow", "wrist"):
        o = obs[f"joint.{name}.position"]
        a.say(f"Joint {name}: {_describe(o)}.", o)
    a.say(f"Battery voltage: {_describe(obs['battery.voltage'])}.", obs["battery.voltage"])
    a.say(f"Battery charge as reported: {_describe(obs['battery.charge'])}.", obs["battery.charge"])
    level = obs["diagnostics.driver.level"]
    if level.value is not None:
        name = _LEVELS.get(level.value, "unknown")
        current = "current" if level.state == CURRENT else f"NOT current (age {level.age_ms} ms)"
        a.say(f"The driver reports diagnostic level {level.value} ({name}), {current}.", level)
    else:
        a.say(f"Driver diagnostic level: {_describe(level)}.", level)
    missing = [o for o in obs.values() if o.value is None]
    if missing:
        names = ", ".join(sorted(o.capability for o in missing))
        a.say(f"No value is available for: {names}.", *missing)
    else:
        a.say("Every observation asked for returned a value.", *obs.values())
    stale = [o for o in obs.values() if o.state == NOT_CURRENT]
    if stale:
        names = ", ".join(sorted(o.capability for o in stale))
        a.say(f"Returned but not current: {names}.", *stale)
    return a


# ------------------------------------------------------------------------- grounding

_NUMBER = re.compile(r"-?\d+(?:\.\d+)?")
#: Words that would turn an observation into a judgement or an instruction.
UNSUPPORTED = re.compile(
    r"\b(health\w*|degrad\w*|diagnos[ei]\w*|recommend\w*|should|likely|predict\w*|safe\w*"
    r"|move the|stop the|command the|on the map|located at)\b", re.IGNORECASE)


def _numbers(o: Observation, spread: int | None) -> set[str]:
    found = set(_NUMBER.findall(o.text)) | set(_NUMBER.findall(o.capability))
    for extra in (o.age_ms, o.max_age_ms, spread):
        if extra is not None:
            found.add(str(extra))
    return found


def check_grounding(answer: Answer) -> list[str]:
    """Every statement cites something; every number in it is in what it cites; no value is stated for
    an observation that has none; no judgement words. Lexical: it catches invented numbers and obvious
    overreach, and it cannot prove an answer is faithful in every other way."""
    problems = []
    for text, cites in answer.lines:
        if not cites:
            problems.append(f"no citation: {text!r}")
            continue
        cited = [answer.evidence[c - 1] for c in cites]
        spread = _spread_ms([o for o in cited if o.timestamp])
        known = set().union(*(_numbers(o, spread) for o in cited))
        for number in _NUMBER.findall(text):
            if number not in known:
                problems.append(f"{number} is in no cited observation: {text!r}")
        for word in UNSUPPORTED.findall(text):
            problems.append(f"unsupported word {word!r}: {text!r}")
    for o in answer.evidence:
        if o.value is None and o.state in (CURRENT, NOT_CURRENT):
            problems.append(f"{o.capability} has a state that needs a value but has none")
    return problems
