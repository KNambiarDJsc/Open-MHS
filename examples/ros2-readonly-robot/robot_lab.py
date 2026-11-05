"""Start this example's whole setup on one Linux host with ROS 2 Jazzy.

``RobotLab`` starts the simulated robot, a throwaway gateway that hosts the existing ROS 2 adapter, a
read-only policy, and an agent credential. Everything shares one clock, on purpose: the gateway judges
freshness from message stamps, so a gateway on another clock would measure that clock, not the robot
(`DESIGN.md`, section 4). On Windows, run all of this inside WSL.

Credentials go through the SDK and the MCP child's environment, never a command line.
"""

from __future__ import annotations

import json
import os
import queue
import random
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(REPO / "python"))
sys.path.insert(0, str(REPO / "examples" / "agent-loop"))

from mcp_stdio import McpStdioClient  # noqa: E402
from openmhs import Client  # noqa: E402

DEVICE = "ros2.sim-robot-01"
AGENT = "agent:robot-observer"
ROS_SETUP = "/opt/ros/jazzy/setup.bash"
#: Exact names, so that a capability added later is not granted by accident.
AGENT_READS = [
    "joint.shoulder.position", "joint.elbow.position", "joint.wrist.position",
    "base.frame", "base.child_frame", "base.position.x", "base.position.y",
    "base.orientation.z", "base.orientation.w",
    "battery.voltage", "battery.charge", "battery.temperature",
    "diagnostics.driver.level",
]
UNGRANTED = "diagnostics.driver.message"
CONTROL_SURFACES = ["/cmd_vel", "/joint_trajectory", "/robot/set_mode", "/robot/run_task", "parameter:max_speed"]


def ros_available() -> bool:
    return Path(ROS_SETUP).exists()


def binary() -> str:
    """``$OPENMHS_BIN``, else this repo's debug build, else ``openmhs`` on PATH."""
    if os.environ.get("OPENMHS_BIN"):
        return os.environ["OPENMHS_BIN"]
    target = Path(os.environ.get("CARGO_TARGET_DIR", REPO / "target"))
    local = target / "debug" / "openmhs"
    if local.exists():
        return str(local)
    found = shutil.which("openmhs")
    if found:
        return found
    raise SystemExit("openmhs binary not found: run `cargo build -p openmhs-cli` or set OPENMHS_BIN")


def in_ros(program: list[str]) -> list[str]:
    """``program``, in a shell that has sourced ROS 2. No environment passes through a command line."""
    return ["bash", "-c", f"source {ROS_SETUP} && exec {' '.join(shlex.quote(a) for a in program)}"]


def command_attempt(client: Any, capability: str, operation: str, value: Any = None) -> dict[str, Any]:
    """Try a command through the SDK and say how it ended. A refusal may be a receipt in state DENIED, or
    an error before any receipt exists. A request the gateway rejects as malformed proves nothing about
    permissions, so that raises rather than counting as a refusal."""
    request: dict[str, Any] = {"target": {"device": DEVICE, "capability": capability}, "operation": operation,
                               "idempotency_key": f"attempt-{capability}-{operation}-{time.monotonic_ns()}"}
    if operation == "write":
        request["value"] = value
    try:
        receipt = client.submit(request)
    except Exception as e:  # noqa: BLE001 - the refusal is what is being observed
        code = getattr(e, "code", type(e).__name__)
        if code == "command.schema":
            raise AssertionError(f"malformed attempt, which proves nothing: {request}: {e}") from e
        return {"capability": capability, "operation": operation, "refused": True, "state": None, "code": code}
    state = str(getattr(receipt.state, "value", receipt.state))
    codes = [str(e.code) for e in receipt.policy.errors]
    return {"capability": capability, "operation": operation, "refused": state == "DENIED", "state": state,
            "code": codes[0] if codes else None, "dispatched": receipt.adapter_result is not None}


