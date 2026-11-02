"""SILA-01..SILA-05 and a latency run against one simulated SiLA 2 server; SILA-06 is the same
run against the other server with the same adapter and overrides (compare the two results).

Each scenario starts the SiLA server (heater_a or heater_b), then a real gateway whose
generic SiLA adapter (adapters/python/sila2) connects to it. Checks compare receipts with
the server itself, read directly with the SiLA SDK's own client, outside OpenMHS.

    $env:OPENMHS_BIN = "...\\openmhs.exe"
    <venv-with-sila2>\\python run_scenarios.py --server heater_a [--out ...]
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import statistics
import subprocess
import sys
import time
from pathlib import Path

from sila2.client import SilaClient

HERE = Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[2] / "shared"))
from harness import REPO, Check, Lab, binary, codes, kill_tree, reached_device, run_all, summary  # noqa: E402

EXP = HERE.parents[1]
ADAPTER = REPO / "adapters" / "python" / "sila2" / "sila_adapter.py"
OVERRIDES = EXP / "overrides" / "temperature_controller.json"
DEVICE = "sila.heater-01"
SET, RESET = "temperature_controller.set_target_temperature", "temperature_controller.reset_heater"
TARGET, CURRENT, FAULTED = ("temperature_controller.target_temperature", "temperature_controller.current_temperature",
                            "temperature_controller.heater_faulted")
SERVER = "heater_a"
GRANTS = [
    {"id": "read", "device": DEVICE, "capability": "*", "operations": ["read"], "effect": "allow"},
    {"id": "invoke", "device": DEVICE, "capability": "*", "operations": ["invoke"], "effect": "allow"},
]


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class SilaLab(Lab):
    def __init__(self, root: Path):
        root.mkdir(parents=True)
        self.port = free_port()
        self.fault_file = root / "sim_fault.json"
        self.report = root / "mapping_report.json"
        url = f"sila://127.0.0.1:{self.port}"
        super().__init__(root, {
            "openmhs_manifest": "0.1",
            "adapter": {"id": "openmhs.py.sila2", "version": "0.1.0"},
            "runtime": {"command": [sys.executable, str(ADAPTER)],
                        "env": {"PYTHONPATH": str(REPO / "python")}, "protocol": ["0.1"]},
            "capabilities": {"allow": [{"name": "*", "operations": ["read", "invoke"]}]},
            "endpoints": {"network": [url]},
            "filesystem": {"read": [str(OVERRIDES)], "write": [str(root)]},
            "limits": {"start_timeout_ms": 60000, "call_timeout_ms": 10000},
            "config": {"server": url, "overrides": str(OVERRIDES), "report_file": str(self.report),
                       "reading_source": "simulator"},
        })
        self.server = None

    def start_server(self) -> None:
        if self.server is None or self.server.poll() is not None:
            env = {**os.environ, "PYTHONPATH": str(EXP / "servers" / SERVER), "SIM_FAULT_FILE": str(self.fault_file)}
            self.server_log = open(self.dir / "sila_server.log", "ab")
            self.server = subprocess.Popen([sys.executable, "-m", SERVER, "--insecure", "-p", str(self.port),
                                            "--disable-discovery"], env=env, stdout=self.server_log,
                                           stderr=subprocess.STDOUT)
            for _ in range(150):
                try:
                    self.direct = SilaClient("127.0.0.1", self.port, insecure=True)
                    self.direct.SiLAService.ServerName.get()
                    return
                except Exception:
                    if self.server.poll() is not None:
                        break
                    time.sleep(0.1)
            self.server_log.flush()
            log = (self.dir / "sila_server.log").read_text(encoding="utf-8", errors="replace")[-800:]
            raise RuntimeError(f"the SiLA server did not start: {log}")

    def start(self, timeout_s: float = 90) -> None:
        self.start_server()
        super().start(timeout_s)

    def kill_all(self) -> None:
        self.kill()
        if self.server:
            kill_tree(self.server)

    def fault(self, fault: dict) -> None:
        self.fault_file.write_text(json.dumps(fault))


def degc(v: float) -> dict:
    return {"value": v, "unit": "Cel"}


def invoke(client, cap, params=None, timeout=30):
    return client.call("commands.submit", {"request": {"target": {"device": DEVICE, "capability": cap},
                                                       "operation": "invoke", "params": params or {}}},
                       timeout=timeout)


def set_target(client, v):
    return invoke(client, SET, {"target_temperature": degc(v)})


# ------------------------------------------------------------------ scenarios


def sila01(lab: SilaLab, check: Check) -> dict:
    d = lab.operator.describe(DEVICE).to_dict()
    caps = {c["name"]: c for c in d["capabilities"]}
    report = json.loads(lab.report.read_text())
    check("properties mapped from the Feature Definition", {TARGET, CURRENT, FAULTED} <= set(caps), sorted(caps))
    check("reviewed commands exposed", {SET, RESET} <= set(caps))
    check("ShutDown, not reviewed, is not exposed", "temperature_controller.shut_down" not in caps
          and any("ShutDown" in e["element"] for e in report["excluded"]), report["excluded"])
    check("SiLA's degC (Kelvin, offset 273.15) became OpenMHS Cel, by structure",
          caps[CURRENT]["value"]["unit"] == "Cel" and caps[CURRENT]["value"]["quantity"] == "temperature")
    check("the override narrowed the server's 4..100 degC to 10..60",
          caps[SET]["params"]["target_temperature"]["range"] == {"min": 10.0, "max": 60.0},
          caps[SET]["params"]["target_temperature"])
    check("side effect, idempotency, verification, preconditions came from the override",
          sorted(next(f for f in report["from_override"] if f["element"].endswith("SetTargetTemperature"))["fields"])
          == ["defined_errors_before_effect", "idempotent", "preconditions", "side_effect", "timing", "verification"])
    return {"descriptor": d, "mapping_report": report}


def sila02(lab: SilaLab, check: Check) -> dict:
    agent = lab.agent("agent:sila", GRANTS)
    read = agent.read(DEVICE, TARGET).to_dict()
    native = lab.direct.TemperatureController.TargetTemperature.get()
    check("typed reading with its unit, equal to the server's own value",
          read["state"]["value"] == {"value": native, "unit": "Cel"} and read["usable"], read["state"])
    cur = agent.read(DEVICE, CURRENT).to_dict()["state"]["value"]
    check("current temperature is a quantity in Cel", cur["unit"] == "Cel", cur)
    return {"target": read["state"], "current": cur, "native_target": native}


def sila03(lab: SilaLab, check: Check) -> dict:
    agent = lab.agent("agent:sila", GRANTS)
    r = set_target(agent, 37.0)
    check("37 degC: VERIFIED by reading the target back", r["state"] == "VERIFIED", (r["state"], r.get("verification")))
    check("the server holds 37 degC", lab.direct.TemperatureController.TargetTemperature.get() == 37.0)
    out = {}
    for v, why in ((70.0, "allowed by SiLA (4..100) but not by the reviewed override"), (150.0, "outside both")):
        d = set_target(agent, v)
        out[v] = summary(d)
        check(f"{v} degC ({why}): DENIED before dispatch", d["state"] == "DENIED" and not reached_device(d),
              (d["state"], codes(d)))
    check("the server still holds 37 degC", lab.direct.TemperatureController.TargetTemperature.get() == 37.0)
    lab.direct.TemperatureController.SetTargetTemperature(TargetTemperature=70.0)
    check("natively, 70 degC is accepted: the narrowing is OpenMHS's",
          lab.direct.TemperatureController.TargetTemperature.get() == 70.0)
    return {"verified": summary(r), "denied": out}


def sila04(lab: SilaLab, check: Check) -> dict:
    agent = lab.agent("agent:sila", GRANTS)
    lab.fault({"heater_fault": True})
    r = set_target(agent, 40.0)
    detail = json.dumps(r.get("adapter_result") or {})
    check("the server's HeaterFault becomes REJECTED, with its SiLA identifier and message",
          r["state"] == "REJECTED" and "HeaterFault" in detail and "org.openmhs/research/TemperatureController" in detail,
          (r["state"], detail))
    again = set_target(agent, 41.0)
    check("while faulted, the next target is DENIED by the not-faulted precondition, not sent",
          again["state"] == "DENIED" and "command.precondition.failed" in codes(again) and not reached_device(again),
          (again["state"], codes(again)))
    reset = invoke(agent, RESET)
    check("ResetHeater: VERIFIED by reading HeaterFaulted back", reset["state"] == "VERIFIED", reset["state"])
    ok = set_target(agent, 42.0)
    check("after the reset a target is VERIFIED", ok["state"] == "VERIFIED", ok["state"])
    return {"rejected": summary(r), "denied": summary(again), "reset": summary(reset)}


def sila05(lab: SilaLab, check: Check) -> dict:
    agent = lab.agent("agent:sila", GRANTS)
    lab.fault({"next_set": "apply_then_delay", "delay_s": 8})
    t = time.perf_counter()
    lost = set_target(agent, 45.0)
    waited = (time.perf_counter() - t) * 1000
    check("the server applied 45 degC", lab.direct.TemperatureController.TargetTemperature.get() == 45.0)
    check("no answer within the 3 s timeout: UNKNOWN_OUTCOME", lost["state"] == "UNKNOWN_OUTCOME",
          (lost["state"], [t["state"] for t in lost["transitions"]]))
    rec = lab.operator.call("commands.reconcile", {"command_id": lost["command_id"]})
    check("reconciled by reading the target back: RECONCILED_APPLIED", rec["state"] == "RECONCILED_APPLIED", rec["state"])
    return {"lost": summary(lost), "reconciled": summary(rec), "waited_ms": round(waited)}


def sila07(lab: SilaLab, check: Check) -> dict:
    """The gateway dies and restarts around a lost answer. The SiLA server stays up throughout."""
    agent = lab.agent("agent:sila", GRANTS)
    lab.fault({"next_set": "apply_then_delay", "delay_s": 8})
    request = {"target": {"device": DEVICE, "capability": SET}, "operation": "invoke", "idempotency_key": "sila07",
               "params": {"target_temperature": degc(41.0)}}
    lost = agent.call("commands.submit", {"request": request}, timeout=30)
    check("the answer was lost: UNKNOWN_OUTCOME", lost["state"] == "UNKNOWN_OUTCOME", lost["state"])
    check("the server holds 41 degC", lab.direct.TemperatureController.TargetTemperature.get() == 41.0)
    lab.kill()
    lab.start()
    after = lab.operator.call("commands.get", {"command_id": lost["command_id"]})
    check("after a hard restart it is still UNKNOWN_OUTCOME", after["state"] == "UNKNOWN_OUTCOME", after["state"])
    again_agent = lab.agent("agent:sila", GRANTS)
    again = again_agent.call("commands.submit", {"request": request}, timeout=30)
    check("the same key after the restart returns the same command, and sends nothing",
          again["command_id"] == lost["command_id"] and again["state"] == "UNKNOWN_OUTCOME", again["state"])
    rec = lab.operator.call("commands.reconcile", {"command_id": lost["command_id"]})
    check("reconciled by reading the server: RECONCILED_APPLIED", rec["state"] == "RECONCILED_APPLIED", rec["state"])
    return {"lost": summary(lost), "after_restart": summary(after), "reconciled": summary(rec)}


def sila08(lab: SilaLab, check: Check) -> dict:
    """The suite really sends commands, so an adapter not told its server is simulated refuses."""
    manifest = json.loads(lab.manifest.read_text())
    manifest["config"].pop("reading_source")
    real = lab.dir / "manifest-as-if-real.json"
    real.write_text(json.dumps(manifest))
    before = lab.direct.TemperatureController.TargetTemperature.get()
    done = subprocess.run([str(binary()), "--home", str(lab.dir / "conformance-home"), "conformance",
                           "--adapter-manifest", str(real)], capture_output=True, text=True, timeout=120)
    text = done.stdout + done.stderr
    check("conformance refuses to test a server not declared simulated", done.returncode != 0
          and "no conformance fixture" in text, (done.returncode, text[-300:]))
    check("and nothing was sent to it", lab.direct.TemperatureController.TargetTemperature.get() == before)
    return {"exit": done.returncode}


def perf(lab: SilaLab, check: Check, n: int = 20) -> dict:
    agent = lab.agent("agent:sila", GRANTS)
    gov, nat = [], []
    for i in range(n):
        t = time.perf_counter()
        r = set_target(agent, 20.0 + i)
        gov.append((time.perf_counter() - t) * 1000)
        check(f"set {i} VERIFIED", r["state"] == "VERIFIED", r["state"])
    for i in range(n):
        t = time.perf_counter()
        lab.direct.TemperatureController.SetTargetTemperature(TargetTemperature=20.0 + i)
        lab.direct.TemperatureController.TargetTemperature.get()
        nat.append((time.perf_counter() - t) * 1000)
    q = lambda xs: {"median": round(statistics.median(xs), 1), "p95": round(sorted(xs)[int(0.95 * (n - 1))], 1),
                    "max": round(max(xs), 1)}
    return {"n": n, "openmhs_ms": q(gov), "native_ms": q(nat), "gateway_start_ms": round(lab.start_ms),
            "note": "native: set + read back with the SiLA SDK client"}


class Make:
    def __call__(self, root: Path) -> SilaLab:
        return SilaLab(root)  # run_all calls kill_all, so the SiLA server goes too


SCENARIOS = {
    "SILA-01": ("discover the server and describe it from its Feature Definition", sila01, Make()),
    "SILA-02": ("read typed properties", sila02, Make()),
    "SILA-03": ("govern a command: verified, narrowed, denied", sila03, Make()),
    "SILA-04": ("a native SiLA execution error, translated", sila04, Make()),
    "SILA-05": ("applied, response lost", sila05, Make()),
    "SILA-07": ("a hard gateway restart around a lost answer", sila07, Make()),
    "SILA-08": ("conformance refuses a server not declared simulated", sila08, Make()),
    "PERF": ("latency, governed vs the SiLA client alone", perf, Make()),
}


def main() -> None:
    global SERVER
    ap = argparse.ArgumentParser()
    ap.add_argument("--server", choices=["heater_a", "heater_b"], default="heater_a")
    ap.add_argument("--out", default="results.json")
    ap.add_argument("--only", nargs="*")
    ap.add_argument("--work")
    a = ap.parse_args()
    SERVER = a.server
    import sila2

    run_all(SCENARIOS, {"sila2": sila2.__version__, "sila_server": SERVER}, a.out, a.only, a.work)


if __name__ == "__main__":
    main()
