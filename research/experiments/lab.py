"""Shared utilities for the LabThings / OpenFlexure experiments (simulators only).

* ``FaultProxy``  an HTTP proxy that can drop, hang, reset, delay or freeze responses, so the
                  adapter's view of a LabThings server can fail in ways the server itself does not.
* ``Gateway``     a real OpenMHS gateway process serving one imported Thing.
* ``truth``       ground truth, read straight from the LabThings server, bypassing the proxy.
"""

from __future__ import annotations

import http.client
import json
import os
import re
import socket
import subprocess
import sys
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Optional

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "python"))
sys.path.insert(0, str(REPO / "adapters" / "python" / "wot"))

import import_td  # noqa: E402
import td_import  # noqa: E402
from openmhs import Client  # noqa: E402

OPENMHS = os.environ.get("OPENMHS_BIN") or str(REPO / "target" / "debug" / "openmhs.exe")
WORK = Path(os.environ.get("OPENMHS_RESEARCH_WORK", r"D:\openmhs-research\work"))
BENCH = "http://127.0.0.1:5100"


# ------------------------------------------------------------------ ground truth

def http_json(method: str, url: str, body: Any = None, timeout: float = 5) -> tuple[int, Any]:
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:  # noqa: S310
            raw = r.read()
            return r.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            return e.code, json.loads(raw)
        except ValueError:
            return e.code, raw.decode("utf-8", "replace")


def truth(base: str = BENCH) -> dict:
    """What the LabThings server itself says, with no proxy in between."""
    _, pos = http_json("GET", f"{base}/bench/position")
    _, jobs = http_json("GET", f"{base}/bench/jobs_started")
    _, busy = http_json("GET", f"{base}/bench/busy")
    _, invs = http_json("GET", f"{base}/action_invocations/")
    return {"position": pos, "jobs_started": jobs, "busy": busy,
            "invocations": [{"id": i["id"], "action": i["action"].rsplit("/", 1)[-1], "status": i["status"]}
                            for i in (invs or [])]}


def settle(base: str = BENCH, timeout: float = 30) -> None:
    """Wait until the bench is idle (no running job)."""
    end = time.time() + timeout
    while time.time() < end:
        if truth(base)["busy"] is False:
            return
        time.sleep(0.2)
    raise TimeoutError("the bench did not become idle")


def direct_move(target: int, seconds: float = 0.2, base: str = BENCH) -> None:
    """Move the bench directly (not through OpenMHS), to set up a known starting state."""
    _, inv = http_json("POST", f"{base}/bench/move_to", {"target": target, "seconds": seconds})
    end = time.time() + 30
    while time.time() < end:
        _, st = http_json("GET", inv["href"])
        if st["status"] in ("completed", "error", "cancelled"):
            return
        time.sleep(0.1)


# ------------------------------------------------------------------ fault proxy

