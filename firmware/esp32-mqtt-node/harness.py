"""Test harness for firmware/esp32-mqtt-node: Mosquitto, a clock source and the firmware
running in Espressif's QEMU, all on loopback.

Nothing here is used by OpenMHS. It stands in for the things a real ESP32 deployment has
around it: a broker, a time source, and power (pause, reset and kill are the QEMU monitor's
``stop``, ``system_reset`` and ``quit``).

Requirements, none bundled: ``mosquitto``, ``mosquitto_pub`` and ``mosquitto_sub`` on PATH
(or in OPENMHS_MOSQUITTO_DIR); Espressif's ``qemu-system-xtensa`` (OPENMHS_QEMU); the
firmware's merged flash image (OPENMHS_FLASH_IMAGE, see README.md).
"""

from __future__ import annotations

import json
import os
import shutil
import socket
import subprocess
import tempfile
import threading
import time
from pathlib import Path

PREFIX = "esp32/node-01"
#: The firmware's broker URI is compiled in (main/Kconfig.projbuild).
BROKER_PORT = 18830


def tool(name: str) -> str | None:
    d = os.environ.get("OPENMHS_MOSQUITTO_DIR")
    if d:
        for ext in ("", ".exe"):
            if (Path(d) / (name + ext)).exists():
                return str(Path(d) / (name + ext))
    return shutil.which(name)


def available() -> str | None:
    """None if everything needed is present, else what is missing."""
    missing = [n for n in ("mosquitto", "mosquitto_pub", "mosquitto_sub") if tool(n) is None]
    if missing:
        return "missing on PATH: " + ", ".join(missing)
    for var in ("OPENMHS_QEMU", "OPENMHS_FLASH_IMAGE"):
        if not os.environ.get(var) or not Path(os.environ[var]).exists():
            return f"set {var}"
    return None


class Broker:
    """Mosquitto on 127.0.0.1, anonymous, no persistence. Restartable."""

    def __init__(self, port: int = BROKER_PORT):
        self.port = port
        self.dir = Path(tempfile.mkdtemp(prefix="openmhs-mosquitto-"))
        (self.dir / "mosquitto.conf").write_text(
            f"listener {port} 127.0.0.1\nallow_anonymous true\npersistence false\nlog_type all\n", encoding="utf-8"
        )
        self.process: subprocess.Popen | None = None

    def start(self) -> None:
        with socket.socket() as s:
            if s.connect_ex(("127.0.0.1", self.port)) == 0:
                raise RuntimeError(f"port {self.port} is in use; the firmware expects its broker there")
        self.log = open(self.dir / "mosquitto.log", "ab")
        self.process = subprocess.Popen(
            [tool("mosquitto"), "-c", str(self.dir / "mosquitto.conf")], stdout=self.log, stderr=subprocess.STDOUT
        )
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            with socket.socket() as s:
                if s.connect_ex(("127.0.0.1", self.port)) == 0:
                    return
            time.sleep(0.1)
        raise RuntimeError("mosquitto did not start; see " + str(self.dir / "mosquitto.log"))

    def stop(self) -> None:
        if self.process is not None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
            self.process = None
            self.log.close()

    def close(self) -> None:
        self.stop()
        shutil.rmtree(self.dir, ignore_errors=True)

    # -- a plain MQTT client for the harness, via Mosquitto's own tools --

    def publish(self, topic: str, payload: str, retain: bool = False) -> None:
        cmd = [tool("mosquitto_pub"), "-h", "127.0.0.1", "-p", str(self.port), "-t", topic, "-m", payload, "-q", "1"]
        if retain:
            cmd.append("-r")
        subprocess.run(cmd, check=True, capture_output=True, timeout=15)

    def listen(self, topic: str, seconds: float, count: int | None = None) -> list:
        """Messages on ``topic`` (a filter) seen within ``seconds``: [(topic, payload)]."""
        cmd = [tool("mosquitto_sub"), "-h", "127.0.0.1", "-p", str(self.port), "-t", topic, "-v", "-W", str(max(1, int(seconds)))]
        if count:
            cmd += ["-C", str(count)]
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=seconds + 15)
        out = []
        for line in p.stdout.splitlines():
            t, _, payload = line.partition(" ")
            out.append((t, payload))
        return out

    def state(self, seconds: float = 5) -> dict | None:
        """The retained node state, or None if there is none."""
        msgs = self.listen(f"{PREFIX}/state", seconds, count=1)
        return json.loads(msgs[0][1]) if msgs else None

    def command(self, topic: str, payload: dict | str, wait: float = 4) -> dict | None:
        """Publish like a client would and return the first acknowledgement, if any."""
        sub = subprocess.Popen(
            [tool("mosquitto_sub"), "-h", "127.0.0.1", "-p", str(self.port), "-t", f"{PREFIX}/ack", "-C", "1", "-W", str(int(wait))],
            stdout=subprocess.PIPE, text=True,
        )
        time.sleep(0.4)
        self.publish(f"{PREFIX}/{topic}", payload if isinstance(payload, str) else json.dumps(payload))
        out, _ = sub.communicate(timeout=wait + 10)
        return json.loads(out.strip()) if out.strip() else None


