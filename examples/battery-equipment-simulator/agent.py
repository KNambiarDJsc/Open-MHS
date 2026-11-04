"""A scripted, read-only agent for the simulated battery test instrument.

It has no model and no key. It talks to the gateway through MCP tools with an agent credential
that can only read, turns what comes back into short answers, and cites the reading behind every
claim. It never fills a missing value, never smooths a bad one, and never offers a diagnosis,
health score, remaining-life estimate or advice: those are not in the data.

Data-quality vocabulary used here (see the README for how each state arises):

    OK            fresh, in range, and the instrument does not doubt it
    STALE         older than ``STALE_AFTER_S``: the instrument stopped updating, or the link is slow
    SUSPECT       the instrument (or the gateway) flags the value as doubtful; it is shown, flagged
    UNREACHABLE   the device cannot be reached; there is no value
    MALFORMED     the adapter refused the device's answer (e.g. out of the declared range); no value
    MISSING       the instrument recorded no value for this sample; there is no value
    OUT_OF_ORDER  a sample is timestamped earlier than the one before it; it is flagged, not reordered
    DENIED        the credential or policy does not allow the read
"""

from __future__ import annotations

import datetime as dt
import re
from dataclasses import dataclass, field
from typing import Any, Protocol

import register_map as rm

DEVICE = rm.DEVICE_ID
#: Example policy, not a standard: a measurement older than this is not described as current.
STALE_AFTER_S = 5.0

OK, STALE, SUSPECT, UNREACHABLE, MALFORMED, MISSING, OUT_OF_ORDER, DENIED, ERROR = (
    "OK", "STALE", "SUSPECT", "UNREACHABLE", "MALFORMED", "MISSING", "OUT_OF_ORDER", "DENIED", "ERROR")

_ERROR_STATE = {
    "adapter.malformed_response": MALFORMED,
    "adapter.disconnected": UNREACHABLE,
    "adapter.timeout": UNREACHABLE,
    "device.unavailable": UNREACHABLE,
}


class Tools(Protocol):
    """What the agent needs: MCP ``tools/call`` (the example's ``McpStdioClient`` fits)."""

    def call(self, name: str, arguments: dict[str, Any] | None = None) -> Any: ...


def iso(epoch: float) -> str:
    return dt.datetime.fromtimestamp(epoch, dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


_TIME = re.compile(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d+))?Z")


def parse_time(text: str) -> float:
    """The gateway's ``2026-11-04T05:25:49.52Z`` as Unix seconds. (``fromisoformat`` before
    Python 3.11 rejects fractions that are not 3 or 6 digits, and the gateway trims zeros.)"""
    m = _TIME.fullmatch(text)
    if not m:
        raise ValueError(f"not a gateway timestamp: {text!r}")
    base = dt.datetime.strptime(m.group(1), "%Y-%m-%dT%H:%M:%S").replace(tzinfo=dt.timezone.utc)
    return base.timestamp() + (float(f"0.{m.group(2)}") if m.group(2) else 0.0)


def fmt(value: Any, unit: str | None) -> str:
    if isinstance(value, float):
        value = f"{value:g}"
    return f"{value} {unit}" if unit else str(value)


# ------------------------------------------------------------------------- readings


@dataclass
class Reading:
    """One capability read through the gateway, with its provenance. ``value`` is None when
    there is none; it is never replaced by a default."""

    capability: str
    state: str
    value: Any = None
    unit: str | None = None
    timestamp: str | None = None
    source: str | None = None
    quality: str | None = None
    usable: bool | None = None
    error: str | None = None  # the gateway's error code, never device text
    flags: list[str] = field(default_factory=list)
    derived: dict[str, float] = field(default_factory=dict)

    @property
    def text(self) -> str:
        return fmt(self.value, self.unit) if self.value is not None else f"no value ({self.state})"


