"""Model providers for the agent harness: one interface, any model.

A provider holds one conversation in its own wire format and only ever appends to it, so
nothing a model returned (thinking blocks, call ids) is rewritten between turns. The
harness sees three things: the text of a turn, the tool calls it asks for, and why it
stopped.

- :class:`ScriptedProvider`: deterministic, for tests and demos. No model, no key.
- :class:`OpenAICompatibleProvider`: any server speaking the OpenAI chat-completions
  format, which covers local models served by llama.cpp, vLLM or Ollama. Standard library.
- :class:`openmhs.agent.anthropic_provider.AnthropicProvider`: Claude, through the official
  ``anthropic`` SDK (optional dependency).

A provider never executes anything. It proposes calls; the harness decides what to do
with them, and the gateway decides what is allowed.
"""

from __future__ import annotations

import json
import os
import urllib.request
from dataclasses import dataclass, field
from typing import Any, Callable, Protocol


@dataclass
class ToolSpec:
    """A tool as the model sees it: from the gateway's MCP ``tools/list``."""

    name: str
    description: str
    input_schema: dict[str, Any]
    #: The tool may change the physical world (MCP ``destructiveHint``).
    physical: bool = False
    read_only: bool = False


@dataclass
class ToolCall:
    id: str
    name: str
    arguments: dict[str, Any]


@dataclass
class ToolOutcome:
    """What the harness sends back for one call."""

    call: ToolCall
    content: str
    is_error: bool = False


@dataclass
class Turn:
    text: str = ""
    calls: list[ToolCall] = field(default_factory=list)
    #: ``tools`` (wants results), ``done``, ``refused`` (declined by the provider's
    #: safeguards) or ``limit`` (ran out of output tokens).
    stop: str = "done"


class Conversation(Protocol):
    def next_turn(self) -> Turn:
        """Ask the model for its next turn."""

    def add_results(self, outcomes: list[ToolOutcome]) -> None:
        """Append the results of the calls of the last turn, all in one message."""


class ModelProvider(Protocol):
    name: str

    def start(self, system: str, task: str, tools: list[ToolSpec]) -> Conversation:
        """Begin a conversation about ``task`` with these tools available."""


# ---------------------------------------------------------------------- scripted


class _ScriptedConversation:
    def __init__(self, script: Callable[[list[ToolOutcome]], Turn]) -> None:
        self._script = script
        self._last: list[ToolOutcome] = []

    def next_turn(self) -> Turn:
        return self._script(self._last)

    def add_results(self, outcomes: list[ToolOutcome]) -> None:
        self._last = outcomes


class ScriptedProvider:
    """A provider whose turns come from a function of the previous results.

    ``policy(task, tools)`` returns that function. It sees only what a model would: the
    results of its own calls. Used in tests so that a scenario runs the same way every time,
    through the real gateway.
    """

    name = "scripted"

    def __init__(
        self,
        policy: Callable[[str, list[ToolSpec]], Callable[[list[ToolOutcome]], Turn]],
    ) -> None:
        self._policy = policy

    def start(self, system: str, task: str, tools: list[ToolSpec]) -> Conversation:
        return _ScriptedConversation(self._policy(task, tools))


# ---------------------------------------------------------------- OpenAI-compatible


class _ChatCompletionsConversation:
    def __init__(self, provider: OpenAICompatibleProvider, system: str, task: str,
                 tools: list[ToolSpec]) -> None:
        self._p = provider
        self._messages: list[dict[str, Any]] = [
            {"role": "system", "content": system},
            {"role": "user", "content": task},
        ]
        self._tools = [
            {"type": "function",
             "function": {"name": t.name, "description": t.description,
                          "parameters": t.input_schema}}
            for t in tools
        ]

    def next_turn(self) -> Turn:
        body = {"model": self._p.model, "messages": self._messages, "tools": self._tools}
        reply = self._p._post("/chat/completions", body)
        choice = reply["choices"][0]
        message = choice["message"]
        self._messages.append(message)  # as returned: call ids must match the results
        calls = []
        for c in message.get("tool_calls") or []:
            raw = c["function"].get("arguments") or "{}"
            try:
                args = json.loads(raw) if isinstance(raw, str) else dict(raw)
            except ValueError:
                args = {"__invalid_json__": raw}
            calls.append(ToolCall(id=c["id"], name=c["function"]["name"], arguments=args))
        finish = choice.get("finish_reason")
        stop = "tools" if calls else ("limit" if finish == "length" else "done")
        return Turn(text=message.get("content") or "", calls=calls, stop=stop)

    def add_results(self, outcomes: list[ToolOutcome]) -> None:
        for o in outcomes:
            self._messages.append(
                {"role": "tool", "tool_call_id": o.call.id, "content": o.content})


class OpenAICompatibleProvider:
    """Any server with an OpenAI-style ``POST {base_url}/chat/completions``.

    ``base_url`` is for example ``http://localhost:11434/v1`` (Ollama) or
    ``http://localhost:8000/v1`` (vLLM). ``api_key`` defaults to ``$OPENAI_API_KEY`` and may
    be empty for local servers.
    """

    def __init__(self, base_url: str, model: str, api_key: str | None = None,
                 timeout: float = 300) -> None:
        self.base_url = base_url.rstrip("/")
        self.model = model
        self.name = f"openai-compatible:{model}"
        self._key = api_key if api_key is not None else os.environ.get("OPENAI_API_KEY", "")
        self._timeout = timeout

    def _post(self, path: str, body: dict[str, Any]) -> dict[str, Any]:
        req = urllib.request.Request(
            self.base_url + path, data=json.dumps(body).encode("utf-8"), method="POST",
            headers={"Content-Type": "application/json",
                     **({"Authorization": f"Bearer {self._key}"} if self._key else {})},
        )
        with urllib.request.urlopen(req, timeout=self._timeout) as resp:  # noqa: S310
            return json.loads(resp.read().decode("utf-8"))

    def start(self, system: str, task: str, tools: list[ToolSpec]) -> Conversation:
        return _ChatCompletionsConversation(self, system, task, tools)
