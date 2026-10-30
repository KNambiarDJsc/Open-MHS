"""Run a natural-language, capability-discovery square-path task on the simulator only."""

from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [
    str(ROOT / "python"),
    str(ROOT / "adapters" / "python" / "wot"),
    str(ROOT / "research" / "experiments"),
]

import export_report  # noqa: E402
import import_td  # noqa: E402
import td_import  # noqa: E402
import openflexure_quickstart as quickstart  # noqa: E402
from lab import Gateway, OPENMHS, WORK, http_json  # noqa: E402
from openmhs.agent import Harness, OpenAICompatibleProvider  # noqa: E402
from openflexure_quickstart import DEVICE, stage_override  # noqa: E402


class GuidedProvider:
    """Let the model inspect the registered stage contract, then use a small tool set."""

    ALLOWED = {
        "list_devices", "describe_device", "read_state", "check_command", "submit_command",
    }

    def __init__(self, provider):
        self.provider = provider
        self.name = provider.name

    def start(self, system, task, tools):
        return self.provider.start(
            system, task, [tool for tool in tools if tool.name in self.ALLOWED]
        )


def _variant(axis: str, bound: int) -> dict:
    position = f"position.{axis}"
    return {
        "params": {
            axis: {"type": "integer", "range": {"min": -bound, "max": bound}},
        },
        "verification": {
            "mode": "readback", "capability": position,
            "expect_param": axis, "tolerance": {"abs": 0},
            "settle_ms": 100, "timeout_ms": 5000,
        },
    }


def _stage_contract() -> dict:
    """Restrict both axes to a 2-coordinate band for this one simulator task."""
    bound = 2
    override = stage_override(bound=bound)
    action = override["actions"]["move_absolute"]
    action["reason"] = (
        "Experiment-only x/y bounds [-2, 2] for the simulated square path; "
        "not device limits or physical safety limits."
    )
    action["variants"]["move_y"] = _variant("y", bound)
    return override


def _state_value(step) -> int | float | None:
    if (step.tool == "read_state" and not step.is_error
            and isinstance(step.result, dict) and step.result.get("usable") is True
            and step.result.get("quality") == "GOOD"):
        value = step.result.get("value")
        if type(value) in (int, float):
            return value
    return None


