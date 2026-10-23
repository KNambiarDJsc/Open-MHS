"""A minimal MCP client over stdio, standard library only.

Enough of the Model Context Protocol (2025-11-25) to drive `openmhs mcp`: the
initialize handshake, tools/list and tools/call, newline-delimited JSON-RPC.
It is what an MCP host such as Claude Code does, minus the model.
"""

from __future__ import annotations

import itertools
import json
import os
import queue
import subprocess
import threading
from typing import Any

PROTOCOL_VERSION = "2025-11-25"


class McpError(Exception):
    """A JSON-RPC error from the server (a protocol failure, not a tool refusal)."""


class ToolResult:
    """What a tool call returned. A refusal is a result with ``is_error`` set."""

    def __init__(self, raw: dict[str, Any]) -> None:
        self.raw = raw
        self.is_error: bool = bool(raw.get("isError"))
        self.structured: Any = raw.get("structuredContent")
        self.text = "\n".join(
            c.get("text", "") for c in raw.get("content", []) if c.get("type") == "text"
        )

    def __repr__(self) -> str:
        return f"ToolResult(is_error={self.is_error}, structured={self.structured!r})"


class McpStdioClient:
    """Spawn an MCP server and talk to it. Use as a context manager."""

    def __init__(self, command: list[str], env: dict[str, str] | None = None) -> None:
        self._command = command
        self._env = {**os.environ, **(env or {})}
        self._ids = itertools.count(1)
        self._responses: queue.Queue[dict[str, Any]] = queue.Queue()
        self.server_info: dict[str, Any] = {}
        self.instructions: str = ""
        self.stderr_lines: list[str] = []

    def __enter__(self) -> McpStdioClient:
        self._proc = subprocess.Popen(
            self._command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=self._env,
        )
        threading.Thread(target=self._read_stdout, daemon=True).start()
        threading.Thread(target=self._read_stderr, daemon=True).start()
        init = self.request(
            "initialize",
            {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "openmhs-example-agent", "version": "0.1"},
            },
        )
        self.server_info = init.get("serverInfo", {})
        self.instructions = init.get("instructions", "")
        self.notify("notifications/initialized")
        return self

    def __exit__(self, *exc: object) -> None:
        try:
            if self._proc.stdin:
                self._proc.stdin.close()  # EOF ends the session cleanly
            self._proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self._proc.kill()
            self._proc.wait()
        for pipe in (self._proc.stdout, self._proc.stderr):
            if pipe:
                pipe.close()

    # ---------------------------------------------------------------- protocol

    def _send(self, msg: dict[str, Any]) -> None:
        assert self._proc.stdin is not None
        self._proc.stdin.write((json.dumps(msg) + "\n").encode("utf-8"))
        self._proc.stdin.flush()

    def _read_stdout(self) -> None:
        assert self._proc.stdout is not None
        for line in self._proc.stdout:
            line = line.strip()
            if not line:
                continue
            msg = json.loads(line)
            if "id" in msg and ("result" in msg or "error" in msg):
                self._responses.put(msg)
            elif "id" in msg and "method" in msg:
                # A server-to-client request (e.g. ping). Answer so the server never waits.
                self._send({"jsonrpc": "2.0", "id": msg["id"], "result": {}})
        self._responses.put({"id": None, "error": {"message": "server closed the connection"}})

    def _read_stderr(self) -> None:
        assert self._proc.stderr is not None
        for line in self._proc.stderr:
            self.stderr_lines.append(line.decode("utf-8", "replace").rstrip())

    def notify(self, method: str, params: dict[str, Any] | None = None) -> None:
        msg: dict[str, Any] = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        self._send(msg)

    def request(self, method: str, params: dict[str, Any], timeout: float = 120) -> Any:
        rid = next(self._ids)
        self._send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        while True:
            try:
                msg = self._responses.get(timeout=timeout)
            except queue.Empty:
                raise McpError(f"{method}: no answer within {timeout} s") from None
            if msg.get("id") is None and "error" in msg:
                detail = "\n".join(self.stderr_lines[-5:])
                raise McpError(f"{msg['error']['message']}\n{detail}".strip())
            if msg.get("id") != rid:
                continue
            if "error" in msg:
                raise McpError(f"{method}: {msg['error'].get('message')}")
            return msg["result"]

    # ---------------------------------------------------------------- tools

    def list_tools(self) -> list[dict[str, Any]]:
        return list(self.request("tools/list", {})["tools"])

    def call(self, name: str, arguments: dict[str, Any] | None = None) -> ToolResult:
        return ToolResult(self.request("tools/call", {"name": name, "arguments": arguments or {}}))
