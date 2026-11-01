"""SiLA 2 native baseline (no OpenMHS): a server and the SDK's generic client.

Starts simulated server A or B, reads its feature definition back over SiLA, and calls the
feature: a valid target, an out-of-range one, a heater fault. Prints JSON.

    python baseline.py [heater_a|heater_b] [port]
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import sila2
from sila2.client import SilaClient

HERE = Path(__file__).resolve().parent
SERVERS = HERE.parent / "servers"


def start(name: str, port: int, fault_file: Path) -> subprocess.Popen:
    env = {**os.environ, "PYTHONPATH": str(SERVERS / name), "SIM_FAULT_FILE": str(fault_file)}
    log = open(fault_file.with_name(f"{name}.log"), "wb")
    proc = subprocess.Popen([sys.executable, "-m", name, "--insecure", "-p", str(port), "--disable-discovery"],
                            env=env, stdout=log, stderr=subprocess.STDOUT)
    last = None
    for _ in range(100):
        try:
            client = SilaClient("127.0.0.1", port, insecure=True)
            client.SiLAService.ServerName.get()
            return proc
        except Exception as e:
            last = e
            time.sleep(0.1)
    proc.kill()
    raise RuntimeError(f"{name} did not start: {last!r}; log: {fault_file.with_name(name + '.log').read_text()[-800:]}")


def err(e: Exception) -> str:
    return f"{type(e).__module__}.{type(e).__name__}: {e}"


def main() -> None:
    name = sys.argv[1] if len(sys.argv) > 1 else "heater_a"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 50061
    fault_file = Path(tempfile.mkdtemp()) / "fault.json"
    proc = start(name, port, fault_file)
    out: dict = {"sila2": sila2.__version__, "server": name}
    try:
        c = SilaClient("127.0.0.1", port, insecure=True)
        svc = c.SiLAService
        out["server_name"] = svc.ServerName.get()
        out["server_type"] = svc.ServerType.get()
        out["features"] = list(svc.ImplementedFeatures.get())
        fdl = svc.GetFeatureDefinition(FeatureIdentifier=next(f for f in out["features"] if "TemperatureController" in f))
        out["fdl_bytes"] = len(fdl.FeatureDefinition)
        tc = c.TemperatureController
        t = time.perf_counter()
        tc.SetTargetTemperature(TargetTemperature=37.0)
        out["set_37_ms"] = round((time.perf_counter() - t) * 1000, 2)
        out["target_after"] = tc.TargetTemperature.get()
        time.sleep(1.0)
        out["current_after_1s"] = tc.CurrentTemperature.get()
        try:
            tc.SetTargetTemperature(TargetTemperature=150.0)
            out["set_150"] = "accepted"
        except Exception as e:
            out["set_150"] = err(e)
        fault_file.write_text(json.dumps({"heater_fault": True}))
        try:
            tc.SetTargetTemperature(TargetTemperature=40.0)
            out["set_while_faulted"] = "accepted"
        except Exception as e:
            out["set_while_faulted"] = err(e)
        out["faulted"] = tc.HeaterFaulted.get()
        tc.ResetHeater()
        out["faulted_after_reset"] = tc.HeaterFaulted.get()
    finally:
        proc.kill()
    print(json.dumps(out, indent=1, default=str))


if __name__ == "__main__":
    main()
