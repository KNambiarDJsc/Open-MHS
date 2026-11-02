"""Opentrons protocol simulation as an out-of-process OpenMHS adapter.

Opentrons keeps the protocol API, labware, pipettes and its simulator. This adapter runs
**reviewed protocols only**: each is a file pinned by its SHA-256 in the manifest's
``config``. An agent chooses a protocol by name and its numeric parameters; it never sends
code. The bytes that were hashed are the bytes that run, so a file changed after review is
refused, both before dispatch (the ``reviewed`` precondition) and at run time.

opentrons 10.1.0 simulates the Flex only, and passes neither bundled data nor
runtime-parameter overrides to protocols at the API levels the Flex needs. So a run uses the
documented interactive context, ``opentrons.simulate.get_protocol_api``, and calls the
protocol's ``dilute(protocol, params)``.

**What a run proves.** The simulator checks that the protocol's steps are well formed for
the declared robot, labware and pipette. It does not track liquid: it accepts an overfilled
well and a negative volume (see the experiment's native baseline). A ``VERIFIED`` run means
the simulation completed and was recorded, nothing about liquid, volume or samples.

Simulation-only configuration, as in the PyLabRobot adapter: ``state_file`` (run counter and
ids survive a restart), ``evidence_dir`` (each run's command log as JSON), ``fault_file``
(the harness makes the next run apply-then-hang, or hang first).
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import types
from pathlib import Path

# Imported here, before run() starts the SDK's stdin reader thread. On Windows, importing
# numpy (which opentrons does) while that thread is blocked reading the pipe never returns:
# reproduced with a minimal adapter whose initialize() only imports opentrons.simulate.

from openmhs.adapter import Adapter, AdapterError, Applied, Rejected, reading, run

API_LEVEL = "2.20"
FAULTS = ("apply_then_hang", "hang_before_apply")
FIXTURE = {"no_response": "hang_before_apply", "lost_ack": "apply_then_hang",
           "garbled": "apply_then_malformed", "refuse": "reject"}


def descriptor(adapter_id: str, version: str, protocols: list[str], timeout_ms: int = 30000) -> dict:
    fresh = {"max_age_ms": 2000}
    vol = lambda lo, hi: {"type": "number", "quantity": "volume", "unit": "uL", "range": {"min": lo, "max": hi}}
    caps = [
        {"name": "run.last_id", "kind": "property", "operations": ["read"],
         "description": "run_id of the last run the simulator completed.",
         "value": {"type": "string", "max_length": 64}, "side_effect": "none", "freshness": fresh},
        {"name": "run.count", "kind": "property", "operations": ["read"],
         "description": "Runs the simulator completed (its execution counter).",
         "value": {"type": "integer", "range": {"min": 0, "max": 1000000}}, "side_effect": "none", "freshness": fresh},
        {"name": "run.last_commands", "kind": "property", "operations": ["read"],
         "description": "Commands in the last completed run's simulated log.",
         "value": {"type": "integer", "range": {"min": 0, "max": 1000000}}, "side_effect": "none", "freshness": fresh},
    ]
    for name in protocols:
        caps.append({"name": f"protocol.{name}.reviewed", "kind": "property", "operations": ["read"],
                     "description": "The protocol file's SHA-256 equals the reviewed one in the configuration.",
                     "value": {"type": "boolean"}, "side_effect": "none", "freshness": fresh})
        caps.append({"name": f"protocol.{name}.sha256", "kind": "property", "operations": ["read"],
                     "description": "SHA-256 of the protocol file as it is now.",
                     "value": {"type": "string", "max_length": 64}, "side_effect": "none", "freshness": fresh})
    caps.append({
        "name": "protocol.run",
        "kind": "action",
        "description": "Simulate one reviewed serial-dilution protocol on a Flex. Simulated: verification "
                       "reads the run record, not liquid.",
        "operations": ["invoke"],
        "params": {
            "run_id": {"type": "string", "max_length": 64},
            "protocol": {"type": "enum", "values": protocols},
            "n_wells": {"type": "integer", "range": {"min": 2, "max": 12}},
            # Together at most 200 uL, a well's capacity. The descriptor cannot bound a sum
            # of two parameters, so each is bounded on its own.
            "transfer_ul": vol(10, 100),
            "diluent_ul": vol(20, 100),
            "mix_reps": {"type": "integer", "range": {"min": 0, "max": 5}},
        },
        "result": {
            "commands": {"type": "integer"},
            "runlog_sha256": {"type": "string", "max_length": 64},
            "protocol_sha256": {"type": "string", "max_length": 64},
        },
        "side_effect": "irreversible",
        "idempotent": False,
        "timing": {"expected_ms": 1000, "timeout_ms": timeout_ms},
        "preconditions": [{"id": "reviewed", "description": "The protocol file is the reviewed one.",
                           "capability": f"protocol.{protocols[0]}.reviewed", "op": "eq", "value": True,
                           "max_age_ms": 2000}],
        "verification": {"mode": "readback", "capability": "run.last_id", "expect_param": "run_id",
                         "timeout_ms": 5000},
    })
    return {
        "openmhs": "0.1",
        "device": {"id": "ot.flex-01", "name": "Opentrons Flex (protocol simulation)",
                   "manufacturer": "Opentrons (simulated)", "model": "Flex", "serial": "SIM-OT-0001",
                   "firmware": "opentrons-simulate"},
        "adapter": {"id": adapter_id, "version": version},
        "capabilities": caps,
    }


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class OpentronsAdapter(Adapter):
    adapter_id = "openmhs.py.opentrons"
    version = "0.1.0"
    fixture_faults = ("link_down", "no_response", "lost_ack", "garbled", "refuse")
    fixture_counts_deliveries = True

    def __init__(self) -> None:
        self.protocols: dict[str, tuple[Path, str]] = {}
        self.count = 0
        self.last_id = ""
        self.last_commands = 0
        self.used_ids: set[str] = set()
        self.state_file = self.fault_file = self.evidence_dir = None
        self.lock = asyncio.Lock()
        self.deliveries = 0
        self.link_down = False
        self.fixture: str | None = None

    async def initialize(self, config, permissions):
        await super().initialize(config, permissions)

        def under_write(key: str) -> Path | None:
            path = config.get(key)
            return Path(permissions.check_write(path)) if path else None

        self.timeout_ms = int(config.get("run_timeout_ms", 30000))
        self.state_file = under_write("state_file")
        self.fault_file = under_write("fault_file")
        self.evidence_dir = under_write("evidence_dir")
        here = Path(__file__).resolve().parent
        for name, entry in (config.get("protocols") or {}).items():
            path = Path(entry["path"].replace("{adapter_dir}", str(here)))
            self.protocols[name] = (Path(permissions.check_read(str(path))), entry["sha256"])
        if not self.protocols:
            raise AdapterError("not_sent", "config.protocols names no reviewed protocol")

        if self.state_file and self.state_file.exists():
            saved = json.loads(self.state_file.read_text(encoding="utf-8"))
            self.count, self.last_id = saved["count"], saved["last_id"]
            self.last_commands, self.used_ids = saved["last_commands"], set(saved["used_ids"])

    def _save(self) -> None:
        if not self.state_file:
            return
        tmp = self.state_file.with_suffix(".tmp")
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump({"count": self.count, "last_id": self.last_id, "last_commands": self.last_commands,
                       "used_ids": sorted(self.used_ids)}, f)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, self.state_file)

    def _take_fault(self) -> str | None:
        if not self.fault_file or not self.fault_file.exists():
            return None
        fault = json.loads(self.fault_file.read_text(encoding="utf-8")).get("next_transfer")
        self.fault_file.unlink()
        if fault not in FAULTS:
            raise AdapterError("internal", f"unknown simulated fault {fault!r}")
        return fault

    def _link(self) -> None:
        if self.link_down:
            raise AdapterError("disconnected", "simulated disconnect")

    async def describe(self):
        return descriptor(self.adapter_id, self.version, list(self.protocols), self.timeout_ms)

    async def connect(self):
        self._link()

    async def health(self):
        return ("disconnected", "simulated disconnect") if self.link_down else "healthy"

    async def fixture_inject(self, fault):
        if fault == "link_down":
            self.link_down = True
        else:
            self.fixture = FIXTURE[fault]

    async def fixture_clear(self):
        self.link_down = False
        self.fixture = None

    async def read(self, capability):
        self._link()
        values = {"run.last_id": self.last_id, "run.count": self.count, "run.last_commands": self.last_commands}
        if capability in values:
            return reading(values[capability], source="simulator")
        parts = capability.split(".")
        if len(parts) == 3 and parts[0] == "protocol" and parts[1] in self.protocols:
            path, reviewed = self.protocols[parts[1]]
            current = sha256(path.read_bytes())
            if parts[2] == "sha256":
                return reading(current, source="simulator")
            if parts[2] == "reviewed":
                return reading(current == reviewed, source="simulator")
        raise AdapterError("unsupported", f"no readable capability {capability!r}")

    def _simulate(self, name: str, params: dict) -> tuple[list[str], str]:
        """One run on a fresh simulated Flex. Raises whatever the simulator raises."""
        from opentrons import simulate

        path, reviewed = self.protocols[name]
        code = path.read_bytes()
        digest = sha256(code)
        if digest != reviewed:
            raise PermissionError(f"protocol {name} changed since review (sha256 {digest})")
        module = types.ModuleType(f"openmhs_protocol_{name}")
        exec(compile(code, str(path), "exec"), module.__dict__)  # the reviewed bytes, exactly
        ctx = simulate.get_protocol_api(API_LEVEL, robot_type="Flex")
        module.dilute(ctx, params)
        return list(ctx.commands()), digest

    async def execute(self, command, progress):
        self.deliveries += 1
        self.report_deliveries(self.deliveries)
        self._link()
        if command.capability != "protocol.run":
            raise AdapterError("unsupported", f"cannot execute {command.capability!r}")
        p = command.params
        run_id, name = p.get("run_id"), p.get("protocol")
        if not run_id or name not in self.protocols:
            raise AdapterError("not_sent", f"bad run parameters {p!r}")
        params = {
            "n_wells": int(p["n_wells"]),
            "transfer_ul": float(p["transfer_ul"]["value"]),
            "diluent_ul": float(p["diluent_ul"]["value"]),
            "mix_reps": int(p["mix_reps"]),
        }
        fault = self.fixture or self._take_fault()
        # A device that never answers does not freeze reads or other commands: the hang
        # happens outside the lock.
        if fault == "hang_before_apply":
            await asyncio.Event().wait()
        async with self.lock:
            if run_id in self.used_ids:
                return Rejected(f"run_id {run_id!r} was already used")
            if fault == "reject":
                return Rejected("simulated refusal (conformance fixture)")
            try:
                commands, digest = await asyncio.to_thread(self._simulate, name, params)
            except Exception as e:  # the simulator's refusal; a simulation changes nothing
                return Rejected(f"Opentrons simulation refused: {type(e).__name__}: {str(e)[:400]}")
            runlog = json.dumps(commands).encode()
            if self.evidence_dir:
                self.evidence_dir.mkdir(parents=True, exist_ok=True)
                (self.evidence_dir / f"{run_id}.json").write_text(json.dumps(
                    {"run_id": run_id, "command_id": command.command_id, "protocol": name,
                     "protocol_sha256": digest, "params": params, "commands": commands}, indent=1))
            # Accepted only now: a refusal after ACCEPTED could only be recorded as FAILED.
            progress.accepted()
            self.count += 1
            self.last_id = run_id
            self.last_commands = len(commands)
            self.used_ids.add(run_id)
            self._save()
        if fault == "apply_then_malformed":
            raise AdapterError("malformed_response", "simulated garbled response (conformance fixture)")
        if fault == "apply_then_hang":
            await asyncio.Event().wait()
        return Applied({"commands": len(commands), "runlog_sha256": sha256(runlog), "protocol_sha256": digest})


if __name__ == "__main__":
    run(OpentronsAdapter())