class ReadOnlyAgent:
    def __init__(self, tools: Tools, device: str = DEVICE) -> None:
        self.tools = tools
        self.device = device

    # ---------------------------------------------------------------- one read

    def read(self, capability: str) -> Reading:
        result = self.tools.call("read_state", {"device": self.device, "capability": capability})
        if result.is_error:
            m = re.match(r"\[([\w.]+)\]", result.text or "")
            code = m.group(1) if m else "unknown"
            state = DENIED if code.startswith(("auth.", "policy.")) else _ERROR_STATE.get(code, ERROR)
            return Reading(capability, state, error=code)
        s = result.structured
        raw = s["value"]
        value, unit = (raw["value"], raw.get("unit")) if isinstance(raw, dict) else (raw, None)
        r = Reading(capability, OK, value, unit, s["timestamp"], s["source"], s["quality"], s["usable"])
        if s["quality"] != "GOOD":
            r.flags.append(SUSPECT)
        if not s["usable"]:
            r.flags.append(STALE)
        return r

    # ---------------------------------------------------------------- status and telemetry

    def status(self) -> dict[str, Any]:
        result = self.tools.call("list_devices", {})
        devices = result.structured["devices"] if result.structured else []
        entry = next((d for d in devices if d.get("id", d.get("device")) == self.device), None)
        availability = Reading("list_devices", OK, entry["availability"] if entry else "UNKNOWN",
                               source="gateway")
        return {
            "availability": availability,
            "state": self.read("instrument.state"),
            "simulated": self.read("instrument.simulated"),
        }

    def snapshot(self) -> dict[str, Reading]:
        """The latest telemetry, each measurement flagged with the states that apply to it."""
        names = ["telemetry.data_quality", "telemetry.sample_time", "telemetry.voltage",
                 "telemetry.current", "telemetry.temperature"]
        got = {n: self.read(n) for n in names}
        quality, sample_time = got["telemetry.data_quality"], got["telemetry.sample_time"]
        age = None
        if sample_time.value is not None and sample_time.timestamp:
            age = int(parse_time(sample_time.timestamp)) - float(sample_time.value)  # whole seconds
            sample_time.derived["sample_age_s"] = round(age)
        for n in ("telemetry.voltage", "telemetry.current", "telemetry.temperature"):
            r = got[n]
            if r.value is None:
                continue
            if quality.value == "suspect":
                r.flags.append(SUSPECT)
            if age is not None and age > STALE_AFTER_S:
                r.flags.append(STALE)
                r.derived["sample_age_s"] = round(age)
            r.state = _worst(r.flags)
        return got

    # ---------------------------------------------------------------- sessions

    def sessions(self) -> dict[str, Any]:
        """Completed sessions, newest first, without their samples."""
        count = self.read("session.count")
        items: list[dict[str, Reading]] = []
        if count.value is not None:
            for k in range(1, min(int(count.value), rm.SESSION_SLOTS) + 1):
                items.append({f: self.read(f"session_{k}.{f}") for f in
                              ("id", "asset_id", "procedure_version", "status", "sample_count",
                               "start_time", "end_time")})
        return {"count": count, "items": items}

    def session(self, slot: int) -> dict[str, Any]:
        """One session and its samples. A sample the instrument did not record keeps no values;
        a sample that goes back in time is flagged OUT_OF_ORDER and left where it is."""
        meta = {f: self.read(f"session_{slot}.{f}") for f in
                ("id", "asset_id", "procedure_version", "status", "sample_count", "start_time", "end_time")}
        n = meta["sample_count"].value
        samples: list[dict[str, Reading]] = []
        previous: float | None = None
        for j in range(1, min(int(n or 0), rm.SAMPLE_SLOTS) + 1):
            p = f"session_{slot}.sample_{j}"
            s = {f: self.read(f"{p}.{f}") for f in ("time", "voltage", "current", "temperature", "valid")}
            valid = s["valid"].value
            if valid == "missing":
                for f in ("voltage", "current", "temperature"):  # keep missing missing
                    s[f] = Reading(s[f].capability, MISSING, timestamp=s[f].timestamp,
                                   source=s[f].source, quality=s[f].quality)
            elif valid == "suspect":
                for f in ("voltage", "current", "temperature"):
                    if s[f].value is not None:
                        s[f].flags.append(SUSPECT)
                        s[f].state = SUSPECT
            # Order is judged on the sample slots' timestamps, missing samples included: a missing
            # sample still occupies a place in time. A sample that goes back in time is flagged,
            # not reordered, and does not move the reference point.
            t = s["time"].value
            if t is not None:
                if previous is not None and t < previous:
                    for f in ("time", "voltage", "current", "temperature"):
                        if s[f].value is not None:
                            s[f].flags.append(OUT_OF_ORDER)
                            s[f].state = OUT_OF_ORDER
                else:
                    previous = t
            samples.append(s)
        return {"meta": meta, "samples": samples}

    # ---------------------------------------------------------------- what it must not do

    def attempt_control(self, capability: str, value: Any = None) -> dict[str, Any]:
        """Ask for a write or an action. The agent has no permission for it, so this is how the
        example shows the refusal: the gateway answers 'denied' and nothing reaches the device."""
        args: dict[str, Any] = {"device": self.device, "capability": capability,
                                "idempotency_key": f"example-{capability}"}
        if value is not None:
            args["value"] = value
        r = self.tools.call("submit_command", args)
        s = r.structured if isinstance(r.structured, dict) else {}
        return {"is_error": r.is_error, "outcome": s.get("outcome"), "state": s.get("state"),
                "reached_device": s.get("reached_device"),
                "reasons": [x.get("code") for x in s.get("reasons", [])]}


