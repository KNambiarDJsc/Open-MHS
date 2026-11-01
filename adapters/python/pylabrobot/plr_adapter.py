"""PyLabRobot liquid handler, simulated, as an out-of-process OpenMHS adapter.

PyLabRobot keeps the liquid-handling abstractions: the deck, labware, tip and volume
tracking, and the operations (pick up, aspirate, dispense, discard). This adapter only
translates between OpenMHS and one PyLabRobot ``LiquidHandler`` on the device-free
``LiquidHandlerChatterboxBackend``. It refuses every other backend: it never drives
hardware.

**What the state means.** The chatterbox backend models no physics. Every volume and tip
reported here is PyLabRobot's *software tracker* state (resources/volume_tracker.py and
tip_tracker.py). A ``VERIFIED`` transfer means PyLabRobot's trackers recorded that transfer,
not that any liquid moved.

One action, ``liquid.transfer``: a fresh tip from the rack, aspirate from a source well,
dispense into a destination well, discard the tip. It is the narrowest boundary that is
still a whole operation; no single pipette movement is exposed. Its ``transfer_id`` is read
back from ``transfer.last_id`` to verify it, and a transfer id already used is refused, so
a reused id can never verify a transfer that did not happen.

Simulation-only hooks, both off unless the manifest's ``config`` names them:

- ``state_file``: the trackers' state is saved after every completed transfer and loaded at
  start, so the simulated deck survives an adapter or gateway restart (scenario PLR-07).
- ``fault_file``: a JSON file the experiment harness writes to make the *next* transfer
  misbehave once (``{"next_transfer": "apply_then_hang"}``: PLR-04, the lost acknowledgment).
  It is read only by this simulator adapter and is not an OpenMHS mechanism.
"""

from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path

from openmhs.adapter import Adapter, AdapterError, Applied, Rejected, quantity, reading, run

SOURCE_WELLS = ("A1", "A2", "A3")
DEST_WELLS = ("B1", "B2", "B3")
MAX_TRANSFER_UL = 200.0
FAULTS = ("apply_then_hang", "hang_before_apply")  # what the harness's fault_file may name


def descriptor(adapter_id: str, version: str) -> dict:
    volume = lambda: {"type": "number", "quantity": "volume", "unit": "uL", "range": {"min": 0, "max": 360}}
    fresh = {"max_age_ms": 2000}
    caps = [
        {
            "name": "tips.available",
            "kind": "property",
            "description": "Tips left in the rack, by PyLabRobot's tip tracker.",
            "operations": ["read"],
            "value": {"type": "integer", "range": {"min": 0, "max": 96}},
            "side_effect": "none",
            "freshness": fresh,
        },
        {
            "name": "transfer.last_id",
            "kind": "property",
            "description": "transfer_id of the last transfer PyLabRobot's trackers recorded.",
            "operations": ["read"],
            "value": {"type": "string", "max_length": 64},
            "side_effect": "none",
            "freshness": fresh,
        },
        {
            "name": "transfer.count",
            "kind": "property",
            "description": "Transfers the simulator has completed (its execution counter).",
            "operations": ["read"],
            "value": {"type": "integer", "range": {"min": 0, "max": 1000000}},
            "side_effect": "none",
            "freshness": fresh,
        },
    ]
    for plate, wells in (("source", SOURCE_WELLS), ("dest", DEST_WELLS)):
        for well in wells:
            caps.append(
                {
                    "name": f"{plate}.{well.lower()}.volume",
                    "kind": "property",
                    "description": f"Liquid in {plate} well {well}, by PyLabRobot's volume tracker.",
                    "operations": ["read"],
                    "value": volume(),
                    "side_effect": "none",
                    "freshness": fresh,
                }
            )
    caps.append(
        {
            "name": "liquid.transfer",
            "kind": "action",
            "description": (
                "Pick up a fresh tip, aspirate from a source well, dispense into a destination "
                "well, discard the tip. Simulated: verification reads PyLabRobot's software "
                "trackers, not liquid."
            ),
            "operations": ["invoke"],
            "params": {
                "transfer_id": {"type": "string", "max_length": 64},
                "source": {"type": "enum", "values": list(SOURCE_WELLS)},
                "dest": {"type": "enum", "values": list(DEST_WELLS)},
                "volume": {
                    "type": "number",
                    "quantity": "volume",
                    "unit": "uL",
                    "range": {"min": 1, "max": MAX_TRANSFER_UL},
                },
            },
            "side_effect": "irreversible",
            "idempotent": False,
            "timing": {"expected_ms": 200, "timeout_ms": 3000},
            "preconditions": [
                {
                    "id": "tip-available",
                    "description": "A transfer takes a fresh tip.",
                    "capability": "tips.available",
                    "op": "gt",
                    "value": 0,
                    "max_age_ms": 2000,
                }
            ],
            "verification": {
                "mode": "readback",
                "capability": "transfer.last_id",
                "expect_param": "transfer_id",
                "timeout_ms": 3000,
            },
        }
    )
    return {
        "openmhs": "0.1",
        "device": {
            "id": "plr.lh-01",
            "name": "PyLabRobot liquid handler (chatterbox simulation)",
            "manufacturer": "PyLabRobot (simulated)",
            "model": "LiquidHandlerChatterboxBackend",
            "serial": "SIM-PLR-0001",
            "firmware": "pylabrobot",
        },
        "adapter": {"id": adapter_id, "version": version},
        "capabilities": caps,
    }


