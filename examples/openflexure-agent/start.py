"""Connect an AI agent to the OpenFlexure *simulator* through OpenMHS, in one command.

    python start.py                      # import the stage, start a gateway, write mcp-config.json
    python start.py --approval           # every stage move waits for a human approval
    python start.py --stop               # stop the gateway started by this script

It imports the simulator's stage from its live Thing Description with a reviewed envelope
(x, y within +/-1200 and z within +/-400 simulator steps; the description declares no units or
limits), starts an OpenMHS gateway, gives one agent principal read access and bounded moves, and
writes an MCP configuration with two servers:

  openmhs              the OpenMHS MCP server: discovery, reads, checked/verified/audited moves
  openflexure-imaging  capture_image, survey_region, focus_sweep, session_report; its moves also go
                       through OpenMHS with the same agent token, so nothing bypasses the gateway

Simulator only. Point --sim at a real microscope only after reviewing the envelope yourself; this
example has not been run against hardware.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path[:0] = [str(ROOT / "python"), str(ROOT / "adapters" / "python" / "wot")]

import import_td  # noqa: E402
import td_import  # noqa: E402
from openmhs import Client  # noqa: E402

DEVICE = "ofm.stage"
PRINCIPAL = "agent:technician"


def get(url: str):
    with urllib.request.urlopen(url, timeout=15) as r:  # noqa: S310
        return json.loads(r.read())


def stage_profile(xy: int, z: int) -> dict:
    """The reviewed contract for the stage. A Thing Description cannot say this; a person must."""
    def variant(axis: str, bound: int) -> dict:
        return {"params": {axis: {"type": "integer", "range": {"min": -bound, "max": bound}}},
                "verification": {"mode": "readback", "capability": f"position.{axis}", "expect_param": axis,
                                 "tolerance": {"abs": 0}, "settle_ms": 100, "timeout_ms": 10000}}
    return {"manufacturer": "OpenFlexure (simulator)", "model": "DummyStage", "actions": {"move_absolute": {
        "expose": True, "side_effect": "reversible", "idempotent": False,
        "timing": {"expected_ms": 3000, "timeout_ms": 60000},
        "omit_params": ["backlash_compensation", "block_cancellation"],
        "reviewed_by": "example profile (simulator)",
        "reason": f"Example envelope: x,y within +/-{xy}, z within +/-{z} simulator steps. Not a device limit.",
        "variants": {"move_x": variant("x", xy), "move_y": variant("y", xy), "move_z": variant("z", z)}}}}


def start(args) -> int:
    openmhs = Path(args.openmhs).resolve()
    if not openmhs.exists():
        raise SystemExit(f"OpenMHS binary not found at {openmhs}; build it (cargo build -p openmhs-cli) or pass --openmhs")
    home = Path(args.home).resolve()
    home.mkdir(parents=True, exist_ok=True)
    if (home / "gateway.pid").exists():
        raise SystemExit(f"a gateway from this script may already be running ({home}); use --stop first")
    for stale in ("gateway.json", "journal.sqlite", "journal.sqlite-wal", "journal.sqlite-shm", "policy.json", "tokens.json"):
        (home / stale).unlink(missing_ok=True)       # a fresh session every time

    api = args.sim.rstrip("/") + "/api/v3"
    tds = get(f"{api}/thing_descriptions/")
    samples = {}
    for name, prop in (tds["stage"].get("properties") or {}).items():
        if prop.get("type") == "object" and isinstance(prop.get("additionalProperties"), dict):
            form = td_import._form(prop.get("forms"), "readproperty")
            if form:
                samples[name] = get(args.sim.rstrip("/") + form["href"])
    mapping = td_import.map_thing(tds["stage"], base_url=args.sim.rstrip("/") + "/", device_id=DEVICE,
                                  overrides=stage_profile(args.xy, args.z), samples=samples)
    adapter_dir = home / "adapter"
    adapter_dir.mkdir(exist_ok=True)
    manifest = import_td.build_manifest(mapping, args.sim.rstrip("/") + "/", adapter_dir, source="simulator",
                                        timeout_ms=4000, stamp="received")
    (adapter_dir / "manifest.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")

    log = open(home / "gateway.log", "ab")
    flags = subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0
    proc = subprocess.Popen([str(openmhs), "--home", str(home), "serve", "--port", "0",
                             "--adapter-manifest", str(adapter_dir / "manifest.json"),
                             "--allow-unsigned-adapters"],
                            stdout=log, stderr=subprocess.STDOUT, creationflags=flags,
                            start_new_session=(os.name != "nt"))
    (home / "gateway.pid").write_text(str(proc.pid))
    for _ in range(150):
        if (home / "gateway.json").exists():
            try:
                op = Client.from_home(home)
                op.info()
                break
            except Exception:
                pass
        if proc.poll() is not None:
            raise SystemExit(f"the gateway exited; see {home / 'gateway.log'}")
        time.sleep(0.2)
    else:
        raise SystemExit(f"the gateway did not start; see {home / 'gateway.log'}")

    move = "require_approval" if args.approval else "allow"
    op.add_grant({"id": "tech-read", "principal": PRINCIPAL, "device": DEVICE, "capability": "*",
                  "operations": ["read"], "effect": "allow"})
    for axis in "xyz":
        op.add_grant({"id": f"tech-move-{axis}", "principal": PRINCIPAL, "device": DEVICE,
                      "capability": f"move_{axis}", "operations": ["invoke"], "effect": move})
    token = op.create_token(PRINCIPAL, any_scope=True).token

    env = {"OPENMHS_HOME": str(home), "OPENMHS_TOKEN": token}
    config = {"mcpServers": {
        "openmhs": {"command": str(openmhs), "args": ["mcp"], "env": env},
        "openflexure-imaging": {"command": str(Path(args.python).resolve()), "args": [str(HERE / "imaging_mcp.py")],
                                "env": {**env, "OFM_URL": args.sim.rstrip("/"),
                                        "OPENMHS_BIN": str(openmhs),
                                        "SESSION_DIR": str(home / "session")}}}}
    cfg = home / "mcp-config.json"
    cfg.write_text(json.dumps(config, indent=2), encoding="utf-8")
    print(f"""
