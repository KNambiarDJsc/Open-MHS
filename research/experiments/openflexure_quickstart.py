"""Run a bounded OpenFlexure simulator task through the real OpenMHS MCP path.

Start the OpenFlexure simulator separately, then run this script from the repository root:

    python research/experiments/openflexure_quickstart.py

Use ``--drop-polls`` to lose the LabThings invocation response after acceptance, then
reconcile by readback. This script accepts only loopback URLs and labels its run as a
simulator. It never falls back to a physical device.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [
    str(ROOT / "python"),
    str(ROOT / "adapters" / "python" / "wot"),
    str(ROOT / "research" / "experiments"),
]

import export_report  # noqa: E402
import import_td  # noqa: E402
import td_import  # noqa: E402
from lab import FaultProxy, Gateway, OPENMHS, WORK, http_json  # noqa: E402
from openmhs.agent import (  # noqa: E402
    Harness, OpenAICompatibleProvider, ScriptedProvider, ToolCall, Turn,
)

DEVICE = "ofm.stage"
BASE_CAPABILITY = "move_x"
POSITION_CAPABILITY = "position.x"
BOUND = 100


class ToolLimitedProvider:
    """Expose only the stage read, preflight, and submit tools to the model."""

    ALLOWED = {"read_state", "check_command", "submit_command"}

    def __init__(self, provider):
        self.provider = provider
        self.name = provider.name

    def start(self, system, task, tools):
        return self.provider.start(
            system, task, [tool for tool in tools if tool.name in self.ALLOWED]
        )


def stage_override(bound: int = BOUND) -> dict:
    """An experiment-only contract. OpenFlexure's TD has no units or stage limits."""
    return {
        "manufacturer": "OpenFlexure (simulator)",
        "model": "DummyStage",
        "actions": {
            "move_absolute": {
                "expose": True,
                "side_effect": "reversible",
                "idempotent": False,
                "timing": {"expected_ms": 1000, "timeout_ms": 30000},
                "omit_params": ["backlash_compensation", "block_cancellation"],
                "reason": (
                    f"Experiment-only bound |x| <= {bound}; it is not a device or safety limit"
                ),
                "variants": {
                    BASE_CAPABILITY: {
                        "params": {
                            "x": {
                                "type": "integer",
                                "range": {"min": -bound, "max": bound},
                            }
                        },
                        "verification": {
                            "mode": "readback",
                            "capability": POSITION_CAPABILITY,
                            "expect_param": "x",
                            "tolerance": {"abs": 0},
                            "settle_ms": 100,
                            "timeout_ms": 5000,
                        },
                    }
                },
            }
        },
    }


def scripted_provider(*, drop_polls: bool = False) -> ScriptedProvider:
    """A deterministic agent exercising MCP tools without sending a model API request."""
    phase = 0
    target_x = -50 if drop_polls else 50

    def script(last):
        nonlocal phase
        # Do not send the bounded simulator action unless the deliberately invalid
        # millimetre quantity was explicitly denied by the gateway's preflight.
        if phase == 4:
            if len(last) != 1 or last[0].is_error:
                return Turn(
                    text="Stopped: the unit-check result was unavailable or errored; no move submitted.",
                    stop="done",
                )
            try:
                check = json.loads(last[0].content)
            except (TypeError, json.JSONDecodeError):
                check = {}
            if check.get("decision") != "deny":
                return Turn(
                    text="Stopped: OpenMHS did not deny the undeclared mm unit; no move submitted.",
                    stop="done",
                )
        calls = [
            ("list_devices", {}),
            ("describe_device", {"device": DEVICE}),
            ("read_state", {"device": DEVICE, "capability": POSITION_CAPABILITY}),
            # An explicit mm quantity must be refused: the source TD declares no unit.
            ("check_command", {
                "device": DEVICE,
                "capability": BASE_CAPABILITY,
                "params": {"x": {"value": 50, "unit": "mm"}},
            }),
            ("submit_command", {
                "device": DEVICE,
                "capability": BASE_CAPABILITY,
                "params": {"x": target_x},
                "idempotency_key": (
                    "openflexure-demo-move-x-minus-50-drop-polls"
                    if drop_polls else "openflexure-demo-move-x-50"
                ),
                "deadline_ms": 45000,
            }),
        ]
        if phase >= len(calls):
            return Turn(text="Finished the one bounded simulator request.", stop="done")
        name, arguments = calls[phase]
        phase += 1
        return Turn(
            calls=[ToolCall(id=f"openflexure-demo-{phase}", name=name, arguments=arguments)],
            stop="tools",
        )

    return ScriptedProvider(lambda _task, _tools: script)


