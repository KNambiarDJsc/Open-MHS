"""An agent that learns from what physically happened, across sessions.

    cargo build -p openmhs-cli
    python examples/agent-loop/learning_agent.py

Two separate MCP sessions (two "conversations") against one gateway and one agent
credential. No model and no key; the point is the loop, which a model follows the same way.

Session 1 knows nothing. It asks for 8 V in one jump, is refused with the slew limit, and
ramps in steps the gateway accepts. Each step is confirmed by readback.

Session 2 starts cold, but first calls ``get_insights``. The journal tells it the limit it
ran into (from the refusal's own numbers), the values the supply was confirmed to hold, how
far readings landed from the request, and how long readback took. It plans its ramp from
that and reaches 3 V without being refused once.

What it does not do: learning never widens permissions. Insights are a read of the
journal; the limits, the policy and the descriptor are unchanged by anything an agent
records or concludes. An unfamiliar situation (a new device, a refusal it has not seen,
an unresolved outcome) goes back to ``check_command`` and, failing that, to a human.
Simulated devices only.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from typing import Any

from lab import PSU, SETPOINT, Lab
from mcp_stdio import McpStdioClient

SLEW = "command.slew_exceeded"


def session(lab: Lab, target: float, say) -> dict[str, Any]:
    """One conversation: learn what the journal knows, then move toward ``target`` volts."""
    cmd, env = lab.mcp_command()
    refused = 0
    steps: list[float] = []
    with McpStdioClient(cmd, env) as mcp:
        def call(name: str, args: dict[str, Any]) -> dict[str, Any]:
            return mcp.call(name, args).structured or {}

        now = call("read_state", {"device": PSU, "capability": SETPOINT})["value"]["value"]
        known = call("get_insights", {"device": PSU, "capability": SETPOINT})["insights"]
        cap = (known["capabilities"] or [{}])[0]
        slew = next((r for r in cap.get("refusals", []) if r["code"] == SLEW), None)
        if slew:
            limit = slew["latest_details"]
            say(f"  insights: the supply refused a jump bigger than {limit['max_change']:g} "
                f"{limit['unit']} per {limit['per_ms']} ms ({slew['count']} refusal(s) so far)")
            verified = cap.get("verified")
            if verified:
                say(f"  insights: confirmed range so far {verified['lowest']:g}..{verified['highest']:g} "
                    f"{verified['unit']}; readings landed within {cap['tracking_error']['max_abs']:g} "
                    f"{verified['unit']} of the request; readback took "
                    f"{cap['verify_latency_ms']['median']} ms")
            step, per_ms = float(limit["max_change"]), int(limit["per_ms"])
        else:
            say("  insights: nothing recorded yet; trying the target directly")
            r = mcp.call("submit_command", {"device": PSU, "capability": SETPOINT,
                                            "value": {"value": target, "unit": "V"},
                                            "idempotency_key": f"s{target}-direct"})
            view = r.structured or {}
            if view.get("confirmed"):
                return {"refused": 0, "steps": [target]}
            reason = (view.get("reasons") or [{}])[0]
            if reason.get("code") != SLEW:
                return {"refused": 1, "steps": [], "stopped": reason.get("code")}
            refused += 1
            say(f"  refused: {reason['message']}")
            step, per_ms = float(reason["details"]["max_change"]), int(reason["details"]["per_ms"])

        current = float(now)
        while abs(target - current) > 1e-9:
            nxt = round(current + max(-step, min(step, target - current)), 6)
            args = {"device": PSU, "capability": SETPOINT, "value": {"value": nxt, "unit": "V"}}
            for _ in range(10):  # wait out the slew window, then ask before sending
                if call("check_command", args)["decision"] == "allow":
                    break
                time.sleep((per_ms + 150) / 1000)
            view = call("submit_command", {**args, "idempotency_key": f"s{target}-{len(steps)}-{nxt}"})
            if not view.get("confirmed"):
                reason = (view.get("reasons") or [{}])[0]
                refused += 0 if view.get("reached_device") else 1
                return {"refused": refused, "steps": steps, "stopped": reason.get("code", view.get("state"))}
            steps.append(nxt)
            current = nxt
            say(f"  confirmed {nxt:g} V")
    return {"refused": refused, "steps": steps}


def run(verbose: bool = True) -> dict[str, Any]:
    def say(msg: str) -> None:
        if verbose:
            print(msg, flush=True)

    with Lab() as lab:
        say("session 1: a cold start, target 8 V")
        first = session(lab, 8.0, say)
        say("\nsession 2: a new conversation with the same credential, target 3 V")
        second = session(lab, 3.0, say)
        audit = lab.operator.audit_verify()
    return {"first": first, "second": second, "audit_valid": audit.valid}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--json", action="store_true", help="print only a JSON summary")
    args = ap.parse_args()
    result = run(verbose=not args.json)
    ok = (
        result["first"]["refused"] == 1
        and result["first"]["steps"][-1] == 8.0
        and result["second"]["refused"] == 0
        and result["second"]["steps"][-1] == 3.0
        and result["audit_valid"]
    )
    result["ok"] = ok
    if args.json:
        print(json.dumps(result))
    else:
        print("\nsession 2 reached its target without a single refusal" if ok else f"\nUNEXPECTED: {result}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
