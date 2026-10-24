"""Test harness for the OctoPrint adapter: runs a real OctoPrint with its Virtual Printer
plugin, and a recording proxy that can misbehave on request.

Nothing here is used by the adapter. The harness is the only code in this directory that
sends anything but a GET to OctoPrint, and only to disconnect and reconnect the virtual
printer (the adapter never does; ``test_octoprint.py`` checks that on the wire).

OctoPrint is installed separately (``pip install -r requirements.txt`` in a virtual
environment; it is AGPL-3.0 and nothing of it is bundled here). Point ``OPENMHS_OCTOPRINT``
at its ``octoprint`` executable.
"""

from __future__ import annotations

import http.client
import json
import os
import secrets
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

#: OctoPrint's debug command: the virtual printer closes its own serial connection.
DISCONNECT = "!!DEBUG:action_disconnect"


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def octoprint_executable() -> str | None:
    path = os.environ.get("OPENMHS_OCTOPRINT")
    return path if path and Path(path).exists() else None


#: Never through an environment or system proxy: the API key would travel with it.
_OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))

CONFIG = """\
api:
  key: {key}
server:
  firstRun: false
  onlineCheck:
    enabled: false
  pluginBlacklist:
    enabled: false
  host: 127.0.0.1
  port: {port}
accessControl:
  enabled: true
serial:
  port: VIRTUAL
  baudrate: 115200
  autoconnect: true
plugins:
  virtual_printer:
    enabled: true
  _disabled: [tracking, errortracking, announcements, softwareupdate, pluginmanager]
appearance:
  name: openmhs-experiment-01
"""


