"""Micro-Manager (through pymmcore-plus) as an out-of-process OpenMHS adapter, demo devices only.

Micro-Manager keeps the device abstractions, the core and acquisition. One Micro-Manager core
owns every device of a microscope, so this adapter exposes **one OpenMHS device whose
components are the Micro-Manager devices**: XY stage, Z stage, camera, shutter, and the
filter wheels behind the ``Channel`` configuration group. It loads only a configuration made
of the ``DemoCamera`` and ``Utilities`` libraries and refuses anything else: it never drives
hardware.

Capabilities:
- positions and exposure are writable properties, verified by reading them back;
- the channel is the ``Channel`` group's preset;
- ``camera.snap`` is an action that takes one image, saves it with a provenance record (its
  SHA-256, exposure, channel, stage positions as Micro-Manager reports them, the time, and
  the OpenMHS command id), and is verified by reading back ``camera.last_image_id``.

**What an image proves.** The demo camera synthesizes images. Nothing here is a biological or
optical measurement; the provenance record attests what the *software* reported when the
image was taken.

Simulation-only configuration, as in the other lab adapters: ``state_file`` (positions,
exposure and channel restored at start, as a physical stage would keep its place),
``evidence_dir`` (images and their provenance), ``fault_file``.
"""

from __future__ import annotations

import numpy as np
from pymmcore_plus import CMMCorePlus

import asyncio
import hashlib
import json
import os
from datetime import datetime, timezone
from pathlib import Path

from openmhs.adapter import Adapter, AdapterError, Applied, Rejected, quantity, reading, run

ALLOWED_LIBRARIES = {"DemoCamera", "Utilities", ""}  # "" is the Core itself
XY_RANGE_UM = 5000.0
Z_RANGE_UM = (0.0, 100.0)
FIXTURE = {"no_response": "hang_before_apply", "lost_ack": "apply_then_hang",
           "garbled": "apply_then_malformed", "refuse": "reject"}
FAULTS = ("apply_then_hang", "hang_before_apply", "camera_offline", "camera_online")


def descriptor(adapter_id: str, version: str, channels: list[str]) -> dict:
    fresh = {"max_age_ms": 1000}
    um = lambda lo, hi: {"type": "number", "quantity": "length", "unit": "um", "range": {"min": lo, "max": hi}}
    axis = lambda name, comp, lo, hi: {
        "name": name, "kind": "property", "component": comp, "operations": ["read", "write"],
        "description": f"Position as Micro-Manager reports it ({comp}).",
        "value": um(lo, hi), "side_effect": "hazardous", "idempotent": True, "freshness": fresh,
        "timing": {"expected_ms": 200, "timeout_ms": 5000},
        "verification": {"mode": "readback", "tolerance": {"abs": 0.5}, "timeout_ms": 5000},
    }
    caps = [
        axis("stage.x.position", "xy_stage", -XY_RANGE_UM, XY_RANGE_UM),
        axis("stage.y.position", "xy_stage", -XY_RANGE_UM, XY_RANGE_UM),
        axis("stage.z.position", "z_stage", *Z_RANGE_UM),
        {"name": "camera.exposure", "kind": "property", "component": "camera", "operations": ["read", "write"],
         "value": {"type": "number", "quantity": "time", "unit": "ms", "range": {"min": 1, "max": 1000}},
         "side_effect": "reversible", "idempotent": True, "freshness": fresh,
         "timing": {"expected_ms": 50, "timeout_ms": 3000},
         "verification": {"mode": "readback", "tolerance": {"abs": 0.01}, "timeout_ms": 3000}},
        {"name": "channel", "kind": "property", "component": "filters", "operations": ["read", "write"],
         "description": "Preset of Micro-Manager's Channel group (filter wheels).",
         "value": {"type": "enum", "values": channels}, "side_effect": "reversible", "idempotent": True,
         "freshness": fresh, "timing": {"expected_ms": 100, "timeout_ms": 3000},
         "verification": {"mode": "readback", "timeout_ms": 3000}},
        {"name": "camera.available", "kind": "property", "component": "camera", "operations": ["read"],
         "description": "A camera device is loaded and set as the core's camera.",
         "value": {"type": "boolean"}, "side_effect": "none", "freshness": fresh},
        {"name": "camera.last_image_id", "kind": "property", "component": "camera", "operations": ["read"],
         "value": {"type": "string", "max_length": 64}, "side_effect": "none", "freshness": fresh},
        {"name": "camera.images", "kind": "property", "component": "camera", "operations": ["read"],
         "description": "Images the camera has taken (its execution counter).",
         "value": {"type": "integer", "range": {"min": 0, "max": 100000000}}, "side_effect": "none", "freshness": fresh},
        {"name": "camera.snap", "kind": "action", "component": "camera", "operations": ["invoke"],
         "description": "Take one image at the current position, exposure and channel, and record its "
                        "provenance. Synthetic image from Micro-Manager's demo camera.",
         "params": {"image_id": {"type": "string", "max_length": 64}},
         "result": {"sha256": {"type": "string", "max_length": 64}, "width": {"type": "integer"},
                    "height": {"type": "integer"}},
         "side_effect": "reversible", "idempotent": False,
         "timing": {"expected_ms": 100, "timeout_ms": 5000},
         "preconditions": [{"id": "camera", "capability": "camera.available", "op": "eq", "value": True,
                            "max_age_ms": 1000}],
         "verification": {"mode": "readback", "capability": "camera.last_image_id", "expect_param": "image_id",
                          "timeout_ms": 3000}},
    ]
    return {
        "openmhs": "0.1",
        "device": {"id": "mm.scope-01", "name": "Micro-Manager demo microscope", "manufacturer": "Micro-Manager (simulated)",
                   "model": "DemoCamera", "serial": "SIM-MM-0001", "firmware": "MMCore"},
        "adapter": {"id": adapter_id, "version": version},
        "components": [
            {"id": "xy_stage", "name": "XY stage (Micro-Manager XY)"},
            {"id": "z_stage", "name": "Focus drive (Micro-Manager Z)"},
            {"id": "camera", "name": "Camera (Micro-Manager Camera, DCam)"},
            {"id": "filters", "name": "Excitation, dichroic and emission wheels (Channel group)"},
        ],
        "capabilities": caps,
    }