_PRECEDENCE = [OUT_OF_ORDER, SUSPECT, STALE]


def _worst(flags: list[str]) -> str:
    for f in _PRECEDENCE:
        if f in flags:
            return f
    return OK


# ------------------------------------------------------------------------- answers


@dataclass
class Answer:
    """Plain statements, each citing the readings it rests on (by evidence number)."""

    lines: list[tuple[str, list[int]]] = field(default_factory=list)
    evidence: list[Reading] = field(default_factory=list)
    _ids: dict[int, int] = field(default_factory=dict, repr=False)

    def cite(self, reading: Reading) -> int:
        key = id(reading)
        if key not in self._ids:
            self.evidence.append(reading)
            self._ids[key] = len(self.evidence)
        return self._ids[key]

    def say(self, text: str, *readings: Reading) -> None:
        self.lines.append((text, [self.cite(r) for r in readings]))

    def render(self) -> str:
        out = ["SIMULATED DATA (EXAMPLE-ONLY profile, no real device)"]
        for text, cites in self.lines:
            out.append(f"- {text} " + "".join(f"[{c}]" for c in cites))
        out.append("Evidence:")
        for i, r in enumerate(self.evidence, 1):
            when = r.timestamp or "-"
            out.append(f"  [{i}] {DEVICE} {r.capability}: {r.text}; read {when}; "
                       f"source={r.source or '-'}; gateway quality={r.quality or '-'}; "
                       f"state={r.state}" + (f" +{','.join(r.flags)}" if r.flags else "")
                       + (f"; error={r.error}" if r.error else ""))
        return "\n".join(out)


def answer_status(status: dict[str, Any]) -> Answer:
    a = Answer()
    state, sim, avail = status["state"], status["simulated"], status["availability"]
    a.say(f"The gateway lists the device as {avail.value}.", avail)
    if state.value is not None:
        a.say(f"The instrument reports its state as {state.value}.", state)
    else:
        a.say(f"The instrument state could not be read ({state.state}).", state)
    if sim.value is True:
        a.say("The instrument identifies itself as a simulator.", sim)
    return a


