"""Test support: run a real gateway binary (`openmhs serve --sim`) in a throwaway home.

The binary is ``$OPENMHS_BIN``, else ``target/debug/openmhs`` in this repository
(``cargo build -p openmhs-cli`` first). Tests that need it fail loudly if it is missing
rather than skipping, so a green run always means the gateway was exercised.
"""

from __future__ import annotations

import json
import os
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
AXIS = "sim.axis-01"


def binary() -> Path:
    env = os.environ.get("OPENMHS_BIN")
    if env:
        return Path(env)
    exe = "openmhs.exe" if sys.platform == "win32" else "openmhs"
    path = REPO / "target" / "debug" / exe
    if not path.exists():
        raise RuntimeError(f"{path} not found: run `cargo build -p openmhs-cli` or set OPENMHS_BIN")
    return path


class Gateway:
    """A gateway process with the virtual lab. Use as a context manager."""

    def __init__(self) -> None:
        self._dir = tempfile.TemporaryDirectory(prefix="openmhs-test-")
        self.home = Path(self._dir.name)
        self.proc: subprocess.Popen[bytes] | None = None
        self.url = ""
        self.operator_token = ""

    def __enter__(self) -> Gateway:
        self.proc = subprocess.Popen(
            [str(binary()), "--home", str(self.home), "serve", "--sim", "--port", "0"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
        )
        runtime = self.home / "gateway.json"
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                err = self.proc.stderr.read().decode("utf-8", "replace") if self.proc.stderr else ""
                raise RuntimeError(f"gateway exited with {self.proc.returncode}: {err}")
            if runtime.exists():
                try:
                    info = json.loads(runtime.read_text(encoding="utf-8"))
                    self.url, self.operator_token = info["url"], info["token"]
                    break
                except (ValueError, KeyError):
                    pass  # half-written; try again
            time.sleep(0.05)
        else:
            self.__exit__(None, None, None)
            raise RuntimeError("gateway did not start within 30 s")
        return self

    def __exit__(self, *exc: object) -> None:
        if self.proc is not None and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        if self.proc is not None and self.proc.stderr:
            self.proc.stderr.close()
        self._dir.cleanup()

    def operator(self) -> Client:
        return Client(self.url, self.operator_token)

    def agent(self, principal: str = "agent:test", grants: list[dict] | None = None) -> tuple[Client, str]:
        """Add ``grants`` (default: read everything, write the supply) and return a client
        for a new agent token, plus the token."""
        op = self.operator()
        for g in grants if grants is not None else default_grants():
            op.add_grant(g)
        token = op.create_token(principal, any_scope=True).token
        return Client(self.url, token), token


def grant(gid: str, device: str, capability: str, ops: list[str], effect: str = "allow") -> dict:
    return {
        "id": gid,
        "principal": "agent:*",
        "device": device,
        "capability": capability,
        "operations": ops,
        "effect": effect,
    }


def default_grants() -> list[dict]:
    return [
        grant("read-all", "*", "*", ["read"]),
        grant("psu-write", PSU, "*", ["write"]),
    ]