OpenMHS gateway running (pid {proc.pid}); stage imported as '{DEVICE}', moves: {move}.
MCP configuration written to {cfg}
(it contains an agent token for this local gateway; treat it like a password)

Claude Code:     claude mcp add-json openmhs '{json.dumps(config["mcpServers"]["openmhs"])}'
                 claude mcp add-json openflexure-imaging '<the openflexure-imaging entry in {cfg.name}>'
Claude Desktop:  merge the "mcpServers" entries of {cfg.name} into claude_desktop_config.json

Then ask, for example:
  "Find the microscope stage, survey a 5x5 region around the current position with 500-step spacing,
   tell me where the sample is, refocus on the best tile, and write me a session report."
Operator view (approvals, audit):  {openmhs} --home {home} commands list
Stop:  python {Path(__file__).name} --stop --home {home}
Simulator only. Not tested on a physical microscope.""")
    return 0


def stop(args) -> int:
    home = Path(args.home).resolve()
    pid_file = home / "gateway.pid"
    if not pid_file.exists():
        print("no gateway recorded for this home")
        return 0
    pid = int(pid_file.read_text())
    try:
        if os.name == "nt":
            subprocess.run(["taskkill", "/PID", str(pid), "/T", "/F"], capture_output=True)
        else:
            os.killpg(pid, signal.SIGTERM)
    except Exception as e:  # already gone
        print(f"note: {e}")
    pid_file.unlink(missing_ok=True)
    print(f"stopped gateway {pid}; the journal and session files remain in {home}")
    return 0


def main() -> int:
    exe = "openmhs.exe" if os.name == "nt" else "openmhs"
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--sim", default="http://127.0.0.1:5000", help="OpenFlexure server URL (the simulator)")
    ap.add_argument("--home", default=str(Path.home() / ".openmhs-openflexure"))
    ap.add_argument("--openmhs", default=os.environ.get("OPENMHS_BIN") or str(ROOT / "target" / "debug" / exe))
    ap.add_argument("--python", default=sys.executable,
                    help="Python for the imaging server (needs numpy, Pillow, scipy)")
    ap.add_argument("--xy", type=int, default=1200)
    ap.add_argument("--z", type=int, default=400)
    ap.add_argument("--approval", action="store_true", help="every move needs a human approval")
    ap.add_argument("--stop", action="store_true")
    args = ap.parse_args()
    return stop(args) if args.stop else start(args)


if __name__ == "__main__":
    raise SystemExit(main())