def read_only_grants() -> list[dict[str, Any]]:
    """Allow reads of exactly the named capabilities; deny every write and invoke, whatever else says."""
    grants: list[dict[str, Any]] = [
        {"id": f"agent-read-{i}", "principal": "agent:*", "device": DEVICE, "capability": name,
         "operations": ["read"], "effect": "allow"}
        for i, name in enumerate(AGENT_READS)
    ]
    grants.append({"id": "agent-no-control", "principal": "agent:*", "device": "*", "capability": "*",
                   "operations": ["write", "invoke"], "effect": "deny"})
    return grants


def read_only_scopes() -> list[dict[str, Any]]:
    return [{"device": DEVICE, "capability": name, "operations": ["read"]} for name in AGENT_READS]


class SimProcess:
    """The simulated robot process: commands in, events out, tripwires kept across restarts."""

    def __init__(self, env: dict[str, str], stderr_path: Path) -> None:
        self.env = env
        self.stderr_path = stderr_path
        self.proc: subprocess.Popen[str] | None = None
        self.events: queue.Queue[dict[str, Any]] = queue.Queue()
        self.tripwires: list[str] = []
        self._ids = 0
        self.last_ready = 0.0

    def start(self, timeout: float = 30) -> None:
        self.events = queue.Queue()
        with open(self.stderr_path, "ab") as stderr:
            self.proc = subprocess.Popen(
                in_ros(["python3", "-u", str(HERE / "sim_robot.py")]), env=self.env,
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr,
                text=True, encoding="utf-8", errors="replace", bufsize=1)
        threading.Thread(target=self._pump, args=(self.proc, self.events), daemon=True).start()
        self.last_ready = self.wait_for("ready", timeout)["_received"]

    def _pump(self, proc: subprocess.Popen[str], events: queue.Queue[dict[str, Any]]) -> None:
        assert proc.stdout is not None
        for line in proc.stdout:
            try:
                event = json.loads(line)
            except ValueError:
                continue
            event["_received"] = time.monotonic()
            if event.get("event") == "tripwire":
                self.tripwires.append(event["surface"])
            events.put(event)
        proc.stdout.close()

    def wait_for(self, name: str, timeout: float = 10, ident: int | None = None) -> dict[str, Any]:
        deadline = time.monotonic() + timeout
        while True:
            left = deadline - time.monotonic()
            if left <= 0:
                raise TimeoutError(f"simulated robot: no {name} within {timeout} s. Its stderr: {self.stderr_tail()}")
            try:
                event = self.events.get(timeout=left)
            except queue.Empty:
                continue
            if event.get("event") == "error":
                raise RuntimeError(event.get("message"))
            if event.get("event") == name and (ident is None or event.get("id") == ident):
                return event

    def stderr_tail(self, size: int = 1500) -> str:
        try:
            return self.stderr_path.read_text(encoding="utf-8", errors="replace")[-size:] or "(empty)"
        except OSError:
            return "(none)"

    def send(self, cmd: str, **fields: Any) -> float:
        """Send a command and wait until it has taken effect. Returns the monotonic time of the ack."""
        assert self.proc is not None and self.proc.stdin is not None
        self._ids += 1
        self.proc.stdin.write(json.dumps({"cmd": cmd, "id": self._ids, **fields}) + "\n")
        self.proc.stdin.flush()
        return self.wait_for("ack", 10, self._ids)["_received"]

    def kill(self) -> float:
        """SIGKILL the robot, as a crash would. Returns the monotonic time it was gone."""
        assert self.proc is not None and self.proc.stdin is not None
        self.proc.send_signal(signal.SIGKILL)  # bash exec'd python3, so this is the robot itself
        self.proc.wait(timeout=10)
        gone = time.monotonic()
        self.proc.stdin.close()
        return gone

    def stop(self) -> None:
        if self.proc is None:
            return
        if self.proc.stdin and not self.proc.stdin.closed:
            try:
                self.proc.stdin.close()  # end of input is how the robot knows to exit
            except OSError:
                pass
        if self.proc.poll() is None:
            try:
                self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self.proc = None


