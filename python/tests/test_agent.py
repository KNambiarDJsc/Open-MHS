"""The agent harness against a real gateway (``openmhs mcp --sim``).

Every tool call goes through the real MCP server and gateway. Only the model is replaced:
by a script, by a fake OpenAI-compatible server, or by a fake Anthropic client that records
the requests it was sent. None of these tests calls a real model or needs a key.
"""

from __future__ import annotations

import http.server
import json
import os
import threading
import time
import unittest
from types import SimpleNamespace
from typing import Any

from support import binary

from openmhs.agent import (
    Harness,
    OpenAICompatibleProvider,
    ScriptedProvider,
    ToolCall,
    ToolOutcome,
    Turn,
)
from openmhs.agent.anthropic_provider import AnthropicProvider

PSU = "sim.psu-01"
SETPOINT = "power.output.voltage.setpoint"


def lab() -> tuple[list[str], dict[str, str]]:
    env = {k: v for k, v in os.environ.items() if k not in ("OPENMHS_TOKEN", "OPENMHS_HOME")}
    return [str(binary()), "mcp", "--sim"], env


def volts(v: float) -> dict[str, Any]:
    return {"value": v, "unit": "V"}


def call(name: str, **args: Any) -> Turn:
    return Turn(calls=[ToolCall(id=f"c-{name}-{time.monotonic_ns()}", name=name,
                                arguments=args)], stop="tools")


def done(text: str = "done") -> Turn:
    return Turn(text=text, stop="done")


def result(outcomes: list[ToolOutcome]) -> dict[str, Any]:
    return json.loads(outcomes[0].content)


class Scripted(unittest.TestCase):
    def run_script(self, script, **kw):
        cmd, env = lab()
        provider = ScriptedProvider(lambda task, tools: script)
        return Harness(provider, cmd, env=env, **kw).run("set the bench supply to 5 V")

    def test_a_refusal_is_replanned_from_its_numbers_and_the_run_becomes_a_plan(self) -> None:
        state: dict[str, Any] = {"phase": "jump"}

        def script(last: list[ToolOutcome]) -> Turn:
            if state["phase"] == "jump":
                state["phase"] = "read-refusal"
                return call("submit_command", device=PSU, capability=SETPOINT,
                            value=volts(5), idempotency_key="j")
            if state["phase"] == "read-refusal":
                r = result(last)
                self.assertFalse(r["reached_device"])
                state["step"] = r["reasons"][0]["details"]["max_change"]
                state["at"], state["phase"] = 0.0, "ramp"
            if state["phase"] == "ramp":
                if state["at"] >= 5:
                    return done("at 5 V")
                time.sleep(1.1)  # the slew window
                state["at"] = min(5.0, state["at"] + state["step"])
                return call("submit_command", device=PSU, capability=SETPOINT,
                            value=volts(state["at"]), idempotency_key=f"r{state['at']}")
            raise AssertionError(state)

        r = self.run_script(script)
        self.assertEqual(r.stopped, "done", r)
        self.assertEqual([s.arguments["value"]["value"] for s in r.confirmed_commands()],
                         [2, 4, 5])
        first = r.calls("submit_command")[0]
        self.assertTrue(first.is_error)
        self.assertIsNone(first.held, "the gateway refused it, not the harness")
        plan = r.proposed_plan("ramp the supply to 5 V")
        self.assertEqual([s["id"] for s in plan["steps"]], ["step-1", "step-2", "step-3"])
        self.assertEqual(plan["steps"][2]["after"], ["step-2"])
        self.assertNotIn("idempotency_key", json.dumps(plan))

    def test_the_gateway_not_the_harness_refuses_what_was_not_granted(self) -> None:
        steps = iter([call("submit_command", device="sim.axis-01", capability="axis.home",
                           idempotency_key="h"), done()])
        r = self.run_script(lambda last: next(steps))
        s = r.calls("submit_command")[0]
        self.assertTrue(s.is_error)
        self.assertIsNone(s.held)
        # The --sim credential is scoped to the supply, so the scope ceiling refuses it
        # before any grant is looked at.
        self.assertEqual(s.result["reasons"][0]["code"], "auth.scope.denied")

    def test_a_person_can_decline_a_physical_call_and_nothing_is_sent(self) -> None:
        asked: list[str] = []
        steps = iter([
            call("submit_command", device=PSU, capability=SETPOINT, value=volts(1),
                 idempotency_key="d"),
            call("read_state", device=PSU, capability=SETPOINT),
            done(),
        ])

        def confirm(c: ToolCall) -> bool:
            asked.append(c.name)
            return False

        r = self.run_script(lambda last: next(steps), confirm=confirm)
        self.assertEqual(asked, ["submit_command"], "read-only tools are not held")
        self.assertEqual(r.calls("submit_command")[0].held, "a person declined this call")
        self.assertEqual(r.calls("read_state")[0].result["value"]["value"], 0.0)

    def test_a_plan_is_checked_before_it_is_submitted(self) -> None:
        def plan(v: float) -> dict[str, Any]:
            return {"plan": {"description": "set the supply", "steps": [
                {"id": "set", "request": {"target": {"device": PSU, "capability": SETPOINT},
                                          "operation": "write", "value": volts(v)}}]}}

        steps = iter([call("submit_plan", **plan(48)), call("submit_plan", **plan(1)), done()])
        r = self.run_script(lambda last: next(steps))
        bad, good = r.calls("submit_plan")
        self.assertEqual(bad.held, "check_plan did not pass")
        self.assertFalse(bad.result["check_plan"]["ok"])
        self.assertIsNone(good.held)
        self.assertFalse(good.is_error, good.result)

    def test_budgets_stop_a_model_that_never_finishes(self) -> None:
        r = self.run_script(lambda last: call("list_devices"), max_steps=3)
        self.assertEqual(r.stopped, "max_steps")
        self.assertEqual(len(r.transcript), 3)