def answer_snapshot(snap: dict[str, Reading]) -> Answer:
    a = Answer()
    for name, label in (("telemetry.voltage", "Voltage"), ("telemetry.current", "Current"),
                        ("telemetry.temperature", "Temperature")):
        r = snap[name]
        if r.value is None:
            a.say(f"{label}: no value, because the reading is {r.state}.", r)
            continue
        note = ""
        if r.flags:
            note = f" Flagged {' and '.join(r.flags)}"
            if STALE in r.flags and "sample_age_s" in r.derived:
                note += f": the instrument last updated {r.derived['sample_age_s']:g} s before this read"
            note += "; do not treat as current." if STALE in r.flags else "."
        a.say(f"{label}: {r.text}.{note}", r, snap["telemetry.sample_time"])
    return a


def answer_session_list(sessions: dict[str, Any]) -> Answer:
    a = Answer()
    count = sessions["count"]
    if count.value is None:
        a.say(f"The session list could not be read ({count.state}).", count)
        return a
    a.say(f"The instrument reports {count.text} completed session(s).", count)
    for s in sessions["items"]:
        a.say(f"Session {s['id'].text}: asset {s['asset_id'].text}, procedure version "
              f"{s['procedure_version'].text}, status {s['status'].text}, {s['sample_count'].text} sample(s).",
              s["id"], s["asset_id"], s["procedure_version"], s["status"], s["sample_count"])
    return a


def answer_session(session: dict[str, Any]) -> Answer:
    a = Answer()
    m = session["meta"]
    a.say(f"Session {m['id'].text} on asset {m['asset_id'].text}, status {m['status'].text}, "
          f"procedure version {m['procedure_version'].text}.",
          m["id"], m["asset_id"], m["status"], m["procedure_version"])
    for i, s in enumerate(session["samples"], 1):
        cells = []
        for f, label in (("voltage", "V"), ("current", "I"), ("temperature", "T")):
            r = s[f]
            cells.append(f"{label} {r.text}" + (f" ({'/'.join(r.flags)})" if r.flags else ""))
        a.say(f"Sample {i} (t={s['time'].text}, Unix seconds): " + ", ".join(cells) + ".",
              s["time"], s["voltage"], s["current"], s["temperature"])
    return a


# ------------------------------------------------------------------------- grounding

_NUMBER = re.compile(r"-?\d+(?:\.\d+)?")
#: Words that would turn a reading into a judgement. None of them can be backed by this data.
UNSUPPORTED = ("health", "healthy", "degrad", "remaining life", "diagnos", "recommend", "should",
               "likely", "predict", "fail soon", "safe to")


def _numbers(reading: Reading) -> set[str]:
    nums = set(_NUMBER.findall(reading.text))
    nums |= {f"{v:g}" for v in reading.derived.values()}
    nums |= set(_NUMBER.findall(reading.capability))  # "session_1", "sample_2"
    return nums


def check_grounding(answer: Answer) -> list[str]:
    """Every statement cites a reading, every number in it appears in a cited reading, and none
    makes a judgement the data cannot support. Returns the problems (empty means grounded)."""
    problems = []
    for text, cites in answer.lines:
        if not cites:
            problems.append(f"no citation: {text!r}")
            continue
        allowed: set[str] = set()
        for c in cites:
            allowed |= _numbers(answer.evidence[c - 1])
        # "Sample 3" / "1 sample" style ordinals and counts come from the structure, not the device
        cleaned = re.sub(r"\bSample \d+\b|\b[Ss]lot \d+\b", "", text).replace(DEVICE, "")
        text_numbers = set(_NUMBER.findall(cleaned))
        for n in text_numbers - allowed:
            problems.append(f"number {n} is not in the cited readings: {text!r}")
        for word in UNSUPPORTED:
            if word in text.lower():
                problems.append(f"unsupported judgement {word!r}: {text!r}")
    return problems
