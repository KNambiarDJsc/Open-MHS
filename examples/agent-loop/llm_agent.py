"""Optional: the same agent loop, with Claude deciding instead of a script.

    pip install anthropic
    export ANTHROPIC_API_KEY=...        # only you can supply this; nothing here needs it
    python examples/agent-loop/llm_agent.py
    python examples/agent-loop/llm_agent.py --task "Ramp the supply to 7 V" --confirm

Without a key it explains how to run and exits; tests never call the API. To see what
Claude would be given without any key or SDK: ``--print-tools``.

Claude talks to a throwaway lab of *simulated* devices through `openmhs mcp`, holding
only an agent token. It gets the MCP server's tools, converted to Claude tool
definitions, and the server's instructions as part of its system prompt. Every call it
makes goes through the gateway's policy, limits and audit log like any other client's.
With ``--confirm`` you also approve each command at the terminal before it is sent.

Uses a manual tool-use loop because the transport is our own minimal MCP client (the SDK's
MCP helpers in ``anthropic.lib.tools.mcp`` need the official ``mcp`` package instead).
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from typing import Any

from lab import PSU, SETPOINT, Lab
from mcp_stdio import McpStdioClient

MODEL = "claude-opus-5-5"
MAX_TURNS = 30
DEFAULT_TASK = (
    "Set the bench power supply's output voltage setpoint to 5 V and confirm it took "
    "effect. Then home the linear stage."
)

SYSTEM = """\
You are a lab assistant operating hardware through the OpenMHS gateway's MCP tools. The \
devices in this session are simulators, but act exactly as you would with real equipment.
When you are finished, reply with a short report: what you did, what was confirmed by a \
readback, what was refused and why, and anything the human needs to decide.

The gateway's own instructions follow.

"""


def risk_note(annotations: dict[str, Any] | None) -> str:
    """The Claude API has no tool annotations, so put the risk where the model reads."""
    a = annotations or {}
    if a.get("readOnlyHint"):
        return "[read-only] "
    if a.get("destructiveHint"):
        return "[CHANGES THE PHYSICAL WORLD] "
    return "[changes gateway state] "


def claude_tools(mcp_tools: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """MCP tool definitions -> Claude tool definitions."""
    out = []
    for t in sorted(mcp_tools, key=lambda t: t["name"]):  # stable order keeps the cache warm
        schema = {k: v for k, v in t["inputSchema"].items() if k != "$schema"}
        out.append({
            "name": t["name"],
            "description": risk_note(t.get("annotations")) + (t.get("description") or ""),
            "input_schema": schema,
        })
    return out


def ask_human(name: str, args: dict[str, Any]) -> bool:
    print(f"\n  Claude wants to call {name} {json.dumps(args)}")
    try:
        return input("  allow? [y/N] ").strip().lower() in ("y", "yes")
    except EOFError:
        return False


def run(task: str, confirm: bool, effort: str) -> int:
    import anthropic  # optional dependency, imported only when actually talking to Claude

    client = anthropic.Anthropic()
    with Lab() as lab:
        cmd, env = lab.mcp_command()
        with McpStdioClient(cmd, env) as mcp:
            listed = mcp.list_tools()
            tools = claude_tools(listed)
            # Everything that changes the physical world: submit_command and submit_plan.
            physical = {t["name"] for t in listed if (t.get("annotations") or {}).get("destructiveHint")}
            system = SYSTEM + mcp.instructions
            messages: list[dict[str, Any]] = [{"role": "user", "content": task}]
            print(f"task: {task}\nmodel: {MODEL}  tools: {', '.join(t['name'] for t in tools)}")

            for _ in range(MAX_TURNS):
                try:
                    response = client.beta.messages.create(
                        model=MODEL,
                        max_tokens=16000,
                        system=system,
                        tools=tools,
                        messages=messages,
                        output_config={"effort": effort},
                        cache_control={"type": "ephemeral"},
                        # On a safety decline, retry on Anthropic's recommended model.
                        betas=["server-side-fallback-2026-07-01"],
                        fallbacks="default",
                    )
                except anthropic.AuthenticationError:
                    print("error: the API key was rejected", file=sys.stderr)
                    return 1
                except anthropic.RateLimitError:
                    print("error: rate limited; try again later", file=sys.stderr)
                    return 1
                except anthropic.APIStatusError as e:
                    print(f"error: API returned {e.status_code}: {e.message}", file=sys.stderr)
                    return 1
                except anthropic.APIConnectionError:
                    print("error: cannot reach the Claude API", file=sys.stderr)
                    return 1

                if response.stop_reason == "refusal":
                    category = response.stop_details.category if response.stop_details else None
                    print(f"\nClaude declined this request (category: {category}).")
                    return 1
                # Append the whole content unchanged: thinking blocks must be passed back.
                messages.append({"role": "assistant", "content": response.content})
                for block in response.content:
                    if block.type == "text" and block.text.strip():
                        print(f"\nclaude: {block.text}")

                if response.stop_reason == "max_tokens":
                    print("\nstopped: the response hit max_tokens")
                    return 1
                if response.stop_reason == "pause_turn":
                    continue
                calls = [b for b in response.content if b.type == "tool_use"]
                if not calls:
                    break

                results = []
                for call in calls:
                    if confirm and call.name in physical and not ask_human(call.name, call.input):
                        results.append({"type": "tool_result", "tool_use_id": call.id, "is_error": True,
                                        "content": "The human operator declined this command. "
                                                   "Do not retry it; ask them what they want instead."})
                        continue
                    r = mcp.call(call.name, call.input)
                    state = (r.structured or {}).get("state") or ("error" if r.is_error else "ok")
                    print(f"  -> {call.name}({json.dumps(call.input)}) : {state}")
                    results.append({"type": "tool_result", "tool_use_id": call.id,
                                    "content": r.text or json.dumps(r.structured),
                                    "is_error": r.is_error})
                messages.append({"role": "user", "content": results})
            else:
                print(f"\nstopped after {MAX_TURNS} turns")

        final = lab.operator.read(PSU, SETPOINT).value
        audit = lab.operator.audit_verify()
        print(f"\nground truth: setpoint {final.value:g} {final.unit}; audit chain "
              f"{'valid' if audit.valid else 'INVALID'} ({audit.records} records)")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--task", default=DEFAULT_TASK)
    ap.add_argument("--confirm", action="store_true", help="approve each command at the terminal")
    ap.add_argument("--effort", default="high", choices=["low", "medium", "high", "xhigh", "max"])
    ap.add_argument("--print-tools", action="store_true",
                    help="print the Claude tool definitions and exit (no key, no SDK needed)")
    args = ap.parse_args()

    if args.print_tools:
        with Lab() as lab:
            cmd, env = lab.mcp_command()
            with McpStdioClient(cmd, env) as mcp:
                print(json.dumps(claude_tools(mcp.list_tools()), indent=2))
        return 0
    if not (os.environ.get("ANTHROPIC_API_KEY") or os.environ.get("ANTHROPIC_AUTH_TOKEN")):
        print("This example needs your own Claude API key: set ANTHROPIC_API_KEY and run again.\n"
              "Nothing was run. The scripted agent needs no key: python examples/agent-loop/mock_agent.py")
        return 0
    try:
        import anthropic  # noqa: F401
    except ImportError:
        print("This example needs the Anthropic SDK: pip install anthropic")
        return 1
    return run(args.task, args.confirm, args.effort)


if __name__ == "__main__":
    sys.exit(main())
