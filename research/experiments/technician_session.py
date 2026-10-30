"""A technician's natural-language request, handled by a model with both MCP servers attached.

Uses the MCP configuration written by examples/openflexure-agent/start.py (the OpenMHS server and
the openflexure-imaging server), exactly what a technician would add to Claude Desktop or Claude
Code. The model sees every tool of both servers and only the technician's words. Afterwards the
model's answer is checked against what the tools actually returned. Simulator only.

    OPENAI_API_KEY=... python technician_session.py <mcp-config.json>
"""

from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT / "python"), str(ROOT / "research" / "experiments")]

import realistic_survey as budget  # noqa: E402  (spend ledger and model settings)
from openmhs.agent.mcp import McpStdioClient  # noqa: E402
from openmhs.agent.providers import ToolOutcome, ToolSpec  # noqa: E402

OUT = ROOT / "research" / "results" / "reports" / "technician_session"
REQUEST = ("There's a slide on the microscope stage. Please find where the sample is: survey a 5x5 area around "
           "the current position with 500-step spacing and tell me which tiles have the most sample. Then refocus "
           "on the best tile and give me a session report, including whether the audit check passed.")
SYSTEM = ("You help a lab technician run an OpenFlexure microscope (a simulator) through the tools provided. "
          "Prefer the high-level imaging tools for surveys and focusing. Positions are simulator steps, not "
          "millimetres. You cannot see images; report only what the tools returned, and say plainly if something "
          "failed, was refused or is uncertain.")


def _names_tile(text: str, survey: dict) -> bool:
    """The top tile, by image id or by its (x, y) coordinates."""
    top = next(t for t in survey["tiles"] if t["image_id"] == survey["most_sample"][0])
    return top["image_id"] in text or f"({top['x']}, {top['y']})" in text


def _ungrounded_numbers(text: str, steps: list[dict]) -> list[str]:
    """Numbers in the answer that appear nowhere in any tool result or in the request (file-name
    fragments such as dates are ignored). An empty list means every figure came from a tool."""
    import re
    blob = json.dumps([s["result"] for s in steps], default=str) + REQUEST
    known = set(re.findall(r"-?\d+(?:\.\d+)?", blob))
    found = re.findall(r"(?<![\w\-])-?\d+(?:\.\d+)?(?![\w\-])", re.sub(r"(?m)^\s*\d+\.\s", "", re.sub(r"`[^`]*`", "", text)))
    return sorted({n for n in found if n not in known and n.lstrip("-") not in known})


def main() -> int:
    cfg = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    OUT.mkdir(parents=True, exist_ok=True)
    provider = budget.make_provider()
    servers = {}
    for name, s in cfg["mcpServers"].items():
        servers[name] = McpStdioClient([s["command"], *s["args"]], env={**os.environ, **s["env"]}).__enter__()
    try:
        route, specs = {}, []
        for name, c in servers.items():
            for t in c.list_tools():
                route[t["name"]] = c
                specs.append(ToolSpec(t["name"], t.get("description", ""), t.get("inputSchema") or {"type": "object"}))
        conv = provider.start(SYSTEM, REQUEST, specs)
        steps, text, stopped, t0 = [], "", "max_turns", time.time()
        for _ in range(30):
            turn = conv.next_turn()
            text = turn.text or text
            if turn.stop != "tools" or not turn.calls:
                stopped = turn.stop
                break
            outs = []
            for call in turn.calls:
                r = route[call.name].call(call.name, call.arguments)
                content = json.dumps(r.structured) if r.structured is not None else r.text
                steps.append({"tool": call.name, "arguments": call.arguments, "is_error": r.is_error,
                              "result": r.structured if r.structured is not None else r.text})
                print(f"[{call.name}] {json.dumps(call.arguments)[:120]} -> {content[:160]}", flush=True)
                outs.append(ToolOutcome(call=call, content=content, is_error=r.is_error))
            conv.add_results(outs)
        budget.add_spend(provider.estimated_cost_usd, "technician_session")

        # check the answer against what the tools returned
        survey = next((s["result"] for s in steps if s["tool"] == "survey_region" and not s["is_error"]), None)
        focus = next((s["result"] for s in reversed(steps) if s["tool"] == "focus_sweep" and not s["is_error"]), None)
        report = next((s["result"] for s in reversed(steps) if s["tool"] == "session_report" and not s["is_error"]), None)
        checks = {
            "used_survey_tool": survey is not None,
            "survey_complete": bool(survey) and survey["captured_tiles"] == survey["planned_tiles"] == 25,
            "answer_names_tool_top_tile": bool(survey) and _names_tile(text, survey),
            "numbers_not_in_tool_output": _ungrounded_numbers(text, steps),
            "refocused": focus is not None and focus.get("stopped") is None,
            "answer_states_tool_best_z": bool(focus) and f"{focus['best_z']}" in text,
            "report_written": report is not None,
            "audit_verified": bool(report) and report.get("audit_verify_exit") == 0,
            "moved_stage_directly": sum(1 for s in steps if s["tool"] == "submit_command"),
        }
        result = {"request": REQUEST, "model": provider.model, "stopped": stopped, "final_answer": text,
                  "tool_sequence": [s["tool"] for s in steps], "checks": checks, "steps": steps,
                  "usage": {"input_tokens": provider.input_tokens, "output_tokens": provider.output_tokens,
                            "estimated_cost_usd": round(provider.estimated_cost_usd, 6)},
                  "seconds": round(time.time() - t0, 1), "simulator_only": True, "date": time.strftime("%Y-%m-%d")}
        (OUT / "session.json").write_text(json.dumps(result, indent=1, default=str) + "\n", encoding="utf-8")
        print("\nFINAL ANSWER:\n" + text + "\n\nCHECKS: " + json.dumps(checks) +
              f"\ncost ${provider.estimated_cost_usd:.4f}, ledger ${budget.spent():.4f}")
        return 0
    finally:
        for c in servers.values():
            c.__exit__(None, None, None)


if __name__ == "__main__":
    raise SystemExit(main())
