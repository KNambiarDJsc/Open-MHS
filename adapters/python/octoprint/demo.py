"""Experiment 01, end to end: an agent reads a printer through OpenMHS, against a real
OctoPrint with its Virtual Printer plugin. No hardware, no model, no network beyond loopback.

    python demo.py --octoprint <path to octoprint> --bin <path to openmhs> [--out transcript.txt]

It starts OctoPrint, a recording proxy and an OpenMHS gateway in a scratch directory, runs
the steps below with the real CLI, prints what the CLI printed, and stops everything. The
API key and tokens are random per run and are redacted from the output.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(HERE))

import octoprint_harness as h  # noqa: E402

DEVICE = "octoprint.virtual-01"
lines: list = []


def say(text=""):
    print(text, flush=True)
    lines.append(text)


def step(n, title):
    say()
    say(f"## {n}. {title}")


class Demo:
    def __init__(self, octo, proxy, binary, env, secrets):
        self.octo, self.proxy, self.bin, self.env, self.secrets = octo, proxy, binary, env, secrets
        self.token = None

    def redact(self, text):
        for s in self.secrets():
            text = text.replace(s, "<redacted>")
        return re.sub(r"omhs_[A-Za-z0-9_\-]+", "omhs_<redacted>", text)

    def cli(self, *args, as_agent=False, show=True, tail=None):
        env = dict(self.env)
        if as_agent:
            env["OPENMHS_TOKEN"] = self.token
        p = subprocess.run([self.bin, *args], env=env, capture_output=True, text=True, timeout=60)
        raw = (p.stdout + p.stderr).rstrip()
        out = self.redact(raw)
        if show:
            who = "agent" if as_agent else "operator"
            say(f"$ openmhs {' '.join(args)}   # as {who}")
            for line in (out.splitlines()[:tail] if tail else out.splitlines()):
                say(f"  {line}")
        return p.returncode, raw


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--octoprint", default=os.environ.get("OPENMHS_OCTOPRINT"))
    ap.add_argument("--bin", default=os.environ.get("OPENMHS_BIN"))
    ap.add_argument("--out", help="write the transcript here")
    ap.add_argument("--wire-log", help="write the proxy's request log (JSON) here")
    a = ap.parse_args()
    if not a.octoprint or not Path(a.octoprint).exists() or not a.bin or not Path(a.bin).exists():
        print("need --octoprint (OctoPrint executable) and --bin (openmhs binary)", file=sys.stderr)
        return 2

    home = Path(tempfile.mkdtemp(prefix="openmhs-exp01-"))
    gateway = None
    try:
        say("OpenMHS Experiment 01: an agent and a virtual 3D printer")
        say("OctoPrint's Virtual Printer plugin stands in for the printer. Nothing physical is involved.")
        with h.OctoPrintServer(a.octoprint) as octo, h.RecordingProxy(octo.url) as proxy:
            _, version = octo.request("GET", "/api/version")
            say(f"Peer: {version['text']} on {octo.url} (virtual printer, port VIRTUAL)")
            manifest = json.loads((HERE / "manifest.json").read_text(encoding="utf-8"))
            manifest["runtime"]["command"] = [sys.executable, str(HERE / "octoprint_adapter.py")]
            manifest["runtime"]["env"] = {"PYTHONPATH": str(ROOT / "python")}
            manifest["endpoints"]["network"] = [proxy.url]
            manifest["config"]["url"] = proxy.url
            (home / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
            port = h.free_port()
            env = dict(os.environ, OPENMHS_HOME=str(home / "gw"), OCTOPRINT_API_KEY=octo.key)
            env.pop("OPENMHS_TOKEN", None)
            gateway = subprocess.Popen(
                [a.bin, "serve", "--adapter-manifest", str(home / "manifest.json"), "--allow-unsigned-adapters", "--port", str(port)],
                env=env, stdout=open(home / "gateway.log", "wb"), stderr=subprocess.STDOUT,
            )
            d = Demo(octo, proxy, a.bin, env, lambda: [octo.key])
            for _ in range(120):
                if d.cli("devices", "list", show=False)[0] == 0:
                    break
                time.sleep(0.5)
            else:
                say("the gateway did not start")
                return 1
            step(1, "The gateway discovered the printer through its adapter")
            d.cli("devices", "list")
            d.cli("devices", "inspect", DEVICE, tail=12)

            step(2, "Nothing is allowed until the operator allows it")
            n = proxy.count()
            d.cli("read", DEVICE, "hotend.temperature")
            say(f"  (requests that reached OctoPrint meanwhile: {proxy.count() - n})")

            step(3, "Grant an agent read access, and nothing else")
            d.cli("grants", "add", "--id", "agent-printer-read", "--principal", "agent:demo", "--device", DEVICE,
                  "--capability", "*", "--ops", "read", "--effect", "allow")
            rc, out = d.cli("tokens", "create", "agent:demo", "--any-scope", show=False)
            d.token = re.search(r"omhs_[A-Za-z0-9_\-]+", out).group(0)
            say("  (token issued to agent:demo)")

            step(4, "The agent reads the printer; the values come from OctoPrint")
            for cap in ("printer.state", "hotend.temperature", "hotend.target", "bed.temperature", "bed.target"):
                d.cli("read", DEVICE, cap, as_agent=True)

            step(5, "The agent tries to heat the nozzle")
            n = proxy.count()
            d.cli("exec", DEVICE, "hotend.target", "--value", "200Cel", as_agent=True)
            say(f"  (requests that reached OctoPrint meanwhile: {proxy.count() - n})")

            step(6, "The operator even grants write; the device still has no write capability")
            d.cli("grants", "add", "--id", "agent-printer-write", "--principal", "agent:demo", "--device", DEVICE,
                  "--capability", "*", "--ops", "write", "--effect", "allow")
            n = proxy.count()
            d.cli("exec", DEVICE, "hotend.target", "--value", "200Cel", as_agent=True)
            say(f"  (requests that reached OctoPrint meanwhile: {proxy.count() - n})")
            _, printer = octo.request("GET", "/api/printer")
            say(f"  OctoPrint's own view afterwards: hotend target {printer['temperature']['tool0']['target']} C (unchanged)")

            step(7, "The printer is unplugged (OctoPrint debug command !!DEBUG:action_disconnect)")
            octo.disconnect_printer()
            say(f"  OctoPrint connection state: {octo.connection_state()}")
            d.cli("read", DEVICE, "hotend.temperature", as_agent=True)
            d.cli("read", DEVICE, "printer.state", as_agent=True)

            step(8, "It is plugged back in; reads recover without restarting anything")
            octo.reconnect_printer()
            d.cli("read", DEVICE, "hotend.temperature", as_agent=True)

            step(9, "OctoPrint itself goes away (the proxy drops every connection)")
            proxy.mode = ("drop",)
            d.cli("read", DEVICE, "hotend.temperature", as_agent=True)
            proxy.mode = ("pass",)
            d.cli("read", DEVICE, "hotend.temperature", as_agent=True)

            step(10, "The audit log is intact")
            d.cli("audit", "verify")

            step(11, "What the adapter actually sent to OctoPrint")
            with proxy.lock:
                reqs = list(proxy.requests)
            paths = sorted({p for _, p, _ in reqs})
            say(f"  requests: {len(reqs)}   methods: {sorted({m for m, _, _ in reqs})}   paths: {paths}")
            ok = {m for m, _, _ in reqs} == {"GET"} and set(paths) <= {"/api/connection", "/api/printer"}
            say(f"  read-only on the wire: {'yes' if ok else 'NO'}")
            if a.wire_log:
                Path(a.wire_log).write_text(json.dumps(reqs, indent=1) + "\n", encoding="utf-8")
            say()
            say("Simulated: OctoPrint is real software, the printer behind it is its Virtual Printer plugin.")
            say("Not verified: any physical printer, serial link, firmware or heater.")
            rc = 0 if ok else 1
    finally:
        if gateway is not None:
            gateway.terminate()
            try:
                gateway.wait(timeout=15)
            except subprocess.TimeoutExpired:
                gateway.kill()
        shutil.rmtree(home, ignore_errors=True)
    if a.out:
        Path(a.out).write_text("\n".join(lines) + "\n", encoding="utf-8")
    return rc


if __name__ == "__main__":
    sys.exit(main())
