"""A simulated power supply, as an out-of-process Python adapter.

The same device model and descriptor as the built-in Rust simulator (crates/openmhs-sim,
spec/examples/valid/power-supply.json), under its own adapter and device ids, so the two
can be held to the same conformance level. The output drives a 10 Ohm load; enabling it
while the load current would exceed the limit trips protection and disables it.

Run by the gateway from manifest.json, never by hand: it speaks the adapter-host protocol
on stdin and stdout (spec/adapter-protocol.md).
"""

from __future__ import annotations

import asyncio
import json
import random
from pathlib import Path

from openmhs.adapter import AdapterError, Adapter, Applied, Rejected, quantity, reading, run

DESCRIPTOR = Path(__file__).resolve().parents[3] / "spec" / "examples" / "valid" / "power-supply.json"
LOAD_OHMS = 10.0
NOISE_V = 0.002
LATENCY_S = 0.005

FAULT_BEHAVIOUR = {
    "no_response": "hang",
    "lost_ack": "apply_then_hang",
    "garbled": "apply_then_malformed",
    "refuse": "reject",
}


def number(value) -> float:
    if isinstance(value, dict) and isinstance(value.get("value"), (int, float)):
        return float(value["value"])
    raise AdapterError("not_sent", "expected a quantity")


def boolean(value) -> bool:
    if isinstance(value, bool):
        return value
    raise AdapterError("not_sent", "expected a boolean")


class PsuSim(Adapter):
    adapter_id = "openmhs.py.psu-sim"
    version = "0.1.0"
    fixture_faults = ("link_down", "no_response", "lost_ack", "garbled", "refuse")
    fixture_counts_deliveries = True

    def __init__(self) -> None:
        self.setpoint_v = 0.0
        self.enabled = False
        self.limit_a = 1.0
        self.tripped = False
        self.link_down = False
        self.behaviour = None
        self.executes = 0
        self.rng = random.Random(42)

    async def initialize(self, config, permissions):
        await super().initialize(config, permissions)
        self.rng = random.Random(config.get("seed", 42))

    async def describe(self):
        d = json.loads(DESCRIPTOR.read_text(encoding="utf-8"))
        d["device"]["id"] = "py.psu-01"
        d["device"]["name"] = "Simulated power supply (Python adapter)"
        d["device"]["serial"] = "SIM-PYPSU-0001"
        d["adapter"]["id"] = self.adapter_id
        d["adapter"]["version"] = self.version
        return d

    def link(self) -> None:
        if self.link_down:
            raise AdapterError("disconnected", "simulated disconnect")

    async def connect(self):
        self.link()

    async def health(self):
        return ("disconnected", "simulated disconnect") if self.link_down else "healthy"

    async def read(self, capability):
        self.link()
        live = self.enabled and not self.tripped
        if capability == "power.output.voltage.setpoint":
            value = quantity(self.setpoint_v, "V")
        elif capability == "power.output.voltage.measured":
            noise = (self.rng.random() * 2 - 1) * NOISE_V
            value = quantity(self.setpoint_v + noise if live else 0.0, "V")
        elif capability == "power.output.current.limit":
            value = quantity(self.limit_a, "A")
        elif capability == "power.output.current.measured":
            value = quantity(self.setpoint_v / LOAD_OHMS if live else 0.0, "A")
        elif capability == "power.output.enabled":
            value = self.enabled
        elif capability == "protection.tripped":
            value = self.tripped
        else:
            raise AdapterError("unsupported", f"no readable capability {capability!r}")
        return reading(value, source="simulator")

    async def execute(self, command, progress):
        self.executes += 1
        self.report_deliveries(self.executes)
        self.link()
        await asyncio.sleep(LATENCY_S)
        if self.behaviour == "hang":
            await asyncio.Event().wait()
        if self.behaviour == "reject":
            return Rejected("simulated rejection")

        progress.accepted()
        cap = command.capability
        if cap == "power.output.voltage.setpoint":
            self.setpoint_v = number(command.value)
        elif cap == "power.output.current.limit":
            self.limit_a = number(command.value)
        elif cap == "power.output.enabled":
            on = boolean(command.value)
            if on and self.tripped:
                return Rejected("protection tripped")
            self.enabled = on
        elif cap == "protection.reset":
            self.tripped = False
            self.enabled = False
        else:
            raise AdapterError("unsupported", f"cannot execute {cap!r}")
        if self.enabled and self.setpoint_v / LOAD_OHMS > self.limit_a:
            self.tripped = True
            self.enabled = False

        if self.behaviour == "apply_then_hang":
            await asyncio.Event().wait()
        if self.behaviour == "apply_then_malformed":
            raise AdapterError("malformed_response", "simulated garbled response")
        return Applied()

    async def fixture_inject(self, fault):
        if fault == "link_down":
            self.link_down = True
        else:
            self.behaviour = FAULT_BEHAVIOUR[fault]

    async def fixture_clear(self):
        self.link_down = False
        self.behaviour = None


if __name__ == "__main__":
    run(PsuSim())