class OctoPrintServer:
    """A throwaway OctoPrint on a free loopback port, with a random API key, a scratch
    base directory, the Virtual Printer plugin enabled and its outbound plugins off."""

    def __init__(self, executable: str, keep_logs: Path | None = None):
        self.executable = executable
        self.port = free_port()
        self.key = secrets.token_hex(16).upper()
        self.url = f"http://127.0.0.1:{self.port}"
        self.base = Path(tempfile.mkdtemp(prefix="openmhs-octoprint-"))
        self.keep_logs = keep_logs
        self.process: subprocess.Popen | None = None

    def __enter__(self):
        (self.base / "config.yaml").write_text(CONFIG.format(key=self.key, port=self.port), encoding="utf-8")
        self.log = open(self.base / "serve.log", "wb")
        self.process = subprocess.Popen(
            [self.executable, "serve", "--basedir", str(self.base), "--host", "127.0.0.1", "--port", str(self.port)],
            stdout=self.log,
            stderr=subprocess.STDOUT,
        )
        try:
            self.wait_until("Operational", timeout=90)
            self.wait_for_temperatures(timeout=60)
        except BaseException:
            self.__exit__()
            raise
        return self

    def __exit__(self, *exc):
        if self.process is not None:
            self.process.terminate()
            try:
                self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.process.kill()
        self.log.close()
        if self.keep_logs is not None:
            self.keep_logs.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(self.base / "serve.log", self.keep_logs / "octoprint-serve.log")
        shutil.rmtree(self.base, ignore_errors=True)

    # -- direct calls (the harness, not the adapter) --

    def request(self, method: str, path: str, body: dict | None = None, key: str | None = None):
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(
            self.url + path,
            data=data,
            method=method,
            headers={"X-Api-Key": self.key if key is None else key, "Content-Type": "application/json"},
        )
        try:
            with _OPENER.open(req, timeout=10) as r:
                raw, status = r.read(), r.status
        except urllib.error.HTTPError as e:
            raw, status = e.read(), e.code
        try:
            return status, json.loads(raw) if raw else None
        except ValueError:
            return status, raw

    def connection_state(self) -> str:
        return self.request("GET", "/api/connection")[1]["current"]["state"]

    def wait_until(self, state: str, timeout: float = 30) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                status, body = self.request("GET", "/api/connection")
                if status == 200 and body["current"]["state"] == state:
                    return
            except (OSError, KeyError, TypeError, http.client.HTTPException):
                pass
            time.sleep(0.5)
        raise TimeoutError(f"OctoPrint did not reach {state!r} within {timeout:g} s; see {self.base / 'serve.log'}")

    def wait_for_temperatures(self, timeout: float = 30) -> None:
        """OctoPrint reports null temperatures until the printer answers its first poll."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            status, body = self.request("GET", "/api/printer")
            if status == 200 and body["temperature"].get("tool0", {}).get("actual") is not None:
                return
            time.sleep(0.3)
        raise TimeoutError("OctoPrint reported no temperature")

    def disconnect_printer(self) -> None:
        """Make the virtual printer drop its connection, as a real one would if unplugged."""
        status, _ = self.request("POST", "/api/printer/command", {"command": DISCONNECT})
        assert status == 204, status
        self.wait_until("Closed", timeout=15)

    def reconnect_printer(self) -> None:
        status, _ = self.request("POST", "/api/connection", {"command": "connect", "port": "VIRTUAL", "baudrate": 115200})
        assert status == 204, status
        self.wait_until("Operational", timeout=30)
        self.wait_for_temperatures()


class RecordingProxy:
    """A loopback HTTP proxy between the adapter and OctoPrint. It records every request
    (method, path) and can answer wrongly. ``mode`` is one of:

    ``("pass",)``              forward to OctoPrint
    ``("delay", seconds)``     wait, then forward (the adapter should time out)
    ``("drop",)``              close the connection without answering
    ``("status", code)``       answer with that status and an empty JSON object
    ``("body", bytes)``        answer 200 with exactly these bytes
    ``("redirect", url)``      answer 302 to ``url``
    """

    def __init__(self, upstream: str):
        host, port = upstream.removeprefix("http://").split(":")
        self.upstream = (host, int(port))
        self.mode: tuple = ("pass",)
        self.requests: list = []  # (method, path, query)
        self.lock = threading.Lock()
        proxy = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args):
                pass

            def _handle(self):
                try:
                    self._forward()
                except (ConnectionError, TimeoutError, OSError):
                    self.close_connection = True  # the client gave up (a timeout test does this on purpose)

            def _forward(self):
                path, _, query = self.path.partition("?")
                length = int(self.headers.get("Content-Length") or 0)
                body = self.rfile.read(length) if length else None
                with proxy.lock:
                    proxy.requests.append((self.command, path, query))
                    mode = proxy.mode
                if mode[0] == "drop":
                    self.close_connection = True
                    self.connection.shutdown(socket.SHUT_RDWR)
                    self.connection.close()
                    return
                if mode[0] == "delay":
                    time.sleep(mode[1])
                if mode[0] in ("status", "body", "redirect"):
                    status = {"status": mode[1], "body": 200, "redirect": 302}[mode[0]]
                    payload = mode[1] if mode[0] == "body" else b"{}"
                    self.send_response(status)
                    if mode[0] == "redirect":
                        self.send_header("Location", mode[1])
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                    return
                conn = http.client.HTTPConnection(*proxy.upstream, timeout=15)
                headers = {k: v for k, v in self.headers.items() if k.lower() not in ("host", "connection")}
                try:
                    conn.request(self.command, self.path, body=body, headers=headers)
                    response = conn.getresponse()
                    payload = response.read()
                finally:
                    conn.close()
                self.send_response(response.status)
                self.send_header("Content-Type", response.getheader("Content-Type", "application/json"))
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            do_GET = do_POST = do_PUT = do_DELETE = do_PATCH = do_HEAD = do_OPTIONS = _handle

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *exc):
        self.server.shutdown()
        self.server.server_close()

    def reset(self) -> None:
        with self.lock:
            self.requests.clear()

    def methods(self) -> set:
        with self.lock:
            return {m for m, _, _ in self.requests}

    def count(self) -> int:
        with self.lock:
            return len(self.requests)
