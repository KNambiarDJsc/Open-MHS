"""The EXAMPLE-ONLY register map of the simulated battery test instrument.

This is not a standard and not any vendor's register map. It exists so the example has
something concrete to read. It is the single source of truth: ``build_profile.py`` turns it
into the OpenMHS profile, and ``simulator.py`` serves it, so the two cannot drift apart.

Everything is read-only for an agent. The three control points at the end (a voltage limit
and test start/stop) exist so that the permission layer has something real to deny.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

DEVICE_ID = "sim.battery-test-01"
PROFILE_LABEL = "EXAMPLE-ONLY"

SAMPLE_SLOTS = 4
SESSION_SLOTS = 2

SESSION_STATUS = ["completed", "aborted", "error"]
SAMPLE_VALID = ["ok", "missing", "suspect"]
INSTRUMENT_STATE = ["idle", "testing", "fault"]
DATA_QUALITY = ["ok", "suspect"]


@dataclass(frozen=True)
class Reg:
    """One capability and where it lives."""

    name: str
    table: str  # coil | discrete_input | input | holding
    address: int
    ty: str  # number | integer | boolean | enum
    description: str
    kind: str = "u16"  # u16 | i16 | u32 (ignored for bit tables)
    scale: float = 1.0
    quantity: str | None = None
    unit: str | None = None
    range: tuple[float, float] | None = None
    values: tuple[str, ...] | None = None
    writable: bool = False  # a control point: the agent must not be able to use it
    action: str | None = None  # "write true" actions: coil written when invoked
    side_effect: str = "none"
    extra: dict[str, Any] = field(default_factory=dict, compare=False)

    @property
    def width(self) -> int:
        return 2 if self.kind == "u32" and self.table in ("input", "holding") else 1

    @property
    def is_control(self) -> bool:
        return self.writable or self.action is not None


def _session_base(k: int) -> int:
    return 10 + (k - 1) * 40


def _sample_base(k: int, j: int) -> int:
    return _session_base(k) + 10 + (j - 1) * 6


def registers() -> list[Reg]:
    """Every capability, in a stable order."""
    regs: list[Reg] = [
        Reg("instrument.simulated", "discrete_input", 0, "boolean",
            "Always true: this instrument is a simulator (EXAMPLE-ONLY profile)."),
        Reg("instrument.state", "input", 0, "enum", "What the instrument is doing.",
            values=tuple(INSTRUMENT_STATE)),
        Reg("telemetry.data_quality", "input", 1, "enum",
            "The instrument's own judgement of its latest measurement.",
            values=tuple(DATA_QUALITY)),
        Reg("telemetry.voltage", "input", 2, "number", "Measured terminal voltage.",
            scale=0.001, quantity="electric_potential", unit="V", range=(0.0, 60.0)),
        Reg("telemetry.current", "input", 3, "number",
            "Measured current. Positive is charging in this example profile.",
            kind="i16", scale=0.01, quantity="electric_current", unit="A", range=(-100.0, 100.0)),
        Reg("telemetry.temperature", "input", 4, "number", "Measured surface temperature.",
            kind="i16", scale=0.1, quantity="temperature", unit="Cel", range=(-40.0, 125.0)),
        Reg("telemetry.sample_time", "input", 5, "integer",
            "When the instrument last updated its measurement, as seconds since the Unix epoch.",
            kind="u32", range=(0, 4_000_000_000)),
        Reg("session.count", "input", 7, "integer",
            f"How many completed test sessions the instrument keeps (0 to {SESSION_SLOTS}).",
            range=(0, SESSION_SLOTS)),
    ]
    for k in range(1, SESSION_SLOTS + 1):
        base = _session_base(k)
        p = f"session_{k}"
        regs += [
            Reg(f"{p}.id", "input", base, "integer",
                f"Session identifier in slot {k} (slot 1 is the most recent). EXAMPLE-ONLY numeric code.",
                range=(0, 65535)),
            Reg(f"{p}.asset_id", "input", base + 1, "integer",
                "Identifier of the asset under test. EXAMPLE-ONLY numeric code.", range=(0, 65535)),
            Reg(f"{p}.procedure_version", "input", base + 2, "integer",
                "Version of the test procedure that ran.", range=(0, 65535)),
            Reg(f"{p}.status", "input", base + 3, "enum", "How the session ended.",
                values=tuple(SESSION_STATUS)),
            Reg(f"{p}.sample_count", "input", base + 4, "integer",
                f"Samples the session recorded (this example exposes at most {SAMPLE_SLOTS}).",
                range=(0, 65535)),
            Reg(f"{p}.start_time", "input", base + 5, "integer",
                "Session start, seconds since the Unix epoch.", kind="u32", range=(0, 4_000_000_000)),
            Reg(f"{p}.end_time", "input", base + 7, "integer",
                "Session end, seconds since the Unix epoch.", kind="u32", range=(0, 4_000_000_000)),
        ]
        for j in range(1, SAMPLE_SLOTS + 1):
            sb = _sample_base(k, j)
            q = f"{p}.sample_{j}"
            regs += [
                Reg(f"{q}.time", "input", sb, "integer", "When the sample was taken (Unix seconds).",
                    kind="u32", range=(0, 4_000_000_000)),
                Reg(f"{q}.voltage", "input", sb + 2, "number", "Sample voltage.", scale=0.001,
                    quantity="electric_potential", unit="V", range=(0.0, 60.0)),
                Reg(f"{q}.current", "input", sb + 3, "number", "Sample current.", kind="i16",
                    scale=0.01, quantity="electric_current", unit="A", range=(-100.0, 100.0)),
                Reg(f"{q}.temperature", "input", sb + 4, "number", "Sample temperature.", kind="i16",
                    scale=0.1, quantity="temperature", unit="Cel", range=(-40.0, 125.0)),
                Reg(f"{q}.valid", "input", sb + 5, "enum",
                    "Whether the instrument recorded this sample. 'missing' means there is no value.",
                    values=tuple(SAMPLE_VALID)),
            ]
    # Control points. The agent in this example has no permission for any of them.
    regs += [
        Reg("limit.voltage_max", "holding", 0, "number",
            "Upper voltage limit for a test. A control point: changing it affects a battery under test.",
            scale=0.001, quantity="electric_potential", unit="V", range=(0.0, 60.0),
            writable=True, side_effect="hazardous"),
        Reg("test.start", "coil", 0, "boolean", "Start the configured test. A control point.",
            action="start", side_effect="hazardous"),
        Reg("test.stop", "coil", 1, "boolean", "Stop the running test. A control point.",
            action="stop", side_effect="reversible"),
    ]
    return regs


def by_name() -> dict[str, Reg]:
    return {r.name: r for r in registers()}


# ---------------------------------------------------------------------------- encoding


def encode(reg: Reg, value: float | int | bool | str) -> list[int]:
    """Engineering value -> register words. Raises ValueError when it cannot be held."""
    if reg.ty == "enum":
        assert reg.values is not None
        if value not in reg.values:
            raise ValueError(f"{reg.name}: {value!r} is not one of {reg.values}")
        return [reg.values.index(value)]
    if reg.ty == "boolean":
        return [1 if value else 0]
    raw_f = float(value) / reg.scale
    raw = int(round(raw_f))
    if abs(raw_f - raw) > 1e-6:
        raise ValueError(f"{reg.name}: {value} is finer than the register's resolution")
    if reg.kind == "u32":
        if not 0 <= raw <= 0xFFFFFFFF:
            raise ValueError(f"{reg.name}: {value} does not fit 32 bits")
        return [(raw >> 16) & 0xFFFF, raw & 0xFFFF]
    if reg.kind == "i16":
        if not -0x8000 <= raw <= 0x7FFF:
            raise ValueError(f"{reg.name}: {value} does not fit a signed 16-bit register")
        return [raw & 0xFFFF]
    if not 0 <= raw <= 0xFFFF:
        raise ValueError(f"{reg.name}: {value} does not fit an unsigned 16-bit register")
    return [raw]


def decode(reg: Reg, words: list[int]) -> float | int | bool | str:
    """Register words -> engineering value (no range check: that is the adapter's job)."""
    if reg.ty == "enum":
        assert reg.values is not None
        return reg.values[words[0]]
    if reg.ty == "boolean":
        return bool(words[0])
    if reg.kind == "u32":
        raw = (words[0] << 16) | words[1]
    elif reg.kind == "i16":
        raw = words[0] - 0x10000 if words[0] >= 0x8000 else words[0]
    else:
        raw = words[0]
    return raw if reg.ty == "integer" else round(raw * reg.scale, 9)