def number(value) -> float:
    if isinstance(value, dict) and isinstance(value.get("value"), (int, float)):
        return float(value["value"])
    raise AdapterError("not_sent", f"expected a quantity, got {value!r}")


class MicroManagerAdapter(Adapter):
    adapter_id = "openmhs.py.micromanager"
    version = "0.1.0"
    fixture_faults = ("link_down", "no_response", "lost_ack", "garbled", "refuse")
    fixture_counts_deliveries = True

    def __init__(self) -> None:
        self.core: CMMCorePlus | None = None
        self.lock = asyncio.Lock()
        self.images = 0
        self.last_image_id = ""
        self.used_ids: set[str] = set()
        self.state_file = self.fault_file = self.evidence_dir = None
        self.deliveries = 0
        self.link_down = False
        self.fixture: str | None = None
        self.camera_label = "Camera"

    async def initialize(self, config, permissions):
        await super().initialize(config, permissions)

        def under_write(key):
            path = config.get(key)
            return Path(permissions.check_write(path)) if path else None

        self.state_file, self.fault_file = under_write("state_file"), under_write("fault_file")
        self.evidence_dir = under_write("evidence_dir")
        adapters = Path(permissions.check_read(config["adapter_dir"]))
        cfg = Path(permissions.check_read(config.get("config_file") or str(adapters / "MMConfig_demo.cfg")))
        await asyncio.to_thread(self._load, adapters, cfg)
        if self.state_file and self.state_file.exists():
            s = json.loads(self.state_file.read_text(encoding="utf-8"))
            self.images, self.last_image_id, self.used_ids = s["images"], s["last_image_id"], set(s["used_ids"])
            await asyncio.to_thread(self._restore, s["devices"])

    def _load(self, adapters: Path, cfg: Path) -> None:
        core = CMMCorePlus()
        core.setDeviceAdapterSearchPaths([str(adapters)])
        core.loadSystemConfiguration(str(cfg))
        for label in core.getLoadedDevices():
            if core.getDeviceLibrary(label) not in ALLOWED_LIBRARIES:
                raise AdapterError("unsupported", f"device {label} is from {core.getDeviceLibrary(label)}; "
                                                  "this adapter runs Micro-Manager's demo devices only")
        self.core = core
        self.camera_label = core.getCameraDevice()
        cam = self.camera_label
        self.camera_source = (core.getDeviceLibrary(cam), core.getDeviceName(cam), core.getParentLabel(cam))

    def _devices(self) -> dict:
        c = self.core
        x, y = c.getXYPosition()
        return {"x": x, "y": y, "z": c.getPosition(), "exposure": c.getExposure(),
                "channel": c.getCurrentConfig("Channel")}

    def _restore(self, d: dict) -> None:
        c = self.core
        c.setXYPosition(d["x"], d["y"])
        c.setPosition(d["z"])
        c.waitForSystem()
        c.setExposure(d["exposure"])
        if d.get("channel"):
            c.setConfig("Channel", d["channel"])

    def _save(self) -> None:
        if not self.state_file:
            return
        tmp = self.state_file.with_suffix(".tmp")
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump({"images": self.images, "last_image_id": self.last_image_id,
                       "used_ids": sorted(self.used_ids), "devices": self._devices()}, f)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, self.state_file)

    def _take_fault(self):
        if not self.fault_file or not self.fault_file.exists():
            return None
        fault = json.loads(self.fault_file.read_text(encoding="utf-8")).get("next")
        self.fault_file.unlink()
        if fault not in FAULTS:
            raise AdapterError("internal", f"unknown simulated fault {fault!r}")
        return fault

    def _camera_ok(self) -> bool:
        return bool(self.core.getCameraDevice())

    def _link(self) -> None:
        if self.link_down:
            raise AdapterError("disconnected", "simulated disconnect")

    async def describe(self):
        return descriptor(self.adapter_id, self.version, list(self.core.getAvailableConfigs("Channel")))

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
        async with self.lock:
            c = self.core
            if capability in ("stage.x.position", "stage.y.position"):
                x, y = c.getXYPosition()
                value = quantity(x if capability == "stage.x.position" else y, "um")
            elif capability == "stage.z.position":
                value = quantity(c.getPosition(), "um")
            elif capability == "camera.exposure":
                value = quantity(c.getExposure(), "ms")
            elif capability == "channel":
                value = c.getCurrentConfig("Channel")
            elif capability == "camera.available":
                value = self._camera_ok()
            elif capability == "camera.last_image_id":
                value = self.last_image_id
            elif capability == "camera.images":
                value = self.images
            else:
                raise AdapterError("unsupported", f"no readable capability {capability!r}")
        return reading(value, source="simulator")

    def _apply(self, command) -> dict:
        """Blocking Micro-Manager calls for one command. Returns the action result."""
        c = self.core
        cap = command.capability
        if cap in ("stage.x.position", "stage.y.position"):
            x, y = c.getXYPosition()
            target = number(command.value)
            c.setXYPosition(target, y) if cap == "stage.x.position" else c.setXYPosition(x, target)
            c.waitForDevice(c.getXYStageDevice())
        elif cap == "stage.z.position":
            c.setPosition(number(command.value))
            c.waitForDevice(c.getFocusDevice())
        elif cap == "camera.exposure":
            c.setExposure(number(command.value))
        elif cap == "channel":
            c.setConfig("Channel", command.value)
            c.waitForConfig("Channel", command.value)
        elif cap == "camera.snap":
            image_id = command.params["image_id"]
            before = self._devices()
            image = c.snap()
            taken = datetime.now(timezone.utc).isoformat()
            data = np.ascontiguousarray(image)
            digest = hashlib.sha256(data.tobytes()).hexdigest()
            record = {"image_id": image_id, "command_id": command.command_id, "sha256": digest,
                      "shape": list(data.shape), "dtype": str(data.dtype), "taken_at": taken,
                      "camera": self.camera_label, "software_state": before,
                      "source": "Micro-Manager DemoCamera (synthetic image)"}
            if self.evidence_dir:
                self.evidence_dir.mkdir(parents=True, exist_ok=True)
                np.save(self.evidence_dir / f"{image_id}.npy", data)
                (self.evidence_dir / f"{image_id}.json").write_text(json.dumps(record, indent=1))
            self.images += 1
            self.last_image_id = image_id
            self.used_ids.add(image_id)
            return {"sha256": digest, "width": int(data.shape[1]), "height": int(data.shape[0])}
        else:
            raise AdapterError("unsupported", f"cannot execute {cap!r}")
        return {}

    async def execute(self, command, progress):
        self.deliveries += 1
        self.report_deliveries(self.deliveries)
        self._link()
        fault = self.fixture or self._take_fault()
        # A device that never answers does not freeze reads or other commands: the hang
        # happens outside the lock.
        if fault == "hang_before_apply":
            await asyncio.Event().wait()
        async with self.lock:
            if fault == "camera_offline":
                await asyncio.to_thread(self.core.unloadDevice, self.camera_label)
                fault = None
            elif fault == "camera_online":
                await asyncio.to_thread(self._reload_camera)
                fault = None
            if command.capability == "camera.snap":
                if command.params.get("image_id") in self.used_ids:
                    return Rejected(f"image_id {command.params.get('image_id')!r} was already used")
                if not self._camera_ok():
                    return Rejected("no camera is loaded")
            if fault == "reject":
                return Rejected("simulated refusal (conformance fixture)")
            try:
                result = await asyncio.to_thread(self._apply, command)
            except AdapterError:
                raise
            except Exception as e:
                # Micro-Manager raised part-way: a move may have started, so the outcome is
                # not known to be nothing.
                raise AdapterError("device_failure", f"Micro-Manager: {type(e).__name__}: {e}") from e
            progress.accepted()
            self._save()
        if fault == "apply_then_malformed":
            raise AdapterError("malformed_response", "simulated garbled response (conformance fixture)")
        if fault == "apply_then_hang":
            await asyncio.Event().wait()
        return Applied(result)

    async def shutdown(self):
        # Blocking, and if Micro-Manager hangs here the SDK ends the process regardless.
        if self.core is not None:
            await asyncio.to_thread(self.core.unloadAllDevices)

    def _reload_camera(self) -> None:
        c = self.core
        library, name, parent = self.camera_source
        c.loadDevice(self.camera_label, library, name)
        if parent:
            c.setParentLabel(self.camera_label, parent)
        c.initializeDevice(self.camera_label)
        c.setCameraDevice(self.camera_label)


if __name__ == "__main__":
    run(MicroManagerAdapter())
