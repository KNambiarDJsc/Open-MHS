"""Start the whole simulated lab for this example: the simulated instrument, a gateway that
reaches it through the OpenMHS Modbus adapter, a read-only policy, and an agent credential.

Everything runs on localhost with no hardware, no network access and no credentials. The
gateway is a throwaway in a temporary directory.
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
from typing import Any

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(REPO / "python"))
sys.path.insert(0, str(REPO / "examples" / "agent-loop"))

import build_profile  # noqa: E402
import register_map as rm  # noqa: E402
import simulator  # noqa: E402
from mcp_stdio import McpStdioClient  # noqa: E402
from openmhs import Client  # noqa: E402

DEVICE = rm.DEVICE_ID
AGENT = "agent:battery-example"

#: What the agent may read. Nothing else is granted, so the control points have no grant.
READ_PATTERNS = ["instrument.*", "telemetry.*", "session.*", "session_*"]
CONTROL_POINTS = ["limit.voltage_max", "test.start", "test.stop"]


def binary() -> str:
    """``$OPENMHS_BIN``, else this repo's debug build, else ``openmhs`` on PATH."""
    if os.environ.get("OPENMHS_BIN"):
        return os.environ["OPENMHS_BIN"]
    exe = "openmhs.exe" if sys.platform == "win32" else "openmhs"
    local = REPO / "target" / "debug" / exe
    if local.exists():
        return str(local)
    found = shutil.which("openmhs")
    if found:
        return found
    raise SystemExit("openmhs binary not found: run `cargo build -p openmhs-cli` or set OPENMHS_BIN")


def read_only_grants() -> list[dict[str, Any]]:
    """Allow reads of the data capabilities; deny every write and invoke, whatever else says."""
    grants = [
        {"id": f"agent-read-{i}", "principal": "agent:*", "device": DEVICE, "capability": pattern,
         "operations": ["read"], "effect": "allow"}
        for i, pattern in enumerate(READ_PATTERNS)
    ]
    grants.append({"id": "agent-no-control", "principal": "agent:*", "device": "*", "capability": "*",
                   "operations": ["write", "invoke"], "effect": "deny"})
    return grants


def read_only_scopes() -> list[dict[str, Any]]:
    return [{"device": DEVICE, "capability": p, "operations": ["read"]} for p in READ_PATTERNS]


class BatteryLab:
    """``with BatteryLab("session") as lab:`` starts everything; leaving the block stops it."""

    def __init__(self, fixture: str = "normal", health_interval_ms: int = 300) -> None:
        self.fixture = fixture
        self.health_interval_ms = health_interval_ms
        self._tmp = tempfile.TemporaryDirectory(prefix="openmhs-battery-")
        self.home = Path(self._tmp.name) / "home"
        self.home.mkdir()
        self.sim: simulator.BatterySimulator | None = None
        self.proc: subprocess.Popen[bytes] | None = None
        self.operator: Client | None = None
        self.agent_token = ""
        self.profile_path = Path(self._tmp.name) / "battery-test-instrument.profile.json"

    def __enter__(self) -> BatteryLab:
        self.sim = simulator.BatterySimulator(simulator.load_fixture(self.fixture))
        port = self.sim.start()
        self.profile_path.write_text(build_profile.render(f"127.0.0.1:{port}"), encoding="utf-8")
        self.proc = subprocess.Popen(
            [binary(), "--home", str(self.home), "serve", "--profile", str(self.profile_path),
             "--port", "0", "--health-interval-ms", str(self.health_interval_ms)],
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        runtime = self.home / "gateway.json"
        deadline = time.monotonic() + 30
        while True:
            if self.proc.poll() is not None:
                err = self.proc.stderr.read().decode("utf-8", "replace") if self.proc.stderr else ""
                raise SystemExit(f"gateway exited with {self.proc.returncode}: {err[-500:]}")
            if runtime.exists():
                try:
                    json.loads(runtime.read_text(encoding="utf-8"))
                    break
                except ValueError:
                    pass
            if time.monotonic() > deadline:
                raise SystemExit("gateway did not start")
            time.sleep(0.05)
        self.operator = Client.from_home(self.home)
        for grant in read_only_grants():
            self.operator.add_grant(grant)
        self.agent_token = self.operator.create_token(AGENT, scopes=read_only_scopes()).token
        return self

    def __exit__(self, *exc: object) -> None:
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        if self.proc and self.proc.stderr:
            self.proc.stderr.close()
        if self.sim:
            self.sim.stop()
        self._tmp.cleanup()

    def mcp(self, token: str | None = None) -> McpStdioClient:
        """An MCP session as the agent, the way an MCP host would open one."""
        return McpStdioClient([binary(), "mcp"], env={
            "OPENMHS_HOME": str(self.home), "OPENMHS_TOKEN": token or self.agent_token})

    def unscoped_agent_token(self, principal: str = "agent:unscoped") -> str:
        """A credential with no scope limit, so only the policy grants decide what it may do.
        It shows that the permission layer denies writes even when the credential would not."""
        assert self.operator is not None
        return self.operator.create_token(principal, any_scope=True).token

    def agent(self) -> Client:
        """The same agent credential through the Python SDK (for tests)."""
        return Client.from_home(self.home, token=self.agent_token)
