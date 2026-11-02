"""Watch OpenMHS handle a lost answer, end to end, with no hardware, no key and no network.

    cargo build -p openmhs-cli
    python examples/first-failure/first_failure.py

It starts a real gateway on simulated devices in a throwaway directory and walks through what
happens when a device does what it was told and the answer never arrives. The gateway is the one
you would deploy; only the device is simulated, so nothing here says anything about hardware.

Set OPENMHS_BIN, or have `openmhs` on PATH, or build it first. Exits non-zero if any step
did not behave as described.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "python"))

from openmhs import Client  # noqa: E402

DEVICE, SETPOINT = "sim.psu-01", "power.output.voltage.setpoint"


def find_binary() -> str:
    exe = "openmhs.exe" if sys.platform == "win32" else "openmhs"
    for candidate in (os.environ.get("OPENMHS_BIN"), shutil.which("openmhs"), REPO / "target" / "debug" / exe,
                      REPO / "target" / "release" / exe):
        if candidate and Path(candidate).exists():
            return str(candidate)
    raise SystemExit("no openmhs binary: run `cargo build -p openmhs-cli`, or set OPENMHS_BIN")


class Walkthrough:
    def __init__(self) -> None:
        self.number = 0
        self.problems = 0

    def step(self, title: str) -> None:
        self.number += 1
        print(f"\n{self.number}. {title}")

    def expect(self, what: str, ok: bool) -> None:
        print(f"   {'ok' if ok else 'NOT AS DESCRIBED'}: {what}")
        self.problems += not ok


def write(client: Client, volts: float, **extra) -> dict:
    request = {"target": {"device": DEVICE, "capability": SETPOINT}, "operation": "write",
               "value": {"value": volts, "unit": "V"}, **extra}
    return client.call("commands.submit", {"request": request})


def journey(r: dict) -> str:
    return " -> ".join(t["state"] for t in r["transitions"])


def main() -> int:
    binary = find_binary()
    w = Walkthrough()
    with tempfile.TemporaryDirectory(prefix="openmhs-first-failure-") as tmp:
        home = Path(tmp) / "home"
        gateway = subprocess.Popen([binary, "--home", str(home), "serve", "--sim", "--port", "0"],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            runtime, deadline = home / "gateway.json", time.monotonic() + 30
            info = None
            while time.monotonic() < deadline and info is None:
                try:
                    info = json.loads(runtime.read_text(encoding="utf-8"))
                    info["token"]
                except (OSError, ValueError, KeyError, TypeError):
                    info = None
                    time.sleep(0.05)
            if info is None:
                raise SystemExit("the gateway did not start within 30 s")
            operator = Client(info["url"], info["token"])

            w.step("A simulated device is discovered, with its limits declared")
            devices = {d.to_dict()["device"]["id"]: d.to_dict() for d in operator.devices()}
            setpoint = next(c for c in devices[DEVICE]["capabilities"] if c["name"] == SETPOINT)
            print(f"   devices: {', '.join(sorted(devices))}")
            print(f"   {SETPOINT}: {setpoint['value']['range']} {setpoint['value']['unit']}, "
                  f"slew {setpoint.get('slew')}")
            w.expect("the simulated supply is registered", DEVICE in devices)

            w.step("Nothing is allowed until an operator allows it")
            operator.add_grant({"id": "read", "principal": "agent:demo", "device": DEVICE, "capability": "*",
                                "operations": ["read"], "effect": "allow"})
            agent = Client(info["url"], operator.create_token("agent:demo", any_scope=True).token)
            refused = write(agent, 1.0)
            print(f"   {refused['state']}: {refused['policy']['errors'][0]['code']}")
            w.expect("an agent with no write grant is refused", refused["state"] == "DENIED")
            operator.add_grant({"id": "write", "principal": "agent:demo", "device": DEVICE, "capability": SETPOINT,
                                "operations": ["write"], "effect": "allow"})

            w.step("A valid action is carried out, and confirmed by reading the device back")
            good = write(agent, 1.5)
            print(f"   {journey(good)}")
            w.expect("VERIFIED, by a reading taken after it was sent", good["state"] == "VERIFIED")

            w.step("An invalid action is refused before anything reaches the device")
            bad = write(agent, 48.0)
            print(f"   {bad['state']}: {bad['policy']['errors'][0]['code']}: {bad['policy']['errors'][0]['message']}")
            w.expect("48 V is refused, and was never dispatched", bad["state"] == "DENIED"
                     and "DISPATCHED" not in journey(bad))

            w.step("A lost answer: the device applies the command, and the reply never comes")
            operator.sim_fault(DEVICE, "apply-then-hang")
            time.sleep(1.1)  # the slew limit
            lost = write(agent, 2.5, idempotency_key="first-failure", deadline_ms=2000)
            print(f"   {journey(lost)}")

            w.step("The outcome is uncertain, and OpenMHS says so instead of guessing")
            w.expect("UNKNOWN_OUTCOME: not a success, and not a failure", lost["state"] == "UNKNOWN_OUTCOME")
            again = write(agent, 2.5, idempotency_key="first-failure", deadline_ms=2000)
            w.expect("a retry with the same key returns the same command, and sends nothing",
                     again["command_id"] == lost["command_id"])
            operator.sim_fault(DEVICE, "clear")

            w.step("Reconciliation establishes what can be determined, by reading the device")
            settled = operator.call("commands.reconcile", {"command_id": lost["command_id"]})
            observed = settled["verification"]["observed"]["value"]
            print(f"   {settled['state']}: the device reads {observed['value']} {observed['unit']}")
            w.expect("the write did take effect, and that is now on record", settled["state"] == "RECONCILED_APPLIED")

            w.step("The evidence is checked by a separate process, without the gateway's help")
            export, checkpoint = Path(tmp) / "evidence.json", Path(tmp) / "checkpoint.json"
            subprocess.run([binary, "--home", str(home), "audit", "export", "--out", str(export), "--checkpoint-out",
                            str(checkpoint)], check=True, capture_output=True)
            verified = subprocess.run([binary, "--json", "--home", str(Path(tmp) / "elsewhere"), "audit", "verify-file",
                                       str(export), "--checkpoint", str(checkpoint)], capture_output=True, text=True)
            report = json.loads(verified.stdout)
            print(f"   audit verify-file: exit {verified.returncode}, {report.get('records')} records")
            w.expect("the exported evidence verifies offline", verified.returncode == 0)
            tampered = export.with_name("tampered.json")
            text = export.read_text(encoding="utf-8")
            tampered.write_text(text.replace("RECONCILED_APPLIED", "RECONCILED_NOT_APPLIED", 1), encoding="utf-8")
            caught = subprocess.run([binary, "--json", "--home", str(Path(tmp) / "elsewhere"), "audit", "verify-file",
                                     str(tampered), "--checkpoint", str(checkpoint)], capture_output=True, text=True)
            w.expect("and changing one word in it is detected", caught.returncode != 0)
            print(f"\n   Keep a copy with: openmhs audit export --out evidence.json\n"
                  f"   This command: {lost['command_id']}")
        finally:
            gateway.terminate()
            try:
                gateway.wait(timeout=10)
            except subprocess.TimeoutExpired:
                gateway.kill()
                gateway.wait()
    print("\nSimulated device, real gateway. What VERIFIED means here is that a simulator's software "
          "state agreed.\nNothing here is a physical measurement, or a safety claim.")
    if w.problems:
        print(f"\n{w.problems} step(s) did not behave as described.")
        return 1
    print("\nEvery step behaved as described.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
