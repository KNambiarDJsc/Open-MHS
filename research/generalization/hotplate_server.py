"""A stand-in hotplate that serves hotplate_td.json (simulator; no physics claimed).

Written from the Thing Description, not from OpenMHS: HTTP Basic authentication, properties as
GET/PUT of JSON, **synchronous** actions (POST answers 200 with the output), none of LabThings'
invocation resources. The plate warms or cools 2 degC per second toward the target while
"heating"; that is a toy model, not a thermal one.

    python hotplate_server.py [--port 5200]       # user "lab", password "bench"
"""

from __future__ import annotations

import argparse
import base64
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

USER, PASSWORD = "lab", "bench"
STATE = {"temperature": 22.0, "target": 22.0, "heating": False, "stir": 0, "fault": False,
         "program_until": None, "calls": []}
LOCK = threading.Lock()


def tick() -> None:
    while True:
        time.sleep(0.25)
        with LOCK:
            s = STATE
            goal = s["target"] if s["heating"] else 22.0
            step = 0.5
            if abs(goal - s["temperature"]) <= step:
                s["temperature"] = goal
            else:
                s["temperature"] += step if goal > s["temperature"] else -step
            if s["program_until"] and time.time() >= s["program_until"]:
                s["heating"], s["program_until"] = False, None
            s["stir"] = 400 if s["heating"] else 0


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a) -> None:
        pass

    def _json(self, status: int, body) -> None:
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _authorized(self) -> bool:
        want = "Basic " + base64.b64encode(f"{USER}:{PASSWORD}".encode()).decode()
        if self.headers.get("Authorization") == want:
            return True
        self.send_response(401)
        self.send_header("WWW-Authenticate", 'Basic realm="hotplate"')
        self.send_header("Content-Length", "0")
        self.end_headers()
        return False

    def _body(self):
        n = int(self.headers.get("Content-Length") or 0)
        return json.loads(self.rfile.read(n)) if n else None

    def do_GET(self) -> None:  # noqa: N802
        if not self._authorized():
            return
        with LOCK:
            s = STATE
            state = "fault" if s["fault"] else ("heating" if s["heating"] else
                    ("cooling" if abs(s["temperature"] - 22.0) > 0.5 else "idle"))
            props = {"temperature": round(s["temperature"], 1), "targetTemperature": s["target"],
                     "state": state, "stirRate": s["stir"]}
        name = self.path.rsplit("/", 1)[-1]
        if self.path.startswith("/properties/") and name in props:
            return self._json(200, props[name])
        self._json(404, {"error": "no such property"})

    def do_PUT(self) -> None:  # noqa: N802
        if not self._authorized():
            return
        if self.path == "/properties/targetTemperature":
            v = self._body()
            if not isinstance(v, (int, float)) or not 20 <= v <= 300:
                return self._json(400, {"error": "target must be 20..300"})
            with LOCK:
                STATE["target"] = float(v)
                STATE["calls"].append(("PUT", self.path, v))
            return self.send_response(204) or self.end_headers()
        self._json(404, {"error": "no such property"})

    def do_POST(self) -> None:  # noqa: N802
        if not self._authorized():
            return
        body = self._body()
        name = self.path.rsplit("/", 1)[-1]
        with LOCK:
            STATE["calls"].append(("POST", self.path, body))
            if name == "start":
                STATE["heating"] = True
                return self._json(200, None)
            if name == "stop":
                STATE["heating"] = False
                return self._json(200, None)
            if name == "selfTest":
                return self._json(200, {"ok": True})
            if name == "setProgram":
                if not isinstance(body, dict) or not {"temperature", "minutes"} <= body.keys():
                    return self._json(400, {"error": "temperature and minutes are required"})
                STATE["target"], STATE["heating"] = float(body["temperature"]), True
                STATE["program_until"] = time.time() + 60 * int(body["minutes"])
                return self._json(200, None)
        self._json(404, {"error": "no such action"})


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=5200)
    port = ap.parse_args().port
    threading.Thread(target=tick, daemon=True).start()
    ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