class FaultProxy:
    """Forwards HTTP to an upstream and misbehaves on demand.

    Rules are tried in order; the first whose ``match`` regex fits ``"METHOD /path"`` and that
    still has uses left applies. Actions:

    ``drop_response``  forward the request (so the server acts), then never answer;
    ``hang``           do not forward and never answer;
    ``reset``          close the connection at once;
    ``status:NNN``     answer NNN without forwarding;
    ``delay:S``        forward after S seconds;
    ``freeze``         answer GETs from a snapshot taken when the rule was added.
    """

    def __init__(self, upstream: str = BENCH) -> None:
        self.up = re.match(r"https?://([^:/]+):(\d+)", upstream).groups()
        self.rules: list[dict] = []
        self.log: list[tuple[float, str, str]] = []
        self._lock = threading.Lock()
        proxy = self

        class H(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *a: Any) -> None:
                pass

            def _serve(self) -> None:
                proxy._handle(self)

            do_GET = do_POST = do_DELETE = _serve

        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), H)
        self.httpd.daemon_threads = True
        self.port = self.httpd.server_address[1]
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()

    @property
    def base(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def add(self, match: str, action: str, times: Optional[int] = None) -> dict:
        rule: dict = {"match": re.compile(match), "action": action, "times": times, "hits": 0}
        if action == "freeze":
            m = re.match(r"GET (\S+)", match.replace("\\", ""))
            path = m.group(1) if m else "/"
            conn = http.client.HTTPConnection(*self.up, timeout=5)
            conn.request("GET", path)
            r = conn.getresponse()
            rule["snapshot"] = (r.status, r.read(),
                                [(k, v) for k, v in r.getheaders()
                                 if k.lower() not in ("transfer-encoding", "connection", "content-length")])
        with self._lock:
            self.rules.append(rule)
        return rule

    def clear(self) -> None:
        with self._lock:
            self.rules.clear()

    def stop(self) -> None:
        self.httpd.shutdown()
        self.httpd.server_close()

    def _rule_for(self, key: str) -> Optional[dict]:
        with self._lock:
            for r in self.rules:
                if r["match"].search(key) and (r["times"] is None or r["hits"] < r["times"]):
                    r["hits"] += 1
                    return r
        return None

    def _forward(self, h: BaseHTTPRequestHandler, body: bytes) -> tuple[int, list, bytes]:
        conn = http.client.HTTPConnection(*self.up, timeout=30)
        headers = {k: v for k, v in h.headers.items() if k.lower() not in ("host", "connection")}
        conn.request(h.command, h.path, body or None, headers)
        r = conn.getresponse()
        return r.status, [(k, v) for k, v in r.getheaders()
                          if k.lower() not in ("transfer-encoding", "connection", "content-length")], r.read()

    def _send(self, h: BaseHTTPRequestHandler, status: int, headers: list, data: bytes) -> None:
        h.send_response_only(status)  # no automatic Server/Date: send exactly the given headers
        for k, v in headers:
            h.send_header(k, v)
        h.send_header("Content-Length", str(len(data)))
        h.end_headers()
        h.wfile.write(data)

    def _handle(self, h: BaseHTTPRequestHandler) -> None:
        n = int(h.headers.get("Content-Length") or 0)
        body = h.rfile.read(n) if n else b""
        key = f"{h.command} {h.path}"
        rule = self._rule_for(key)
        self.log.append((time.time(), key, rule["action"] if rule else "pass"))
        act = rule["action"] if rule else "pass"
        try:
            if act == "reset":
                h.connection.shutdown(socket.SHUT_RDWR)
                h.connection.close()
            elif act == "hang":
                time.sleep(60)
            elif act == "drop_response":
                self._forward(h, body)
                time.sleep(60)
            elif act.startswith("status:"):
                self._send(h, int(act.split(":")[1]), [("Content-Type", "application/json")],
                           b'{"detail":"injected"}')
            elif act == "freeze":
                st, data, hdrs = rule["snapshot"]
                self._send(h, st, hdrs, data)
            else:
                if act.startswith("delay:"):
                    time.sleep(float(act.split(":")[1]))
                self._send(h, *self._forward(h, body))
        except (BrokenPipeError, ConnectionError, OSError):
            pass


# ------------------------------------------------------------------ manifests and gateways

BENCH_OVERRIDES = {
    "manufacturer": "labbench (experiment)", "model": "LabBench",
    "actions": {
        "move_to": {
            "expose": True,
            # Deliberately NOT idempotent: this experiment is about whether OpenMHS ever
            # repeats an action it should not. (Moving to an absolute target is harmless to
            # repeat; the declaration is the experiment's, not a claim about the device.)
            "side_effect": "reversible", "idempotent": False,
            "timing": {"expected_ms": 1500, "timeout_ms": 30000},
            "params": {
                "target": {"type": "integer", "range": {"min": -1000, "max": 1000}},
                "seconds": {"type": "number", "quantity": "time", "unit": "s", "range": {"min": 0.1, "max": 10}},
                "fail_after": {"type": "number", "quantity": "time", "unit": "s", "range": {"min": -1, "max": 10}},
            },
            "verification": {"mode": "readback", "capability": "position", "expect_param": "target",
                             "tolerance": {"abs": 0}, "settle_ms": 100, "timeout_ms": 5000},
            "reviewed_by": "experiment author (simulator only)",
            "reason": "bounds and idempotency chosen for the experiment",
        }
    },
}


def make_manifest(out: Path, base_url: str, *, upstream: str = BENCH, thing: str = "bench",
                  device_id: str = "lab.bench", overrides: Optional[dict] = None,
                  source: str = "simulator", timeout_ms: int = 1500, stamp: str = "received") -> Path:
    out.mkdir(parents=True, exist_ok=True)
    tds = http_json("GET", f"{upstream}/thing_descriptions/")[1]
    td = tds[thing]
    m = td_import.map_thing(td, base_url=base_url, device_id=device_id,
                            overrides=BENCH_OVERRIDES if overrides is None else overrides)
    manifest = import_td.build_manifest(m, base_url, out, source=source, timeout_ms=timeout_ms,
                                        stamp=stamp)
    path = out / "manifest.json"
    path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    return path


class Gateway:
    """A real ``openmhs serve`` process with one adapter manifest."""

    def __init__(self, home: Path, manifest: Path, *, grants: bool = True, skew_ms: int = 0) -> None:
        self.home, self.manifest, self.proc, self.grants = Path(home), Path(manifest), None, grants
        self.skew_ms = skew_ms

    def start(self) -> "Gateway":
        self.home.mkdir(parents=True, exist_ok=True)
        (self.home / "gateway.json").unlink(missing_ok=True)
        self.log = open(self.home / "serve.log", "ab")
        self.proc = subprocess.Popen(
            [OPENMHS, "--home", str(self.home), "serve", "--port", "0", "--adapter-manifest",
             str(self.manifest), "--allow-unsigned-adapters", "--health-interval-ms", "500",
             *(["--readback-skew-ms", str(self.skew_ms)] if self.skew_ms else [])],
            stdout=self.log, stderr=subprocess.STDOUT)
        end = time.time() + 30
        while time.time() < end:
            if (self.home / "gateway.json").exists():
                try:
                    self.op = Client.from_home(self.home)
                    break
                except Exception:
                    pass
            time.sleep(0.2)
        else:
            raise RuntimeError("the gateway did not start; see " + str(self.home / "serve.log"))
        if self.grants:
            for g in ({"id": "r", "principal": "agent:*", "device": "*", "capability": "*",
                       "operations": ["read"], "effect": "allow"},
                      {"id": "w", "principal": "agent:*", "device": "*", "capability": "*",
                       "operations": ["invoke", "write"], "effect": "allow"}):
                try:
                    self.op.add_grant(g)
                except Exception:
                    pass  # already present after a restart
            try:
                self.token = self.op.create_token("agent:exp", any_scope=True).token
            except Exception:
                self.token = None
            if self.token:
                self.agent = Client.from_home(self.home, token=self.token)
        return self

    def child_pids(self) -> list[int]:
        out = subprocess.run(
            ["powershell", "-NoProfile", "-Command",
             f"(Get-CimInstance Win32_Process -Filter 'ParentProcessId={self.proc.pid}').ProcessId"],
            capture_output=True, text=True).stdout.split()
        return [int(x) for x in out if x.isdigit()]

    def kill(self) -> None:
        """A hard stop: no graceful shutdown, no journal flush beyond what is already durable."""
        if self.proc and self.proc.poll() is None:
            self.proc.kill()
            self.proc.wait()

    def stop(self) -> None:
        self.kill()
        try:
            self.log.close()
        except Exception:
            pass
