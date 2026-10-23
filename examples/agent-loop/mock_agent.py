"""A scripted agent that hits the gateway's limits, reads why, and replans.

    cargo build -p openmhs-cli
    python examples/agent-loop/mock_agent.py

No model and no API key: the "agent" is a few rules that act only on what the gateway
returns, which is the point. Every refusal arrives over MCP as structured data
(a reason code, the rule, the numbers that bound it, next steps), and that is enough to
replan without guessing. Simulated devices only.

Tasks:
  1. Set the bench supply to 5 V. A single jump breaks the supply's slew limit; the
     agent reads the limit from the refusal and ramps instead, checking each step first.
  2. Set it to 15 V. That is outside the declared range: the agent does not retry or
     "get close", it reports back.
  3. Home the linear stage. This agent has no permission to move it: it stops and asks.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from typing import Any

from lab import PSU, SETPOINT, Lab
from mcp_stdio import McpStdioClient, ToolResult

AXIS = "sim.axis-01"
MARGIN_MS = 150


class Agent:
    def __init__(self, mcp: McpStdioClient, verbose: bool = True) -> None:
        self.mcp = mcp
        self.verbose = verbose
        self.keys = 0
        self.log: list[dict[str, Any]] = []

    def say(self, msg: str) -> None:
        if self.verbose:
            print(msg, flush=True)

    def tool(self, name: str, args: dict[str, Any] | None = None) -> ToolResult:
        r = self.mcp.call(name, args)
        s = r.structured if isinstance(r.structured, dict) else {}
        label = s.get("state") or s.get("decision") or ("error" if r.is_error else "ok")
        self.say(f"  -> {name}({json.dumps(args or {})}) : {label}")
        self.log.append({"tool": name, "args": args, "is_error": r.is_error, "result": s})
        return r

    def key(self, task: str) -> str:
        self.keys += 1
        return f"{task}-{self.keys}"

    def reading(self, device: str, capability: str) -> float:
        r = self.tool("read_state", {"device": device, "capability": capability})
        if r.is_error or not r.structured["usable"]:
            raise RuntimeError(f"no usable reading of {capability}: {r.text}")
        return float(r.structured["value"]["value"])

    # ------------------------------------------------------------------ the loop

    def set_value(self, task: str, device: str, capability: str, target: float, unit: str) -> str:
        """Try to reach ``target``; replan from refusals. Returns how it ended."""
        want = {"value": target, "unit": unit}
        r = self.tool("submit_command", {"device": device, "capability": capability,
                                         "value": want, "idempotency_key": self.key(task)})
        view = r.structured
        if view and view["confirmed"]:
            self.say(f"  {view['summary']}")
            return "done"
        reason = (view or {}).get("reasons", [{}])[0] if view else {}
        code = reason.get("code", "")
        if view:
            self.say(f"  refused: {code} ({reason.get('message')})")
            for step in view["next_steps"]:
                self.say(f"  gateway suggests: {step}")

        if code == "command.slew_exceeded":
            return self.ramp(task, device, capability, target, unit, reason["details"])
        if code == "command.value.out_of_range":
            d = reason.get("details", {})
            self.say(f"  replan: {target:g} {unit} is outside {d.get('min'):g}..{d.get('max'):g} "
                     f"{d.get('unit', unit)}. Not retrying; reporting to the human.")
            return "refused_out_of_range"
        if code.startswith("policy.denied"):
            self.say("  replan: not permitted. Asking the human for permission instead.")
            return "escalated"
        if view and view["state"] == "AWAITING_APPROVAL":
            r = self.tool("get_command", {"command_id": view["command_id"], "wait_ms": 30_000})
            return "done" if r.structured.get("confirmed") else "not_approved"
        if view and view["outcome"] == "uncertain":
            r = self.tool("reconcile_command", {"command_id": view["command_id"]})
            return "reconciled:" + r.structured["state"]
        return "gave_up:" + (code or r.text)

    def ramp(self, task: str, device: str, capability: str, target: float, unit: str,
             limit: dict[str, Any]) -> str:
        step, per_ms = float(limit["max_change"]), int(limit["per_ms"])
        current = self.reading(device, capability)
        self.say(f"  replan: the limit is {step:g} {unit} per {per_ms} ms; "
                 f"ramping {current:g} -> {target:g} {unit}")
        while abs(target - current) > 1e-9:
            nxt = current + max(-step, min(step, target - current))
            args = {"device": device, "capability": capability,
                    "value": {"value": round(nxt, 6), "unit": unit}}
            # Look before leaping: wait out the window until a check says yes.
            for _ in range(10):
                if self.tool("check_command", args).structured["decision"] == "allow":
                    break
                time.sleep((per_ms + MARGIN_MS) / 1000)
            r = self.tool("submit_command", {**args, "idempotency_key": self.key(task)})
            if not r.structured or not r.structured["confirmed"]:
                self.say(f"  step to {nxt:g} {unit} did not verify: {r.text}")
                return "ramp_failed"
            current = self.reading(device, capability)
            self.say(f"  verified at {current:g} {unit}")
        return "done"

    def home_stage(self, task: str) -> str:
        r = self.tool("submit_command", {"device": AXIS, "capability": "axis.home",
                                         "idempotency_key": self.key(task)})
        view = r.structured or {}
        if view.get("confirmed"):
            return "done"
        codes = [x["code"] for x in view.get("reasons", [])]
        self.say(f"  refused: {', '.join(codes) or r.text}")
        if any(c.startswith("policy.denied") for c in codes):
            self.say("  replan: I may not move the stage. Asking the human for permission.")
            return "escalated"
        return "gave_up"


def run(verbose: bool = True) -> dict[str, Any]:
    with Lab() as lab:
        cmd, env = lab.mcp_command()
        with McpStdioClient(cmd, env) as mcp:
            agent = Agent(mcp, verbose)
            agent.say(f"connected to {mcp.server_info.get('name')} "
                      f"{mcp.server_info.get('version')} over MCP stdio")
            tools = [t["name"] for t in mcp.list_tools()]
            agent.say(f"tools: {', '.join(tools)}")
            devices = agent.tool("list_devices").structured["devices"]
            agent.say(f"devices: {', '.join(d['id'] for d in devices)}")

            outcomes = {}
            agent.say("\ntask 1: set the bench supply to 5 V")
            outcomes["psu_5v"] = agent.set_value("t1", PSU, SETPOINT, 5.0, "V")
            agent.say("\ntask 2: set the bench supply to 15 V")
            outcomes["psu_15v"] = agent.set_value("t2", PSU, SETPOINT, 15.0, "V")
            agent.say("\ntask 3: home the linear stage")
            outcomes["home_stage"] = agent.home_stage("t3")
            final = agent.reading(PSU, SETPOINT)

        audit = lab.operator.audit_verify()
        result = {
            "outcomes": outcomes,
            "final_setpoint_v": final,
            "tool_calls": len(agent.log),
            "audit_valid": audit.valid,
            "audit_records": audit.records,
        }
        agent.say(f"\nfinal setpoint {final:g} V; audit chain "
                  f"{'valid' if audit.valid else 'INVALID'} ({audit.records} records)")
        return result


EXPECTED = {"psu_5v": "done", "psu_15v": "refused_out_of_range", "home_stage": "escalated"}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--json", action="store_true", help="print only a JSON summary")
    args = ap.parse_args()
    result = run(verbose=not args.json)
    ok = result["outcomes"] == EXPECTED and abs(result["final_setpoint_v"] - 5.0) < 1e-6 \
        and result["audit_valid"]
    result["ok"] = ok
    if args.json:
        print(json.dumps(result))
    else:
        print("ALL TASKS ENDED AS EXPECTED" if ok else f"UNEXPECTED: {result}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