class RobotLab:
    """``with RobotLab() as lab:`` starts everything; leaving the block stops it."""

    def __init__(self, health_interval_ms: int = 300, manifest_change: Any = None,
                 fault_from_start: str | None = None) -> None:
        self.health_interval_ms = health_interval_ms
        self.manifest_change = manifest_change
        self.fault_from_start = fault_from_start
        # A domain of its own, so that parallel runs and anything else on this host stay apart.
        self.domain = str(random.randint(100, 200))
        self.ros_env = {
            "ROS_DOMAIN_ID": self.domain,
            "ROS_AUTOMATIC_DISCOVERY_RANGE": "LOCALHOST",
        }
        self._tmp = tempfile.TemporaryDirectory(prefix="openmhs-robot-")
        # Its own log directory: a fixed one in /tmp stops the next user on this host from starting ROS 2.
        self.ros_env["ROS_LOG_DIR"] = str(Path(self._tmp.name) / "ros-log")
        self.env = {**os.environ, **self.ros_env}
        self.home = Path(self._tmp.name) / "home"
        self.home.mkdir()
        self.sim = SimProcess(self.env, Path(self._tmp.name) / "sim_robot.stderr")
        self.proc: subprocess.Popen[bytes] | None = None
        self.gateway_log = Path(self._tmp.name) / "gateway.log"
        self._log_file: Any = None
        self.operator: Client | None = None
        self.agent_token = ""
        self.leftover: list[str] = []

    def manifest(self) -> dict[str, Any]:
        """The committed manifest, on this run's own ROS domain, with its paths made absolute because
        this copy lives in the run's temporary directory."""
        m = json.loads((HERE / "robot.manifest.json").read_text(encoding="utf-8"))
        m["runtime"]["env"] = dict(self.ros_env)
        m["runtime"]["command"] = [a.replace("{manifest_dir}", str(HERE)) for a in m["runtime"]["command"]]
        if self.manifest_change:
            self.manifest_change(m)
        return m

    def __enter__(self) -> RobotLab:
        try:
            self.sim.start()
            if self.fault_from_start:
                self.sim.send("fault", name=self.fault_from_start)
            path = Path(self._tmp.name) / "robot.manifest.json"
            path.write_text(json.dumps(self.manifest(), indent=2), encoding="utf-8")
            self._log_file = open(self.gateway_log, "wb")
            # Unsigned on purpose: this manifest is generated per run for a throwaway gateway.
            self.proc = subprocess.Popen(
                [binary(), "--home", str(self.home), "serve", "--adapter-manifest", str(path),
                 "--allow-unsigned-adapters", "--port", "0",
                 "--health-interval-ms", str(self.health_interval_ms)],
                stdout=self._log_file, stderr=subprocess.STDOUT, env=self.env)
            self._wait_for_gateway()
            self.operator = Client.from_home(self.home)
            for grant in read_only_grants():
                self.operator.add_grant(grant)
            self.agent_token = self.operator.create_token(AGENT, scopes=read_only_scopes()).token
        except BaseException:
            self.__exit__()
            raise
        return self

    def _wait_for_gateway(self, timeout: float = 90) -> None:
        runtime = self.home / "gateway.json"
        deadline = time.monotonic() + timeout
        while True:
            assert self.proc is not None
            if self.proc.poll() is not None:
                raise RuntimeError(f"gateway exited with {self.proc.returncode}: {self.log_tail()}")
            if runtime.exists():
                try:
                    json.loads(runtime.read_text(encoding="utf-8"))
                    return
                except ValueError:
                    pass
            if time.monotonic() > deadline:
                raise RuntimeError(f"gateway did not start: {self.log_tail()}")
            time.sleep(0.05)

    def log_tail(self, size: int = 2000) -> str:
        try:
            return self.gateway_log.read_text(encoding="utf-8", errors="replace")[-size:]
        except OSError:
            return ""

    def __exit__(self, *exc: object) -> None:
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        if self._log_file:
            self._log_file.close()
        self.sim.stop()
        self._reap()
        self._tmp.cleanup()

    def _reap(self, grace_s: float = 8.0) -> None:
        """The adapter exits when its stdin closes. Allow it that long, then kill what is left.
        ``self.leftover`` is how a test sees that cleanup needed force."""
        deadline = time.monotonic() + grace_s
        self.leftover = self.processes()
        while self.leftover and time.monotonic() < deadline:
            time.sleep(0.25)
            self.leftover = self.processes()
        for pid in self.leftover:
            try:
                os.kill(int(pid), signal.SIGKILL)
            except OSError:
                pass

    def processes(self) -> list[str]:
        """Adapter and simulated robot processes on this run's ROS domain."""
        found = []
        for entry in Path("/proc").iterdir():
            if not entry.name.isdigit():
                continue
            try:
                cmdline = (entry / "cmdline").read_bytes()
                environ = (entry / "environ").read_bytes()
            except OSError:
                continue
            if (b"ros2_adapter.py" in cmdline or b"sim_robot.py" in cmdline) and \
                    f"ROS_DOMAIN_ID={self.domain}".encode() in environ.split(b"\0"):
                found.append(entry.name)
        return found

    # ------------------------------------------------------------------- clients

    def mcp(self, token: str | None = None) -> McpStdioClient:
        """An MCP session as the agent, the way an MCP host would open one."""
        return McpStdioClient([binary(), "mcp"], env={
            "OPENMHS_HOME": str(self.home), "OPENMHS_TOKEN": token or self.agent_token})

    def agent_client(self) -> Client:
        return Client.from_home(self.home, token=self.agent_token)

    def client_for(self, token: str) -> Client:
        return Client.from_home(self.home, token=token)

    @staticmethod
    def read_plain(client: Client, capability: str) -> dict[str, Any]:
        """One read as plain data: ``value`` and friends, or ``error``. For the evaluation's records."""
        try:
            r = client.read(DEVICE, capability)
        except Exception as e:  # noqa: BLE001 - a failed read is an observation
            return {"error": getattr(e, "code", type(e).__name__), "message": str(e)[:240]}
        raw = r.state.value
        text = str(r.state.timestamp)
        h, m, s = text.split("T")[1].rstrip("Z").split(":")
        return {"value": getattr(raw, "value", raw), "unit": getattr(raw, "unit", None), "timestamp": text,
                "seconds": int(h) * 3600 + int(m) * 60 + float(s), "age_ms": r.age_ms, "usable": r.usable,
                "quality": str(getattr(r.state.quality, "value", r.state.quality)),
                "source": str(getattr(r.state.source, "value", r.state.source))}

    def unscoped_agent_token(self, principal: str = "agent:unscoped") -> str:
        """No scope limit on the credential, so only the policy grants decide."""
        assert self.operator is not None
        return self.operator.create_token(principal, any_scope=True).token

    def cli(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run([binary(), "--home", str(self.home), *args], capture_output=True,
                              text=True, encoding="utf-8", errors="replace", timeout=60, check=False)

    def ros(self, *args: str, timeout: float = 60) -> dict[str, Any]:
        """Run ``ros_probe.py`` on this run's ROS domain."""
        result = subprocess.run(in_ros(["python3", "-u", str(HERE / "ros_probe.py"), *args]), env=self.env,
                                capture_output=True, text=True, timeout=timeout, check=False)
        if result.returncode != 0:
            raise RuntimeError(f"ros_probe {args} failed: {result.stderr[-800:]}")
        return json.loads(result.stdout.strip().splitlines()[-1])

    def wait_readable(self, capability: str, timeout: float = 30) -> float:
        """Poll until the capability reads and is usable. Returns the monotonic time it did."""
        client = self.agent_client()
        deadline = time.monotonic() + timeout
        last: Any = None
        while time.monotonic() < deadline:
            try:
                r = client.read(DEVICE, capability)
                if r.usable:
                    return time.monotonic()
                last = f"not usable, age {r.age_ms} ms"
            except Exception as e:  # noqa: BLE001 - the point is to keep polling through failures
                last = e
            time.sleep(0.02)
        raise TimeoutError(f"{capability} not readable within {timeout} s: {last}")
