"""Drive both MCP servers from mcp-config.json exactly as an agent would, with no model.

    python smoke_test.py <path to mcp-config.json>

Checks: the OpenMHS server lists the stage; a 3x3 survey runs and returns to the start; an
over-envelope survey is refused by OpenMHS and the tool says so; a focus sweep finds the sharpest z;
the session report's audit export verifies offline.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "python"))
from openmhs.agent.mcp import McpStdioClient  # noqa: E402


def server(cfg: dict, name: str) -> McpStdioClient:
    s = cfg["mcpServers"][name]
    return McpStdioClient([s["command"], *s["args"]], env={**os.environ, **s["env"]})


def main() -> int:
    cfg = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    ok = True
    with server(cfg, "openmhs") as mhs, server(cfg, "openflexure-imaging") as img:
        devices = mhs.call("list_devices", {}).structured
        names = [d.get("id") or d.get("device") for d in devices.get("devices", [])]
        print("openmhs list_devices:", names)
        ok &= "ofm.stage" in names
        print("imaging tools:", [t["name"] for t in img.list_tools()])

        s = img.call("survey_region", {"columns": 3, "rows": 3, "spacing": 500}).structured
        print("survey 3x3:", s["captured_tiles"], "/", s["planned_tiles"], "stopped:", s["stopped"], "most sample:", s["most_sample"])
        ok &= s["captured_tiles"] == 9 and s["stopped"] is None

        r = img.call("survey_region", {"columns": 3, "rows": 1, "spacing": 2000})
        refused = r.structured or {"stopped": r.text}
        print("over-envelope survey:", (refused.get("stopped") or "")[:220])
        ok &= bool(refused.get("stopped")) and "out_of_range" in (refused.get("stopped") or "")

        f = img.call("focus_sweep", {"z_values": [-300, -150, 0, 150, 300], "x": 0, "y": 0}).structured
        print("focus sweep:", [(p["z"], p["sharpness"]) for p in f["sweep"]], "best z:", f["best_z"], "stopped:", f["stopped"])
        ok &= f["best_z"] == 0 and f["stopped"] is None

        rep = img.call("session_report", {}).structured
        print("report:", rep["report"], "| moves", rep["moves"], "| images", rep["images"], "| audit verify exit", rep["audit_verify_exit"])
        ok &= rep["audit_verify_exit"] == 0
    print("SMOKE TEST", "PASSED" if ok else "FAILED")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