class _FakeChatServer(http.server.BaseHTTPRequestHandler):
    requests: list[dict[str, Any]] = []

    def do_POST(self) -> None:  # noqa: N802
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        type(self).requests.append(body)
        if len(type(self).requests) == 1:
            message = {"role": "assistant", "content": None, "tool_calls": [
                {"id": "call-1", "type": "function",
                 "function": {"name": "list_devices", "arguments": "{}"}}]}
            finish = "tool_calls"
        else:
            message = {"role": "assistant", "content": "three devices"}
            finish = "stop"
        out = json.dumps({"choices": [{"message": message, "finish_reason": finish}]}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)

    def log_message(self, *args: Any) -> None:
        pass


class OpenAICompatible(unittest.TestCase):
    def test_a_chat_completions_server_drives_the_same_tools(self) -> None:
        _FakeChatServer.requests = []
        server = http.server.HTTPServer(("127.0.0.1", 0), _FakeChatServer)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            provider = OpenAICompatibleProvider(
                f"http://127.0.0.1:{server.server_port}/v1", "local-model", api_key="")
            cmd, env = lab()
            r = Harness(provider, cmd, env=env).run("what devices are there?")
        finally:
            server.shutdown()
        self.assertEqual(r.stopped, "done")
        self.assertEqual(r.text, "three devices")
        first, second = _FakeChatServer.requests
        names = {t["function"]["name"] for t in first["tools"]}
        self.assertIn("submit_command", names)
        self.assertTrue(all(t["type"] == "function" for t in first["tools"]))
        self.assertEqual(first["messages"][0]["role"], "system")
        tool_msg = second["messages"][-1]
        self.assertEqual((tool_msg["role"], tool_msg["tool_call_id"]), ("tool", "call-1"))
        self.assertEqual(len(json.loads(tool_msg["content"])["devices"]), 3)


class _FakeAnthropic:
    """Records each request; answers with a list_devices call, a pause, then a summary."""

    def __init__(self) -> None:
        self.requests: list[dict[str, Any]] = []
        self.beta = SimpleNamespace(messages=SimpleNamespace(create=self.create))
        self.messages = SimpleNamespace(create=self.create)

    def create(self, **kw: Any) -> Any:
        self.requests.append(json.loads(json.dumps(kw, default=lambda o: o.__dict__)))
        n = len(self.requests)
        if n == 1:
            return SimpleNamespace(stop_reason="tool_use", content=[
                SimpleNamespace(type="text", text="Looking."),
                SimpleNamespace(type="tool_use", id="toolu_1", name="list_devices", input={})])
        if n == 2:
            return SimpleNamespace(stop_reason="pause_turn", content=[])
        return SimpleNamespace(stop_reason="end_turn",
                               content=[SimpleNamespace(type="text", text="Three devices.")])


class Anthropic(unittest.TestCase):
    def test_requests_follow_the_messages_api_and_history_is_append_only(self) -> None:
        fake = _FakeAnthropic()
        cmd, env = lab()
        r = Harness(AnthropicProvider(client=fake), cmd, env=env).run("what devices are there?")
        self.assertEqual(r.stopped, "done")
        self.assertEqual(r.text, "Three devices.")
        first, second, third = fake.requests
        self.assertEqual(first["model"], "claude-opus-5-5")
        self.assertEqual(first["fallbacks"], "default")
        self.assertEqual(first["betas"], ["server-side-fallback-2026-07-01"])
        self.assertEqual(first["output_config"], {"effort": "high"})
        self.assertTrue(all({"name", "description", "input_schema"} == set(t)
                            for t in first["tools"]))
        self.assertTrue(all("$schema" not in t["input_schema"] for t in first["tools"]))
        results = second["messages"][-1]
        self.assertEqual(results["role"], "user")
        self.assertEqual(results["content"][0]["type"], "tool_result")
        self.assertEqual(results["content"][0]["tool_use_id"], "toolu_1")
        # Earlier turns are resent exactly as they were.
        self.assertEqual(second["messages"][:2], third["messages"][:2])

    def test_a_refusal_ends_the_run_without_running_its_calls(self) -> None:
        fake = _FakeAnthropic()
        fake.create = lambda **kw: SimpleNamespace(stop_reason="refusal", content=[
            SimpleNamespace(type="tool_use", id="t", name="submit_command", input={})])
        fake.beta.messages.create = fake.create
        cmd, env = lab()
        r = Harness(AnthropicProvider(client=fake), cmd, env=env).run("anything")
        self.assertEqual(r.stopped, "refused")
        self.assertEqual(r.transcript, [])


if __name__ == "__main__":
    unittest.main()