def find_command_id(value) -> str | None:
    if isinstance(value, dict):
        command_id = value.get("command_id")
        if isinstance(command_id, str):
            return command_id
        for child in value.values():
            found = find_command_id(child)
            if found:
                return found
    return None


def state_name(receipt) -> str:
    state = getattr(receipt, "state", None)
    return str(getattr(state, "value", state))


def loopback_url(url: str) -> bool:
    parts = urlsplit(url)
    return parts.scheme == "http" and parts.hostname in {"127.0.0.1", "localhost", "::1"}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", default="http://127.0.0.1:5000")
    parser.add_argument("--drop-polls", action="store_true",
                        help="reset invocation polls after the action is accepted")
    parser.add_argument("--agent-model",
                        help="use an OpenAI-compatible model instead of the deterministic provider")
    parser.add_argument("--agent-base-url", default="http://127.0.0.1:11435/v1",
                        help="model URL (loopback or official OpenAI API; default is local Ollama)")
    parser.add_argument("--autonomous-simulator", action="store_true",
                        help="allow one tightly bounded model-selected move in the simulator without an operator approval pause")
    args = parser.parse_args()
    if not loopback_url(args.base_url):
        parser.error("the demo only connects to an HTTP loopback simulator URL")
    if args.agent_model:
        if args.drop_polls:
            parser.error("--drop-polls is a separate deterministic scenario")
        if (not loopback_url(args.agent_base_url)
                and args.agent_base_url != "https://api.openai.com/v1"):
            parser.error("the agent model endpoint must be loopback or the official OpenAI API")
        target_x = None
        scenario_name = "agent-autonomous" if args.autonomous_simulator else "agent-model"
    else:
        if args.autonomous_simulator:
            parser.error("--autonomous-simulator requires --agent-model")
        target_x = -50 if args.drop_polls else 50
        scenario_name = "drop-polls" if args.drop_polls else "normal"

    action_bound = 2 if args.autonomous_simulator else BOUND
    model_target_bound = 2 if args.autonomous_simulator else 20

    api = args.base_url.rstrip("/") + "/api/v3"
    proxy = FaultProxy(upstream=args.base_url)
    if args.drop_polls:
        proxy.add(r"GET /api/v3/action_invocations/", "reset", 99)
    proxy_base = proxy.base + "/"

    run_dir = WORK / "openflexure-quickstart"
    scenario_dir = run_dir / scenario_name
    scenario_dir.mkdir(parents=True, exist_ok=True)
    manifest_dir = run_dir / "adapter"
    manifest_dir.mkdir(parents=True, exist_ok=True)
    tds = http_json("GET", f"{api}/thing_descriptions/")[1]
    if not isinstance(tds, dict) or "stage" not in tds:
        raise RuntimeError("the simulator did not return a stage Thing Description")

    samples = {}
    for name, prop in (tds["stage"].get("properties") or {}).items():
        if prop.get("type") == "object" and isinstance(prop.get("additionalProperties"), dict):
            form = td_import._form(prop.get("forms"), "readproperty")
            if form:
                status, value = http_json("GET", args.base_url.rstrip("/") + form["href"])
                if status == 200 and isinstance(value, dict):
                    samples[name] = value

    mapping = td_import.map_thing(
        tds["stage"], base_url=proxy_base, device_id=DEVICE,
        overrides=stage_override(action_bound), samples=samples,
    )
    if not any(c["name"] == BASE_CAPABILITY for c in mapping.descriptor["capabilities"]):
        raise RuntimeError("the reviewed move_x capability was not generated")
    manifest = import_td.build_manifest(
        mapping, proxy_base, manifest_dir, source="simulator", timeout_ms=2000, stamp="received"
    )
    manifest_path = manifest_dir / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    gateway_home = run_dir / {
        "normal": "gateway",
        "drop-polls": "gateway-drop-polls",
        "agent-model": "gateway-agent-model",
        "agent-autonomous": "gateway-agent-autonomous",
    }[scenario_name]
    gateway = Gateway(gateway_home, manifest_path, grants=False).start()
    try:
        for grant in (
            {
                "id": "demo-stage-read", "principal": "agent:demo", "device": DEVICE,
                "capability": "*", "operations": ["read"], "effect": "allow",
            },
            {
                "id": "demo-stage-approval", "principal": "agent:demo", "device": DEVICE,
                "capability": BASE_CAPABILITY, "operations": ["invoke"],
                "effect": "allow" if args.autonomous_simulator else "require_approval",
            },
        ):
            try:
                gateway.op.add_grant(grant)
            except Exception as exc:
                if getattr(exc, "code", None) != "policy.grant.duplicate":
                    raise
        gateway.token = gateway.op.create_token("agent:demo", any_scope=True).token
        agent_env = {
            **os.environ,
            "OPENMHS_HOME": str(gateway.home),
            "OPENMHS_TOKEN": gateway.token,
        }
        openai_api = args.agent_base_url == "https://api.openai.com/v1"
        provider = (
            OpenAICompatibleProvider(
                args.agent_base_url, args.agent_model, timeout=90, retries=0,
                max_tokens=None if openai_api else 512,
                max_completion_tokens=512 if openai_api else None,
                reasoning_effort="none" if openai_api else None,
                options=None if openai_api else {"num_ctx": 4096, "num_predict": 512},
                # The previous approved model smoke test used about $0.00076. Keep this
                # follow-up below the user's $0.50 cumulative ceiling with extra headroom.
                budget_usd=0.49 if openai_api and args.autonomous_simulator
                else 0.50 if openai_api else None,
                input_price_per_million=0.10 if openai_api else None,
                output_price_per_million=0.50 if openai_api else None,
            )
            if args.agent_model else scripted_provider(drop_polls=args.drop_polls)
        )
        if args.agent_model:
            provider = ToolLimitedProvider(provider)
        model_provider = provider.provider if isinstance(provider, ToolLimitedProvider) else provider
        task = (
            "Control the simulated OpenFlexure stage to advance x by exactly one simulator "
            "coordinate from its current position. First call read_state for ofm.stage and "
            "position.x. Compute the next integer x, keeping it within -2..2. Use only move_x. "
            "Call check_command with that exact integer, then submit exactly once with an "
            "idempotency key. After submission, call read_state again and confirm the new x "
            "is exactly one greater than the initial reading. These are simulator coordinates, "
            "not millimetres. Stop after the readback."
            if args.autonomous_simulator else
            "First call read_state for device ofm.stage and capability position.x. Then "
            "choose one integer x target strictly greater than the current x, within -20..20 "
            "simulator coordinates. Use only move_x. Call check_command with that exact plain "
            "integer before submitting it, then submit exactly once with an idempotency key. "
            "Do not call list_devices or describe coordinates as millimetres. Stop after the "
            "one proposal."
            if args.agent_model else
            "Inspect the simulated stage; check the units and limits; request one bounded target only."
        )
        control = {"initial_x": None, "checked_params": None, "submit_count": 0}

        def on_step(step):
            if (args.autonomous_simulator and step.tool == "read_state"
                    and step.arguments.get("device") == DEVICE
                    and step.arguments.get("capability") == POSITION_CAPABILITY
                    and isinstance(step.result, dict)
                    and isinstance(step.result.get("value"), (int, float))):
                control["initial_x"] = step.result["value"]
            if (args.autonomous_simulator and step.tool == "check_command"
                    and step.arguments.get("device") == DEVICE
                    and step.arguments.get("capability") == BASE_CAPABILITY
                    and isinstance(step.result, dict)
                    and step.result.get("decision") != "deny"):
                control["checked_params"] = step.arguments.get("params")
            print(f"[{step.tool}] {json.dumps(step.result, default=str)[:1200]}", flush=True)

        def autonomous_submit_guard(call):
            params = call.arguments.get("params") or {}
            target = params.get("x")
            initial = control["initial_x"]
            valid = (
                control["submit_count"] == 0
                and call.arguments.get("device") == DEVICE
                and call.arguments.get("capability") == BASE_CAPABILITY
                and set(params) == {"x"}
                and type(target) is int
                and -model_target_bound <= target <= model_target_bound
                and isinstance(initial, (int, float))
                and target == initial + 1
                and control["checked_params"] == params
            )
            if valid:
                control["submit_count"] += 1
            return valid
        try:
            run = Harness(
                provider, [OPENMHS, "mcp"], env=agent_env,
                max_steps=8, max_seconds=180 if args.agent_model else 90,
                confirm=autonomous_submit_guard if args.autonomous_simulator else None,
                on_step=on_step,
            ).run(task)
        except Exception as exc:
            (scenario_dir / "agent-error.json").write_text(
                json.dumps({"provider": getattr(provider, "name", ""),
                            "error_type": type(exc).__name__, "error": str(exc),
                            "usage": {"input_tokens": getattr(model_provider, "input_tokens", 0),
                                      "output_tokens": getattr(model_provider, "output_tokens", 0),
                                      "estimated_cost_usd": getattr(model_provider, "estimated_cost_usd", 0.0)}},
                           indent=2) + "\n",
                encoding="utf-8",
            )
            raise
        trace = [{
            "tool": step.tool, "arguments": step.arguments, "is_error": step.is_error,
            "held": step.held, "result": step.result,
        } for step in run.transcript]
        (scenario_dir / "agent-trace.json").write_text(
            json.dumps({"stopped": run.stopped, "text": run.text, "steps": trace,
                        "usage": {"input_tokens": getattr(model_provider, "input_tokens", 0),
                                  "output_tokens": getattr(model_provider, "output_tokens", 0),
                                  "estimated_cost_usd": getattr(model_provider, "estimated_cost_usd", 0.0)}},
                       indent=2, default=str)
            + "\n", encoding="utf-8"
        )

        submitted = run.calls("submit_command")
        dispatched_submissions = [s for s in submitted if find_command_id(s.result)]
        if args.agent_model:
            # The model can propose only one change, and only inside a narrower review band
            # than the gateway descriptor. Cancel every pending proposal before failing closed.
            def cancel_pending_submissions() -> None:
                for step in submitted:
                    candidate = find_command_id(step.result)
                    if candidate:
                        receipt = gateway.op.command(candidate)
                        if state_name(receipt) == "AWAITING_APPROVAL":
                            gateway.op.cancel(candidate)

            if len(dispatched_submissions) != 1:
                cancel_pending_submissions()
                raise RuntimeError(
                    f"the model dispatched {len(dispatched_submissions)} submit calls; expected exactly one"
                )
            step = dispatched_submissions[0]
            arguments = step.arguments
            params = arguments.get("params") or {}
            target_x = params.get("x")
            initial = next((s.result.get("value") for s in run.calls("read_state")
                            if isinstance(s.result, dict) and s.result.get("capability") == POSITION_CAPABILITY), None)
            checked = [s for s in run.calls("check_command")
                       if s.arguments.get("device") == DEVICE
                       and s.arguments.get("capability") == BASE_CAPABILITY
                       and s.arguments.get("params") == params
                       and isinstance(s.result, dict)
                       and s.result.get("decision") != "deny"]
            if (
                arguments.get("device") != DEVICE
                or arguments.get("capability") != BASE_CAPABILITY
                or set(params) != {"x"}
                or type(target_x) is not int
                or not -20 <= target_x <= 20
                or not isinstance(initial, (int, float))
                or target_x <= initial
                or not checked
            ):
                cancel_pending_submissions()
                raise RuntimeError("the model proposal failed the one-axis, target, readback, or preflight guard")
            if args.autonomous_simulator and (
                target_x != initial + 1
                or not -model_target_bound <= target_x <= model_target_bound
                or control["submit_count"] != 1
            ):
                raise RuntimeError("the autonomous action did not match the one-step target guard")

        command_id = find_command_id(submitted[0].result) if submitted else None
        if not command_id:
            raise RuntimeError("MCP did not return a command id; see "
                               + str(scenario_dir / "agent-trace.json"))

        pending = gateway.op.command(command_id)
        operator_decision = "not_required_policy_allow" if args.autonomous_simulator else "not_required"
        if args.autonomous_simulator and state_name(pending) == "AWAITING_APPROVAL":
            gateway.op.cancel(command_id)
            operator_decision = "cancelled_unexpected_approval"
            print("Autonomous simulator policy unexpectedly required approval; cancelled safely.")
        elif state_name(pending) == "AWAITING_APPROVAL":
            print(f"\nOperator review: {DEVICE}/{BASE_CAPABILITY} with x={target_x}, bound ±{BOUND}.")
            if input("Approve this simulator-only move? [y/N] ").strip().lower() == "y":
                gateway.op.approve(command_id)
                operator_decision = "approved"
            else:
                gateway.op.cancel(command_id)
                operator_decision = "cancelled"

        receipt = gateway.op.command(command_id)
        end = time.monotonic() + 45
        while state_name(receipt) in {"AWAITING_APPROVAL", "DISPATCHED", "ACCEPTED", "APPLIED"}:
            if time.monotonic() >= end:
                break
            time.sleep(0.25)
            receipt = gateway.op.command(command_id)

        if args.drop_polls and state_name(receipt) == "UNKNOWN_OUTCOME":
            print("Invocation poll was lost after acceptance; waiting for the simulator job to settle.")
            proxy.clear()
            deadline = time.monotonic() + 45
            while time.monotonic() < deadline:
                status, moving = http_json("GET", f"{api}/stage/moving")
                if status == 200 and moving is False:
                    break
                time.sleep(0.25)
            receipt = gateway.op.reconcile(command_id)

        report = export_report.build(gateway.op, DEVICE)
        trace_path = scenario_dir / "agent-trace.json"
        trace_document = json.loads(trace_path.read_text(encoding="utf-8"))
        command_report = next((c for c in report.get("commands", [])
                               if c.get("command_id") == command_id), {})
        trace_document["execution"] = {
            "command_id": command_id,
            "operator_decision": operator_decision,
            "final_state": state_name(receipt),
            "verification": command_report.get("verification"),
            "audit_valid": report.get("audit", {}).get("valid"),
        }
        trace_path.write_text(json.dumps(trace_document, indent=2, default=str) + "\n",
                              encoding="utf-8")
        (scenario_dir / "audit-report.json").write_text(
            json.dumps(report, indent=2, default=str) + "\n", encoding="utf-8"
        )
        (scenario_dir / "audit-report.md").write_text(
            export_report.to_markdown(report).rstrip() + "\n", encoding="utf-8"
        )
        print(f"\nFinal command state: {state_name(receipt)}")
        print(f"Audit chain valid: {report['audit']['valid']}; report: {scenario_dir / 'audit-report.md'}")
        print(f"Agent trace: {scenario_dir / 'agent-trace.json'}")
        print("Simulator only. OpenFlexure declares no physical stage units or mechanical limits.")
        return 0 if report["audit"]["valid"] else 1
    finally:
        gateway.stop()
        proxy.stop()


if __name__ == "__main__":
    raise SystemExit(main())