class Tap:
    """Records everything published to a topic filter while it is open:

        with Tap(broker, "esp32/node-01/+") as tap:
            ...
        tap.messages  # [(topic, payload)]
    """

    def __init__(self, broker: Broker, topic: str):
        self.broker, self.topic, self.messages = broker, topic, []

    def __enter__(self):
        self.process = subprocess.Popen(
            [tool("mosquitto_sub"), "-h", "127.0.0.1", "-p", str(self.broker.port), "-t", self.topic, "-v"],
            stdout=subprocess.PIPE, text=True, bufsize=1,
        )
        self.thread = threading.Thread(target=self._read, daemon=True)
        self.thread.start()
        time.sleep(0.5)
        return self

    def _read(self):
        for line in self.process.stdout:
            t, _, payload = line.rstrip("\r\n").partition(" ")
            self.messages.append((t, payload))

    def __exit__(self, *exc):
        time.sleep(0.3)
        self.process.terminate()
        self.process.wait(timeout=5)
        self.thread.join(timeout=3)
        self.process.stdout.close()

    def on(self, suffix: str) -> list:
        return [json.loads(p) for t, p in self.messages if t.endswith("/" + suffix)]


class ClockServer:
    """Answers the node's ``hello`` with the host's time on ``clock``, as an SNTP server
    would. (The firmware can use SNTP instead; under QEMU on Windows the host's UDP port 123
    belongs to the Windows Time service, so the clock comes over MQTT.)"""

    def __init__(self, broker: Broker):
        self.broker = broker
        self.requests = 0
        self.process: subprocess.Popen | None = None
        self.thread: threading.Thread | None = None
        self.stopping = False

    def start(self) -> None:
        self.stopping = False
        self.process = subprocess.Popen(
            [tool("mosquitto_sub"), "-h", "127.0.0.1", "-p", str(self.broker.port), "-t", f"{PREFIX}/hello", "-v"],
            stdout=subprocess.PIPE, text=True, bufsize=1,
        )
        self.thread = threading.Thread(target=self._serve, daemon=True)
        self.thread.start()

    def _serve(self) -> None:
        for _ in self.process.stdout:
            self.requests += 1
            try:
                self.broker.publish(f"{PREFIX}/clock", json.dumps({"epoch_ms": int(time.time() * 1000)}))
            except Exception:
                pass

    def sync_now(self) -> None:
        """Send the time unprompted, as a time source would after the node was stalled."""
        self.broker.publish(f"{PREFIX}/clock", json.dumps({"epoch_ms": int(time.time() * 1000)}))

    def stop(self) -> None:
        self.stopping = True
        if self.process is not None:
            self.process.terminate()
            self.process.wait(timeout=5)
            if self.thread is not None:
                self.thread.join(timeout=3)
            self.process.stdout.close()
            self.process = None


class Node:
    """The firmware in QEMU. ``pause``/``resume`` freeze the whole machine (power stays on
    and the network connection stays open); ``reset`` reboots it; ``kill`` powers it off."""

    def __init__(self, log_dir: Path):
        self.log_dir = log_dir
        self.process: subprocess.Popen | None = None
        self.monitor_port = _free_port()

    def start(self) -> None:
        self.log = open(self.log_dir / "qemu-serial.log", "ab")
        self.process = subprocess.Popen(
            [
                os.environ["OPENMHS_QEMU"], "-M", "esp32", "-nographic",
                "-monitor", f"tcp:127.0.0.1:{self.monitor_port},server,nowait",
                "-drive", f"file={os.environ['OPENMHS_FLASH_IMAGE']},if=mtd,format=raw,snapshot=on",
                "-nic", "user,model=open_eth,id=nic0",
                "-global", "driver=timer.esp32.timg,property=wdt_disable,value=true",
            ],
            stdout=self.log, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL,
        )

    def serial(self) -> str:
        if not self.log.closed:
            self.log.flush()
        return (self.log_dir / "qemu-serial.log").read_text(encoding="utf-8", errors="replace")

    def wait_for_log(self, text: str, timeout: float = 60, since: int = 0) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if text in self.serial()[since:]:
                return
            time.sleep(0.3)
        raise TimeoutError(f"the node never logged {text!r}; serial log:\n{self.serial()[-1500:]}")

    def monitor(self, command: str) -> None:
        with socket.create_connection(("127.0.0.1", self.monitor_port), timeout=5) as s:
            s.recv(4096)
            s.sendall((command + "\n").encode())
            time.sleep(0.3)

    def link(self, up: bool) -> None:
        """Cut or restore the node's network without telling the node (a dropped Wi-Fi link,
        an unplugged cable). Packets both ways are discarded; the node keeps running and keeps
        its clock, unlike ``pause``."""
        self.monitor(f"set_link nic0 {'on' if up else 'off'}")

    def power_cycle(self) -> None:
        """Power off and on: a fresh boot, with the flash as built (no state survives)."""
        mark = len(self.serial())
        self.kill()
        self.start()
        self.wait_for_log("connected to the broker", timeout=90, since=mark)

    def pause(self) -> None:
        self.monitor("stop")

    def resume(self) -> None:
        self.monitor("cont")

    def reset(self) -> None:
        self.monitor("system_reset")

    def kill(self) -> None:
        if self.process is not None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
            self.process = None
            self.log.close()


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Rig:
    """Broker + SNTP + node, started in that order and torn down in reverse."""

    def __init__(self, log_dir: Path | None = None):
        self.log_dir = log_dir or Path(tempfile.mkdtemp(prefix="openmhs-node-"))
        self.broker = Broker()
        self.clock = ClockServer(self.broker)
        self.node = Node(self.log_dir)

    def __enter__(self):
        try:
            self.broker.start()
            self.clock.start()
            self.node.start()
            self.node.wait_for_log("connected to the broker", timeout=90)
        except BaseException:
            self.__exit__()
            raise
        return self

    def __exit__(self, *exc):
        self.node.kill()
        self.clock.stop()
        self.broker.close()
