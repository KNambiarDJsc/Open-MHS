"""What Micro-Manager's demo configuration actually offers (no OpenMHS). Prints JSON.

    python probe.py <dir with MMConfig_demo.cfg and the demo adapters>
"""

from __future__ import annotations

import json
import sys
import time
from pathlib import Path

from pymmcore_plus import CMMCorePlus, __version__ as PLUS_VERSION

root = Path(sys.argv[1])
core = CMMCorePlus()
core.setDeviceAdapterSearchPaths([str(root)])
core.loadSystemConfiguration(str(root / "MMConfig_demo.cfg"))

devices = {}
for label in core.getLoadedDevices():
    devices[label] = {
        "type": str(core.getDeviceType(label)).split(".")[-1],
        "library": core.getDeviceLibrary(label),
        "name": core.getDeviceName(label),
    }
xy, z, cam, shutter = core.getXYStageDevice(), core.getFocusDevice(), core.getCameraDevice(), core.getShutterDevice()
groups = {g: list(core.getAvailableConfigs(g)) for g in core.getAvailableConfigGroups()}

t = time.perf_counter()
core.setExposure(20)
core.setXYPosition(100.0, -50.0)
core.waitForDevice(xy)
core.setPosition(12.5)
core.waitForDevice(z)
image = core.snap()
snap_ms = (time.perf_counter() - t) * 1000

# What does the demo stage do with a wild request?
far = {}
try:
    core.setXYPosition(1e9, 1e9)
    core.waitForDevice(xy)
    far = {"accepted": True, "position": list(core.getXYPosition())}
except Exception as e:
    far = {"accepted": False, "error": f"{type(e).__name__}: {e}"}
limits = {}
for dev in (xy, z):
    for prop in core.getDevicePropertyNames(dev):
        if core.hasPropertyLimits(dev, prop):
            limits[f"{dev}.{prop}"] = [core.getPropertyLowerLimit(dev, prop), core.getPropertyUpperLimit(dev, prop)]

print(json.dumps({
    "pymmcore_plus": PLUS_VERSION,
    "api_version": core.getAPIVersionInfo(),
    "version": core.getVersionInfo(),
    "roles": {"xy": xy, "z": z, "camera": cam, "shutter": shutter},
    "devices": devices,
    "config_groups": groups,
    "image": {"shape": list(image.shape), "dtype": str(image.dtype), "mean": float(image.mean())},
    "positions_after_move": {"xy": list(core.getXYPosition()), "z": core.getPosition()},
    "snap_sequence_ms": round(snap_ms, 1),
    "far_xy_request": far,
    "stage_property_limits": limits,
    "camera_properties": core.getDevicePropertyNames(cam)[:40],
}, indent=1, default=str))
