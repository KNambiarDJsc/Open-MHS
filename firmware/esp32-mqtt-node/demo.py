"""Experiment 02, end to end: an agent commands an ESP32 through OpenMHS and MQTT.

    python demo.py --bin <openmhs> [--out transcript.txt]

The ESP32 is real firmware (firmware/esp32-mqtt-node) running in Espressif's QEMU; the broker is
Mosquitto. Everything is on loopback: no hardware, no model, no external service. Needs the
environment described in README.md (mosquitto on PATH, OPENMHS_QEMU, OPENMHS_FLASH_IMAGE).
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
sys.path.insert(0, str(HERE))

import harness as h  # noqa: E402

DEVICE = "esp32.node-01"
lines: list = []


def say(text=""):
    print(text, flush=True)
    lines.append(text)


def step(n, title):
    say()
    say(f"## {n}. {title}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", default=os.environ.get("OPENMHS_BIN"))
    ap.add_argument("--out")
    a = ap.parse_args()
    missing = h.available()
    if missing or not a.bin or not Path(a.bin).exists():
        print(f"cannot run: {missing or 'need --bin (the openmhs binary)'}", file=sys.stderr)
        return 2

    home = Path(tempfile.mkdtemp(prefix="openmhs-exp02-"))
    (home / "rig").mkdir()
    env = dict(os.environ, OPENMHS_HOME=str(home / "gw"))
    env.pop("OPENMHS_TOKEN", None)
    token = None
    gateway = gateway_log = None

    def cli(*args, agent=False, show=True, tail=None):
        e = dict(env)
        if agent:
            e["OPENMHS_TOKEN"] = token
        p = subprocess.run([a.bin, *args], env=e, capture_output=True, text=True, timeout=120)
        raw = (p.stdout + p.stderr).rstrip()
        if show:
            say(f"$ openmhs {' '.join(args)}   # as {'agent' if agent else 'operator'}")
            out = re.sub(r"omhs_[A-Za-z0-9_\-]+", "omhs_<redacted>", raw).splitlines()
            for line in out[:tail] if tail else out:
                say(f"  {line[:200]}")
        return p.returncode, raw

    def node_log(since, grep="node:"):
        for line in h_rig.node.serial()[since:].splitlines():
            if grep in line:
                say(f"  node> {line.strip()[:160]}")

    try:
        say("OpenMHS Experiment 02: an agent and an ESP32 over MQTT")
        say("The ESP32 is firmware/esp32-mqtt-node running in Espressif's QEMU. Nothing physical is involved.")
        with h.Rig(home / "rig") as h_rig:
            node, broker = h_rig.node, h_rig.broker
            say(f"Peer: Mosquitto on 127.0.0.1:{broker.port}; ESP-IDF firmware in QEMU; clock supplied by the harness")
            port = h._free_port()
            gateway_log = open(home / "gateway.log", "wb")
            gateway = subprocess.Popen(
                [a.bin, "serve", "--profile", str(HERE / "profile.json"), "--port", str(port)],
                env=env, stdout=gateway_log, stderr=subprocess.STDOUT,
            )
            for _ in range(120):
                if cli("devices", "list", show=False)[0] == 0:
                    break
                time.sleep(0.5)

            step(1, "The node boots, joins the network and publishes retained state with its own clock")
            for line in node.serial().splitlines():
                if any(k in line for k in ("OpenMHS ESP32", "Got IPv4", "clock set", "connected to the broker")):
                    say(f"  node> {line.strip()[:160]}")
            say(f"  retained state on {h.PREFIX}/state: {json.dumps(broker.state())}")

            step(2, "The gateway discovered it from the profile")
            cli("devices", "list")

            step(3, "Nothing is allowed until the operator allows it")
            with h.Tap(broker, f"{h.PREFIX}/+") as tap:
                cli("read", DEVICE, "led.on")
                cli("exec", DEVICE, "led.on", "--value", "true")
            say(f"  (messages the node was sent meanwhile: {len(tap.on('set'))})")

            step(4, "Grant an agent read access; it reads the node's own timestamped values")
            cli("grants", "add", "--id", "agent-read", "--principal", "agent:demo", "--device", DEVICE,
                "--capability", "*", "--ops", "read", "--effect", "allow")
            token = re.search(r"omhs_[A-Za-z0-9_\-]+", cli("tokens", "create", "agent:demo", "--any-scope", show=False)[1]).group(0)
            for cap in ("led.on", "node.uptime", "led.pulses"):
                cli("read", DEVICE, cap, agent=True)

            step(5, "Grant write: the agent switches the LED, the node acknowledges, the gateway reads it back")
            cli("grants", "add", "--id", "agent-act", "--principal", "agent:demo", "--device", DEVICE,
                "--capability", "*", "--ops", "write,invoke", "--effect", "allow")
            mark = len(node.serial())
            cli("exec", DEVICE, "led.on", "--value", "true", agent=True, tail=9)
            node_log(mark)
            cli("exec", DEVICE, "led.on", "--value", "false", agent=True, tail=5)

            step(6, "A pulse count outside the declared 1..10 never leaves the gateway")
            with h.Tap(broker, f"{h.PREFIX}/+") as tap:
                cli("exec", DEVICE, "led.pulse", "--param", "n=11", agent=True, tail=5)
            say(f"  (messages the node was sent meanwhile: {len(tap.on('pulse'))})")

            step(7, "An action runs once; there is no readback for it, so it is reported as unverifiable")
            before = broker.state()["pulses"]
            cli("exec", DEVICE, "led.pulse", "--param", "n=2", agent=True, tail=5)
            time.sleep(1.5)
            say(f"  node pulse counter: {before} -> {broker.state()['pulses']}")

            step(8, "The node hangs (the whole machine is frozen); a write times out")
            node.pause()
            rc, out = cli("exec", DEVICE, "led.on", "--value", "true", agent=True, tail=8)
            stalled = re.search(r"^command\s+(\S+)", out, re.M).group(1)
            time.sleep(5)
            cli("read", DEVICE, "led.on", agent=True)
            say("  ...the node is released (and its clock re-synchronised)")
            node.resume()
            h_rig.clock.sync_now()
            time.sleep(6)
            say(f"  retained state now: {json.dumps(broker.state())}  (the frozen node did apply the write, late)")
            say("  the write has a readback, so the operator can reconcile it against the node:")
            cli("commands", "reconcile", stalled, tail=8)

            step(9, "The node is powered off; an action goes nowhere and is not retried")
            node.kill()
            with h.Tap(broker, f"{h.PREFIX}/+") as tap:
                rc, out = cli("exec", DEVICE, "led.pulse", "--param", "n=1", agent=True, tail=9)
            say(f"  (pulse messages the gateway published: {len(tap.on('pulse'))}; it does not retry)")
            cid = re.search(r"^command\s+(\S+)", out, re.M).group(1)
            cli("commands", "reconcile", cid)
            node.power_cycle()
            h_rig.clock.sync_now()
            time.sleep(4)
            say(f"  after power-on: {json.dumps(broker.state())}  (the counter restarted; nothing was applied late)")
            say("  the agent retries; a pulse is not idempotent, so it is refused until the first is settled:")
            with h.Tap(broker, f"{h.PREFIX}/+") as tap:
                cli("exec", DEVICE, "led.pulse", "--param", "n=1", agent=True, tail=6)
            say(f"  (pulse messages the gateway published: {len(tap.on('pulse'))})")
            say("  the operator has seen the counter restart at 0 and records that, with the reason:")
            cli("commands", "resolve", cid, "--not-applied", "--reason", "node rebooted; pulse counter is 0", tail=8)

            step(10, "The audit log is intact")
            cli("audit", "verify")

            say()
            say("Simulated: real ESP-IDF firmware on an emulated ESP32 (QEMU), a real Mosquitto broker.")
            say("Not verified: a physical ESP32, Wi-Fi, a real board's timing, power, GPIO wiring or any actuator.")
    finally:
        if gateway is not None:
            gateway.terminate()
            try:
                gateway.wait(timeout=15)
            except subprocess.TimeoutExpired:
                gateway.kill()
        if gateway_log is not None:
            gateway_log.close()
        shutil.rmtree(home, ignore_errors=True)
    if a.out:
        Path(a.out).write_text("\n".join(lines) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