def run() -> int:
    simulator = "http://127.0.0.1:5000"
    api = simulator + "/api/v3"
    root = WORK / "guided-openflexure-square"
    report_dir = root / "report"
    manifest_dir = root / "adapter"
    report_dir.mkdir(parents=True, exist_ok=True)
    manifest_dir.mkdir(parents=True, exist_ok=True)

    tds = http_json("GET", f"{api}/thing_descriptions/")[1]
    if not isinstance(tds, dict) or "stage" not in tds:
        raise RuntimeError("the simulator did not return its stage Thing Description")

    samples = {}
    for name, prop in (tds["stage"].get("properties") or {}).items():
        if prop.get("type") == "object" and isinstance(prop.get("additionalProperties"), dict):
            form = td_import._form(prop.get("forms"), "readproperty")
            if form:
                status, value = http_json("GET", simulator + form["href"])
                if status == 200 and isinstance(value, dict):
                    samples[name] = value

    proxy = quickstart.FaultProxy(upstream=simulator)
    try:
        mapping = td_import.map_thing(
            tds["stage"], base_url=proxy.base + "/", device_id=DEVICE,
            overrides=_stage_contract(), samples=samples,
        )
        names = {item["name"] for item in mapping.descriptor["capabilities"]}
        required = {"position.x", "position.y", "move_x", "move_y"}
        if not required.issubset(names):
            raise RuntimeError(f"the imported stage lacks required square-path capabilities: {required - names}")

        manifest = import_td.build_manifest(
            mapping, proxy.base + "/", manifest_dir, source="simulator",
            timeout_ms=2000, stamp="received",
        )
        manifest_path = manifest_dir / "manifest.json"
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

        gateway = Gateway(root / "gateway", manifest_path, grants=False).start()
        try:
            grants = [
                {"id": "guided-square-read", "principal": "agent:guided-square",
                 "device": DEVICE, "capability": "*", "operations": ["read"],
                 "effect": "allow"},
                {"id": "guided-square-x", "principal": "agent:guided-square",
                 "device": DEVICE, "capability": "move_x", "operations": ["invoke"],
                 "effect": "allow"},
                {"id": "guided-square-y", "principal": "agent:guided-square",
                 "device": DEVICE, "capability": "move_y", "operations": ["invoke"],
                 "effect": "allow"},
            ]
            for grant in grants:
                gateway.op.add_grant(grant)
            gateway.token = gateway.op.create_token("agent:guided-square", any_scope=True).token
            agent_env = {
                **os.environ,
                "OPENMHS_HOME": str(gateway.home),
                "OPENMHS_TOKEN": gateway.token,
            }

            provider = OpenAICompatibleProvider(
                "https://api.openai.com/v1", "gpt-6-luna", timeout=90, retries=0,
                max_completion_tokens=512, reasoning_effort="none",
                # Prior runs consumed about $0.00175 of the user's $0.50 overall allowance.
                budget_usd=0.47, input_price_per_million=0.10,
                output_price_per_million=0.50,
            )
            guided_provider = GuidedProvider(provider)
            control = {
                "listed_stage": False, "described_stage": False,
                "initial": {}, "observed": {}, "last_preflight": None, "pending_readback": None,
                "leg_index": 0, "legs": [], "final": {}, "blocked_submissions": 0,
            }
            route = [
                ("move_x", "x", 1, "position.x"),
                ("move_y", "y", 1, "position.y"),
                ("move_x", "x", -1, "position.x"),
                ("move_y", "y", -1, "position.y"),
            ]

            def on_step(step):
                result = step.result if isinstance(step.result, dict) else {}
                if step.tool == "list_devices" and not step.is_error:
                    control["listed_stage"] = any(
                        d.get("device") == DEVICE or d.get("id") == DEVICE
                        for d in result.get("devices", []) if isinstance(d, dict)
                    )
                elif (step.tool == "describe_device" and not step.is_error
                      and step.arguments.get("device") == DEVICE):
                    control["described_stage"] = True
                elif step.tool == "check_command":
                    if (not step.is_error and result.get("decision") != "deny"
                            and step.arguments.get("device") == DEVICE):
                        control["last_preflight"] = (
                            step.arguments.get("capability"), step.arguments.get("params"),
                        )
                    else:
                        control["last_preflight"] = None
                elif step.tool == "submit_command":
                    control["last_preflight"] = None
                    command_id = quickstart.find_command_id(result)
                    if (not step.is_error and command_id
                            and result.get("state") == "VERIFIED"
                            and result.get("verification", {}).get("matched") is True
                            and control["pending_readback"] is None
                            and control["leg_index"] < len(route)):
                        capability, axis, delta, position = route[control["leg_index"]]
                        target = control["initial"].get(position)
                        if target is None:
                            control["blocked_submissions"] += 1
                        else:
                            origin = control["initial"][position]
                            target = origin + sum(
                                item[2] for item in route[:control["leg_index"]]
                                if item[3] == position
                            ) + delta
                            control["pending_readback"] = (position, target)
                            control["legs"].append({
                                "capability": capability, "axis": axis, "target": target,
                                "command_id": command_id,
                            })
                            control["leg_index"] += 1
                    else:
                        control["blocked_submissions"] += 1

                value = _state_value(step)
                if value is not None:
                    cap = step.arguments.get("capability")
                    control["observed"][cap] = value
                    if control["pending_readback"] is not None:
                        expected_cap, expected_value = control["pending_readback"]
                        if cap == expected_cap and value == expected_value:
                            control["pending_readback"] = None
                    if control["leg_index"] == 0:
                        control["initial"][cap] = value
                    if (control["leg_index"] == len(route)
                            and control["pending_readback"] is None):
                        # The final x readback is from leg 3; leg 4 only changes y.
                        # Preserve both independently observed final coordinates.
                        control["final"] = dict(control["observed"])

                print(f"[{step.tool}] {json.dumps(step.result, default=str)[:1200]}", flush=True)

            def submit_guard(call):
                if control["leg_index"] >= len(route) or control["pending_readback"] is not None:
                    control["blocked_submissions"] += 1
                    return False
                capability, axis, delta, position = route[control["leg_index"]]
                params = call.arguments.get("params") or {}
                origin = control["initial"].get(position)
                if origin is None:
                    control["blocked_submissions"] += 1
                    return False
                prior_delta = sum(
                    item[2] for item in route[:control["leg_index"]]
                    if item[3] == position
                )
                target = origin + prior_delta + delta
                allowed = (
                    control["listed_stage"] and control["described_stage"]
                    and set(control["initial"]) >= {"position.x", "position.y"}
                    and call.arguments.get("device") == DEVICE
                    and call.arguments.get("capability") == capability
                    and set(params) == {axis} and type(params.get(axis)) is int
                    and params.get(axis) == target and -2 <= target <= 2
                    and control["last_preflight"] == (capability, params)
                )
                if not allowed:
                    control["blocked_submissions"] += 1
                return allowed

            task = (
                "Please inspect the registered devices and the stage's described capabilities. "
                "Then make a tiny clockwise square on the simulated stage, one simulator "
                "coordinate per leg, starting from its current x/y and returning to that same "
                "position: +x, +y, -x, -y. Determine the read and single-axis movement "
                "capabilities from the description. Read both starting positions. Before every "
                "move, check that exact command; submit one move at a time; after each leg, read "
                "the changed coordinate and confirm it. Stop if a capability is missing, a "
                "preflight denies the move, or a readback differs. Report the discovered "
                "capabilities, each observed coordinate, and whether you returned to the start. "
                "These are simulator coordinates, not physical units. Do not use any other device."
            )

            run_result = Harness(
                guided_provider, [OPENMHS, "mcp"], env=agent_env,
                confirm=submit_guard, max_steps=24, max_seconds=240, on_step=on_step,
            ).run(task)

            trace = {
                "provider": provider.name, "model": provider.model,
                "stopped": run_result.stopped, "text": run_result.text,
                "steps": [{"tool": s.tool, "arguments": s.arguments, "is_error": s.is_error,
                           "held": s.held, "result": s.result} for s in run_result.transcript],
                "control": control,
                "usage": {"input_tokens": provider.input_tokens,
                          "output_tokens": provider.output_tokens,
                          "estimated_cost_usd": provider.estimated_cost_usd},
            }
            if (control["leg_index"] != len(route) or control["pending_readback"] is not None
                    or control["final"].get("position.x") != control["initial"].get("position.x")
                    or control["final"].get("position.y") != control["initial"].get("position.y")):
                trace["success"] = False
                (report_dir / "agent-trace.json").write_text(
                    json.dumps(trace, indent=2, default=str) + "\n", encoding="utf-8"
                )
                raise RuntimeError("the guided square did not complete and read back at its start")

            report = export_report.build(gateway.op, DEVICE)
            trace["success"] = report["audit"]["valid"] is True
            trace["audit"] = {"valid": report["audit"]["valid"],
                              "records": report["audit"]["records"]}
            (report_dir / "agent-trace.json").write_text(
                json.dumps(trace, indent=2, default=str) + "\n", encoding="utf-8"
            )
            (report_dir / "audit-report.json").write_text(
                json.dumps(report, indent=2, default=str) + "\n", encoding="utf-8"
            )
            (report_dir / "audit-report.md").write_text(
                export_report.to_markdown(report).rstrip() + "\n", encoding="utf-8"
            )
            run_info = {
                "date": time.strftime("%Y-%m-%d"), "simulator": simulator,
                "hardware_accessed": False, "model": provider.model,
                "reasoning_effort": "none", "task": "guided clockwise four-leg square and return",
                "start": control["initial"], "end": control["final"],
                "legs": control["legs"], "audit_valid": report["audit"]["valid"],
                "audit_records": report["audit"]["records"],
                "input_tokens": provider.input_tokens, "output_tokens": provider.output_tokens,
                "estimated_api_cost_usd": provider.estimated_cost_usd,
                "cumulative_estimated_cost_usd_including_prior_runs":
                    0.0017462 + provider.estimated_cost_usd,
                "configured_request_ceiling_usd": 0.47,
                "report": "audit-report.md", "trace": "agent-trace.json",
            }
            (report_dir / "run-info.json").write_text(
                json.dumps(run_info, indent=2) + "\n", encoding="utf-8"
            )
            print(f"\nGuided square completed and returned to start. Audit valid: {report['audit']['valid']}")
            print(f"Trace: {report_dir / 'agent-trace.json'}")
            return 0 if report["audit"]["valid"] else 1
        finally:
            gateway.stop()
    finally:
        proxy.stop()


if __name__ == "__main__":
    raise SystemExit(run())
