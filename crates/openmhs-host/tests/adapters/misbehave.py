"""A test adapter that misbehaves on request, for crates/openmhs-host/tests/process.rs.

``config.mode`` picks the misbehaviour; ``config.marker`` names a file some modes use.
In every mode it also writes to stdout, through ``print`` and straight to file
descriptor 1, to show that neither can corrupt the protocol stream.
"""

from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path

import openmhs.adapter as sdk
from openmhs.adapter import AdapterError, Adapter, Applied, quantity, reading, run

DESCRIPTOR = {
    "openmhs": "0.1",
    "device": {"id": "test.misbehave-01", "manufacturer": "OpenMHS tests", "model": "M-1"},
    "adapter": {"id": "test.misbehave", "version": "0.1.0"},
    "capabilities": [
        {
            "name": "level.measured",
            "kind": "property",
            "operations": ["read"],
            "value": {"type": "number", "quantity": "electric_potential", "unit": "V"},
            "side_effect": "none",
        },
        {
            "name": "dose.run",
            "kind": "action",
            "description": "Not idempotent: running it twice doses twice.",
            "operations": ["invoke"],
            "params": {},
            "side_effect": "irreversible",
            "idempotent": False,
            "timing": {"timeout_ms": 5000},
            "verification": {"mode": "none", "reason": "test device; nothing to read back"},
        },
    ],
}


def noise(where: str) -> None:
    print(f"print() from {where}")
    os.write(1, f"raw write to fd 1 from {where}\n".encode())


class Misbehave(Adapter):
    adapter_id = "test.misbehave"
    version = "0.1.0"

    async def initialize(self, config, permissions):
        await super().initialize(config, permissions)
        self.mode = config.get("mode", "normal")
        self.marker = Path(config["marker"]) if config.get("marker") else None
        noise("initialize")
        if self.mode == "crash_loop" and self.marker and self.marker.exists():
            os._exit(5)
        if self.mode == "child_request":
            self._notify.__self__.out.write(b'{"jsonrpc":"2.0","id":"x","method":"sample"}\n')
            self._notify.__self__.out.flush()

    async def describe(self):
        noise("describe")
        d = json.loads(json.dumps(DESCRIPTOR))
        if self.mode == "descriptor_changes" and self.marker and self.marker.exists():
            d["device"]["model"] = "M-2"
        env = {name: name in os.environ for name in self.config.get("probe_env", [])}
        d["device"]["description"] = json.dumps(env, sort_keys=True)
        return d

    async def read(self, capability):
        noise("read")
        if self.mode in ("crash_on_read", "crash_loop", "descriptor_changes"):
            os._exit(3)
        if self.mode == "garbage_on_read":
            out = self._notify.__self__.out
            out.write(b"this is not JSON\n")
            out.flush()
        if capability != "level.measured":
            raise AdapterError("unsupported", f"no {capability}")
        return reading(quantity(1.5, "V"))

    async def execute(self, command, progress):
        noise("execute")
        if self.marker:
            with open(self.marker, "a", encoding="utf-8") as f:
                f.write(command.command_id + "\n")
        if self.mode == "crash_after_apply":
            progress.accepted()
            os._exit(4)
        if self.mode == "hang_on_execute":
            time.sleep(60)  # blocks the event loop: no more heartbeats
        return Applied()


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--versions":
        sdk.PROTOCOL_VERSIONS = tuple(sys.argv[2].split(","))
    run(Misbehave())
