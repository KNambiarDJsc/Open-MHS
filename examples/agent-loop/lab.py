"""Set up a throwaway lab for the examples: a gateway with simulated devices, a policy,
and an agent token. Simulators only; nothing here talks to real hardware.

The gateway setup uses the Python SDK as the operator. The agent itself only ever gets
an agent token and talks MCP.
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

PSU = "sim.psu-01"
SETPOINT = "power.output.voltage.setpoint"


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


class Lab:
    """A simulated lab. ``with Lab() as lab:`` starts it; leaving the block stops it."""

    def __init__(self, grants: list[dict] | None = None, principal: str = "agent:example") -> None:
        self.grants = grants if grants is not None else default_grants()
        self.principal = principal
        self._tmp = tempfile.TemporaryDirectory(prefix="openmhs-lab-")
        self.home = Path(self._tmp.name)

    def __enter__(self) -> Lab:
        self._proc = subprocess.Popen(
            [binary(), "--home", str(self.home), "serve", "--sim", "--port", "0"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        runtime = self.home / "gateway.json"
        deadline = time.monotonic() + 30
        while True:
            if self._proc.poll() is not None:
                raise SystemExit(f"gateway exited with {self._proc.returncode}")
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
        for g in self.grants:
            self.operator.add_grant(g)
        self.agent_token = self.operator.create_token(self.principal, any_scope=True).token
        return self

    def __exit__(self, *exc: object) -> None:
        if self._proc.poll() is None:
            self._proc.terminate()
            self._proc.wait(timeout=10)
        self._tmp.cleanup()

    def mcp_command(self) -> tuple[list[str], dict[str, str]]:
        """How an MCP host would launch the server for this lab."""
        return [binary(), "mcp"], {"OPENMHS_HOME": str(self.home), "OPENMHS_TOKEN": self.agent_token}


def grant(gid: str, device: str, capability: str, ops: list[str], effect: str = "allow") -> dict:
    return {"id": gid, "principal": "agent:*", "device": device, "capability": capability,
            "operations": ops, "effect": effect}


def default_grants() -> list[dict]:
    """Read anything; write the power supply. Moving the stage is not granted."""
    return [grant("read-all", "*", "*", ["read"]), grant("psu-write", PSU, "*", ["write"])]
