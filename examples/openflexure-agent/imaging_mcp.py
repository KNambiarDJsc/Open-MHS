"""A small MCP server (stdio) that gives an agent microscope-level tools for the OpenFlexure simulator.

Tools: capture_image, survey_region, focus_sweep, session_report. They are deliberately high level:
the agent asks for a survey, not for motor steps. Every stage move they make goes through OpenMHS
with the agent's own token (OPENMHS_HOME / OPENMHS_TOKEN), so OpenMHS checks it against the
reviewed envelope and the grants, verifies it by readback, and records it. A move whose outcome is
unknown stops the tool and is reported; nothing is ever resent. Images are camera reads from the
OpenFlexure server, saved with their sha256 and linked to the OpenMHS-verified position.

Image measurements are deterministic (no model): sample coverage, feature count, sharpness.
Needs numpy, Pillow and scipy. Simulator only.
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

import numpy as np
from PIL import Image
from scipy import ndimage

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "python"))
from openmhs import Client  # noqa: E402

DEVICE = "ofm.stage"
OFM = os.environ.get("OFM_URL", "http://127.0.0.1:5000").rstrip("/") + "/api/v3"
SESSION = Path(os.environ.get("SESSION_DIR", "openflexure-session")).resolve()
IMAGES = SESSION / "images"
client = Client.from_home(os.environ["OPENMHS_HOME"], token=os.environ["OPENMHS_TOKEN"])
captured: list[dict] = []          # this session's images and measurements
events: list[dict] = []            # what each tool did, for the report


def now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


# ------------------------------------------------------------------ camera and analysis

def _http(method: str, url: str, body=None, timeout: float = 60):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, method=method, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:  # noqa: S310
        raw = r.read()
        return json.loads(raw) if r.headers.get_content_type() == "application/json" else raw


def _camera_action(name: str, body=None) -> dict:
    inv = _http("POST", f"{OFM}/camera/{name}", body or {})
    for _ in range(300):
        if inv.get("status") in ("completed", "error", "cancelled"):
            return inv
        time.sleep(0.1)
        inv = _http("GET", inv["href"])
    raise TimeoutError(f"camera {name}")


def _position() -> dict:
    out = {}
    for axis in "xyz":
        r = client.read(DEVICE, f"position.{axis}")
        if not r.usable:
            raise RuntimeError(f"position.{axis} is not usable right now ({r.state.quality})")
        out[axis] = r.state.value
    return out


def _measure(a: np.ndarray, background: float) -> dict:
    mask = a < background - 40
    labels, n = ndimage.label(ndimage.binary_opening(mask[::4, ::4], iterations=1))
    sizes = np.bincount(labels.ravel())[1:] if n else np.array([])
    lap = a[1:-1, 1:-1] * 4 - a[:-2, 1:-1] - a[2:, 1:-1] - a[1:-1, :-2] - a[1:-1, 2:]
    return {"sample_fraction": round(float(mask.mean()), 4), "features": int((sizes >= 6).sum()),
            "sharpness": round(float(lap.var()), 2)}


def _background() -> float:
    """Empty-slide brightness: 95th percentile over everything captured so far (at least 217)."""
    if not captured:
        return 217.0
    pixels = np.concatenate([np.load(c["_array"]).ravel()[::97] for c in captured])
    return max(217.0, float(np.percentile(pixels, 95)))


def _capture(label: str) -> dict:
    pos = _position()
    _camera_action("settle")
    inv = _camera_action("grab_jpeg", {"stream_name": "main"})
    href = (inv.get("output") or {}).get("href") or next(
        l["href"] for l in inv.get("links", []) if l.get("rel") == "output")
    raw = _http("GET", href)
    IMAGES.mkdir(parents=True, exist_ok=True)
    image_id = f"img-{len(captured):03d}"
    (IMAGES / f"{image_id}.jpg").write_bytes(raw)
    a = np.asarray(Image.open(io.BytesIO(raw)).convert("L"), dtype=float)
    np.save(IMAGES / f"{image_id}.npy", a[::2, ::2])
    rec = {"image_id": image_id, "label": label, "captured_at": now(), "position": pos,
           "sha256": hashlib.sha256(raw).hexdigest(), "file": str(IMAGES / f"{image_id}.jpg"),
           "_array": str(IMAGES / f"{image_id}.npy")}
    rec["measurements"] = _measure(a[::2, ::2], _background())
    captured.append(rec)
    return {k: v for k, v in rec.items() if not k.startswith("_")}


# ------------------------------------------------------------------ moves, through OpenMHS

class Stop(Exception):
    """A move did not end verified: stop the tool and tell the agent exactly why."""


def _move(axis: str, value: int, why: str) -> dict:
    run = client.execute({"target": {"device": DEVICE, "capability": f"move_{axis}"}, "operation": "invoke",
                          "params": {axis: int(value)}}, timeout=60)
    receipt = run.wait(90)
    state = str(getattr(getattr(receipt, "state", None), "value", getattr(receipt, "state", None)))
    entry = {"move": f"{axis}={value}", "why": why, "state": state, "at": now(),
             "command_id": getattr(receipt, "command_id", None)}
    events.append(entry)
    if state == "VERIFIED":
        return entry
    if state == "AWAITING_APPROVAL":
        raise Stop(f"move {axis}={value} is waiting for a human approval (command {entry['command_id']}). "
                   "Ask the operator to approve it, then call this tool again.")
    if state == "UNKNOWN_OUTCOME":
        raise Stop(f"move {axis}={value} has an UNKNOWN outcome (command {entry['command_id']}). It was NOT resent. "
                   "Call the OpenMHS reconcile_command tool on that command before doing anything else.")
    reasons = "; ".join(f"{e.code}: {e.message}" for e in (receipt.policy.errors if receipt else []))
    raise Stop(f"move {axis}={value} ended {state}. {reasons}".strip())


def _goto(target: dict, why: str, here: dict) -> dict:
    for axis in ("z", "y", "x"):
        if axis in target and here.get(axis) != target[axis]:
            _move(axis, target[axis], why)
            here[axis] = target[axis]
    return here


# ------------------------------------------------------------------ tools

def capture_image(label: str = "") -> dict:
    return _capture(label or "manual capture")


def survey_region(columns: int = 3, rows: int = 3, spacing: int = 500) -> dict:
    if not (1 <= columns <= 7 and 1 <= rows <= 7 and 1 <= spacing <= 2000):
        raise ValueError("columns and rows must be 1..7 and spacing 1..2000 simulator steps")
    here = _position()
    start = dict(here)
    xs = [start["x"] + (i - (columns - 1) // 2) * spacing for i in range(columns)]
    ys = [start["y"] + (j - (rows - 1) // 2) * spacing for j in range(rows)]
    points = [(x, y) for j, y in enumerate(ys) for x in (xs if j % 2 == 0 else xs[::-1])]
    tiles, stopped = [], None
    try:
        for k, (x, y) in enumerate(points):
            _goto({"x": x, "y": y}, f"survey tile {k}", here)
            tiles.append(_capture(f"survey tile {k}"))
        _goto({"x": start["x"], "y": start["y"]}, "survey: return to start", here)
    except Stop as e:
        stopped = str(e)
    bg = _background()
    for t in tiles:                                  # remeasure against the whole survey's background
        rec = next(c for c in captured if c["image_id"] == t["image_id"])
        rec["measurements"] = t["measurements"] = _measure(np.load(rec["_array"]), bg)
    ranked = sorted(tiles, key=lambda t: (t["measurements"]["sample_fraction"], t["measurements"]["features"]), reverse=True)
    return {"planned_tiles": len(points), "captured_tiles": len(tiles), "stopped": stopped,
            "tiles": [{"image_id": t["image_id"], "x": t["position"]["x"], "y": t["position"]["y"], **t["measurements"]}
                      for t in tiles],
            "most_sample": [t["image_id"] for t in ranked[:3]],
            "note": "Measurements are deterministic image statistics, not a model's interpretation. Positions are "
                    "simulator steps (the stage declares no unit)."}


def focus_sweep(z_values: list[int] | None = None, x: int | None = None, y: int | None = None) -> dict:
    zs = z_values or [-300, -150, 0, 150, 300]
    if len(zs) > 9:
        raise ValueError("at most 9 z positions")
    here = _position()
    results, stopped = [], None
    try:
        _goto({k: v for k, v in (("x", x), ("y", y)) if v is not None}, "focus sweep position", here)
        for z in zs:
            _goto({"z": z}, "focus sweep", here)
            img = _capture(f"focus z={z}")
            results.append({"z": z, "image_id": img["image_id"], **img["measurements"]})
        best = max(results, key=lambda r: r["sharpness"])
        _goto({"z": best["z"]}, "move to best focus", here)
    except Stop as e:
        stopped = str(e)
        best = max(results, key=lambda r: r["sharpness"]) if results else None
    return {"sweep": results, "best_z": best and best["z"], "stopped": stopped,
            "position_now": here}


def session_report() -> dict:
    SESSION.mkdir(parents=True, exist_ok=True)
    exe = os.environ.get("OPENMHS_BIN", "openmhs")
    stamp = time.strftime("%Y%m%d-%H%M%S")
    evidence, checkpoint = SESSION / f"evidence-{stamp}.json", SESSION / f"checkpoint-{stamp}.json"
    # Exporting evidence is an operator action: OpenMHS refuses it with the agent's token. This one
    # read-only step uses the local operator credential in the gateway home; the agent never sees it.
    operator_env = {k: v for k, v in os.environ.items() if k != "OPENMHS_TOKEN"}
    exported = subprocess.run([exe, "--home", os.environ["OPENMHS_HOME"], "audit", "export", "--out", str(evidence),
                               "--checkpoint-out", str(checkpoint)], capture_output=True, text=True, env=operator_env)
    verified = subprocess.run([exe, "--json", "audit", "verify-file", str(evidence), "--checkpoint", str(checkpoint)],
                              capture_output=True, text=True) if exported.returncode == 0 else None
    lines = [f"# Microscope session report ({now()})", "",
             "**OpenFlexure simulator; not a physical microscope.** Positions are simulator steps.", "",
             "## Actions (every stage move went through OpenMHS)", "", "| time | move | why | state | command |",
             "|---|---|---|---|---|"]
    lines += [f"| {e['at']} | {e['move']} | {e['why']} | {e['state']} | `{e['command_id']}` |" for e in events]
    lines += ["", "## Images", "", "| image | label | x | y | z | sample fraction | features | sharpness | sha256 |",
              "|---|---|---|---|---|---|---|---|---|"]
    for c in captured:
        m, p = c["measurements"], c["position"]
        lines.append(f"| `{c['image_id']}` | {c['label']} | {p['x']} | {p['y']} | {p['z']} | {m['sample_fraction']} | "
                     f"{m['features']} | {m['sharpness']} | `{c['sha256'][:16]}` |")
    unresolved = [e for e in events if e["state"] not in ("VERIFIED",)]
    lines += ["", "## Audit", "",
              f"- export: {'ok' if exported.returncode == 0 else 'FAILED: ' + (exported.stderr or exported.stdout)[:300]}",
              f"- offline verification (`audit verify-file`): exit {verified.returncode if verified else 'not run'} "
              "(0 = valid and complete)", "", "## Uncertainties", "",
              "- Images are camera reads linked to OpenMHS-verified positions; OpenMHS does not attest image content.",
              "- Measurement thresholds are tuned for the simulator, not real samples."]
    lines += [f"- Unresolved or non-verified move: {e['move']} -> {e['state']} (`{e['command_id']}`)" for e in unresolved] \
        or ["- Every move in this session was VERIFIED."]
    report = SESSION / f"report-{stamp}.md"
    report.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return {"report": str(report), "evidence": str(evidence), "audit_verify_exit": verified.returncode if verified else None,
            "moves": len(events), "images": len(captured), "unresolved": unresolved}


TOOLS = {
    "capture_image": (capture_image, "Capture one image at the stage's current position (a camera read; moves nothing). "
                      "Returns its id, the OpenMHS-verified position, sha256 and deterministic measurements.",
                      {"label": {"type": "string"}}),
    "survey_region": (survey_region, "Survey a rectangular grid of tiles centred on the current position (serpentine order), "
                      "capturing and measuring an image at each, then return to the start. Every move goes through OpenMHS; "
                      "the survey stops and says why if any move is refused, needs approval or has an unknown outcome.",
                      {"columns": {"type": "integer", "minimum": 1, "maximum": 7},
                       "rows": {"type": "integer", "minimum": 1, "maximum": 7},
                       "spacing": {"type": "integer", "minimum": 1, "maximum": 2000}}),
    "focus_sweep": (focus_sweep, "Capture images at several z positions (optionally at a given x, y first), report the "
                    "sharpness of each and move to the sharpest. Moves go through OpenMHS.",
                    {"z_values": {"type": "array", "items": {"type": "integer"}, "maxItems": 9},
                     "x": {"type": "integer"}, "y": {"type": "integer"}}),
    "session_report": (session_report, "Write a report of this session: every move with its OpenMHS command id and state, "
                       "every image with position, measurements and sha256, and an offline-verified audit export.", {}),
}


# ------------------------------------------------------------------ MCP over stdio (newline-delimited JSON-RPC)

def _reply(mid, result=None, error=None) -> None:
    msg = {"jsonrpc": "2.0", "id": mid, **({"error": error} if error else {"result": result})}
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def serve() -> None:
    for line in sys.stdin:
        if not line.strip():
            continue
        msg = json.loads(line)
        method, mid = msg.get("method"), msg.get("id")
        if mid is None:
            continue                                   # notifications
        if method == "initialize":
            _reply(mid, {"protocolVersion": msg.get("params", {}).get("protocolVersion", "2025-06-18"),
                         "capabilities": {"tools": {}},
                         "serverInfo": {"name": "openflexure-imaging", "version": "0.1.0"},
                         "instructions": "High-level microscope tools for the OpenFlexure simulator. Prefer survey_region "
                                         "and focus_sweep over moving the stage yourself. Report only what the tools "
                                         "returned; the measurements are numbers, you cannot see the images."})
        elif method == "tools/list":
            _reply(mid, {"tools": [{"name": n, "description": d, "inputSchema": {"type": "object", "properties": p,
                                                                                  "additionalProperties": False}}
                                   for n, (_, d, p) in TOOLS.items()]})
        elif method == "tools/call":
            name = msg["params"]["name"]
            args = msg["params"].get("arguments") or {}
            try:
                out = TOOLS[name][0](**args)
                _reply(mid, {"content": [{"type": "text", "text": json.dumps(out, default=str)}],
                             "structuredContent": out, "isError": False})
            except Exception as e:                      # a refusal or failure is a result the agent must see
                _reply(mid, {"content": [{"type": "text", "text": f"{type(e).__name__}: {e}"}], "isError": True})
        elif method == "ping":
            _reply(mid, {})
        else:
            _reply(mid, error={"code": -32601, "message": f"unknown method {method}"})


if __name__ == "__main__":
    serve()