def microlitres(value) -> float:
    if isinstance(value, dict) and isinstance(value.get("value"), (int, float)) and value.get("unit") == "uL":
        return float(value["value"])
    raise AdapterError("not_sent", f"expected a volume in uL, got {value!r}")


#: The SDK's conformance faults (spec/conformance.md §3), as this simulator behaves under each.
FIXTURE = {
    "no_response": "hang_before_apply",
    "lost_ack": "apply_then_hang",
    "garbled": "apply_then_malformed",
    "refuse": "reject",
}


class PyLabRobotAdapter(Adapter):
    adapter_id = "openmhs.py.pylabrobot"
    version = "0.1.0"
    # Only reachable from `openmhs conformance`, never from a running gateway.
    fixture_faults = ("link_down", "no_response", "lost_ack", "garbled", "refuse")
    fixture_counts_deliveries = True

    def __init__(self) -> None:
        self.lh = None
        self.deck = None
        self.tips = self.source = self.dest = None
        self.count = 0
        self.last_id = ""
        self.used_ids: set[str] = set()
        self.state_file: Path | None = None
        self.fault_file: Path | None = None
        self.lock = asyncio.Lock()
        self.deliveries = 0
        self.link_down = False
        self.fixture: str | None = None

    async def initialize(self, config, permissions):
        await super().initialize(config, permissions)
        if config.get("backend", "chatterbox") != "chatterbox":
            raise AdapterError("unsupported", "this adapter only runs PyLabRobot's chatterbox simulation")
        writable = tuple(permissions.filesystem_write)

        def allowed(key: str) -> Path | None:
            path = config.get(key)
            if not path:
                return None
            target = os.path.realpath(path)
            if not any(os.path.normcase(target).startswith(os.path.normcase(os.path.realpath(w))) for w in writable):
                raise AdapterError("not_sent", f"config.{key} {path} is not under the manifest's filesystem.write")
            return Path(target)

        self.state_file = allowed("state_file")
        self.fault_file = allowed("fault_file")
        await self._build(config)

    async def _build(self, config) -> None:
        # Imported here so a missing PyLabRobot fails initialize with a clear message.
        import contextlib
        import io

        from pylabrobot.liquid_handling import LiquidHandler
        from pylabrobot.liquid_handling.backends.chatterbox import LiquidHandlerChatterboxBackend
        from pylabrobot.resources import (
            PLT_CAR_L5AC_A00,
            TIP_CAR_480_A00,
            cor_96_wellplate_360uL_Fb,
            hamilton_96_tiprack_300uL,
            set_tip_tracking,
            set_volume_tracking,
        )
        from pylabrobot.resources.hamilton import STARLetDeck

        set_tip_tracking(True)
        set_volume_tracking(True)
        self.deck = STARLetDeck()
        self.lh = LiquidHandler(backend=LiquidHandlerChatterboxBackend(num_channels=1), deck=self.deck)
        self.tips = hamilton_96_tiprack_300uL(name="tips")
        self.source = cor_96_wellplate_360uL_Fb(name="source")
        self.dest = cor_96_wellplate_360uL_Fb(name="dest")
        tip_car = TIP_CAR_480_A00(name="tip_carrier")
        tip_car[0] = self.tips
        plate_car = PLT_CAR_L5AC_A00(name="plate_carrier")
        plate_car[0] = self.source
        plate_car[1] = self.dest
        self.deck.assign_child_resource(tip_car, rails=1)
        self.deck.assign_child_resource(plate_car, rails=10)
        with contextlib.redirect_stdout(io.StringIO()):
            await self.lh.setup()

        if self.state_file and self.state_file.exists():
            saved = json.loads(self.state_file.read_text(encoding="utf-8"))
            self.deck.load_all_state(saved["deck"])
            self.count, self.last_id = saved["count"], saved["last_id"]
            self.used_ids = set(saved["used_ids"])
            return
        tips = int(config.get("tips", 96))
        self.tips.set_tip_state([i < tips for i in range(96)])
        for well, ul in (config.get("source_ul") or {}).items():
            self.source.get_well(well).tracker.set_volume(float(ul))
        self._save()

    def _save(self) -> None:
        if not self.state_file:
            return
        state = {
            "deck": self.deck.serialize_all_state(),
            "count": self.count,
            "last_id": self.last_id,
            "used_ids": sorted(self.used_ids),
        }
        tmp = self.state_file.with_suffix(".tmp")
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(state, f)
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

    async def describe(self):
        return descriptor(self.adapter_id, self.version)

    def _link(self) -> None:
        if self.link_down:
            raise AdapterError("disconnected", "simulated disconnect")

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

    def _tips_left(self) -> int:
        return sum(1 for spot in self.tips.get_all_items() if spot.tracker.has_tip)

    async def read(self, capability):
        self._link()
        if capability == "tips.available":
            value = self._tips_left()
        elif capability == "transfer.last_id":
            value = self.last_id
        elif capability == "transfer.count":
            value = self.count
        else:
            parts = capability.split(".")
            plates = {"source": (self.source, SOURCE_WELLS), "dest": (self.dest, DEST_WELLS)}
            if len(parts) != 3 or parts[2] != "volume" or parts[0] not in plates:
                raise AdapterError("unsupported", f"no readable capability {capability!r}")
            plate, wells = plates[parts[0]]
            well = parts[1].upper()
            if well not in wells:
                raise AdapterError("unsupported", f"no readable capability {capability!r}")
            value = quantity(plate.get_well(well).tracker.get_used_volume(), "uL")
        return reading(value, source="simulator")

    async def execute(self, command, progress):
        self.deliveries += 1
        self.report_deliveries(self.deliveries)
        self._link()
        if command.capability != "liquid.transfer":
            raise AdapterError("unsupported", f"cannot execute {command.capability!r}")
        p = command.params
        transfer_id, src, dst = p.get("transfer_id"), p.get("source"), p.get("dest")
        volume = microlitres(p.get("volume"))
        if not transfer_id or src not in SOURCE_WELLS or dst not in DEST_WELLS:
            raise AdapterError("not_sent", f"bad transfer parameters {p!r}")
        async with self.lock:
            if transfer_id in self.used_ids:
                # A reused id could otherwise "verify" against an earlier transfer.
                return Rejected(f"transfer_id {transfer_id!r} was already used")
            fault = self.fixture or self._take_fault()
            if fault == "reject":
                return Rejected("simulated refusal (conformance fixture)")
            if fault == "hang_before_apply":
                await asyncio.Event().wait()
            # The whole handler, head included: a refusal after the tip pickup must not
            # leave a tip on the channel. Kept in memory only, between two awaits.
            snapshot = self.lh.serialize_all_state()
            try:
                import contextlib
                import io

                with contextlib.redirect_stdout(io.StringIO()):
                    spot = next(s for s in self.tips.get_all_items() if s.tracker.has_tip)
                    await self.lh.pick_up_tips([spot])
                    await self.lh.aspirate([self.source.get_well(src)], vols=[volume])
                    await self.lh.dispense([self.dest.get_well(dst)], vols=[volume])
                    await self.lh.discard_tips()
            except StopIteration:
                self.lh.load_all_state(snapshot)
                return Rejected("PyLabRobot tip tracker: no tip left in the rack")
            except Exception as e:  # PyLabRobot's own refusal: nothing is kept.
                self.lh.load_all_state(snapshot)
                return Rejected(f"PyLabRobot refused: {type(e).__name__}: {e}")
            # Accepted only now: PyLabRobot checks tips and volumes as it goes, and a refusal
            # after ACCEPTED could only be recorded as a possibly partial FAILED
            # (spec/lifecycle.md), although the rollback above means nothing was applied.
            progress.accepted()
            self.count += 1
            self.last_id = transfer_id
            self.used_ids.add(transfer_id)
            self._save()
        if fault == "apply_then_malformed":
            raise AdapterError("malformed_response", "simulated garbled response (conformance fixture)")
        if fault == "apply_then_hang":
            # Applied and saved, and the lock released; the answer never comes back (PLR-04).
            await asyncio.Event().wait()
        return Applied({})


if __name__ == "__main__":
    run(PyLabRobotAdapter())
