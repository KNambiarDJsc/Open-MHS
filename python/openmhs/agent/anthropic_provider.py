"""Claude as a harness provider, through the official ``anthropic`` SDK.

Optional: ``pip install anthropic``. Credentials are the SDK's own (``ANTHROPIC_API_KEY``,
or a profile from ``ant auth login``); this module never reads or stores them.

The conversation is append-only: each response's ``content`` (thinking and fallback blocks
included) is sent back unchanged, and all of a turn's tool results go back in one user
message.
"""

from __future__ import annotations

from typing import Any

from .providers import Conversation, ToolCall, ToolOutcome, ToolSpec, Turn

DEFAULT_MODEL = "claude-opus-5-5"


class _ClaudeConversation:
    def __init__(self, p: AnthropicProvider, system: str, task: str,
                 tools: list[ToolSpec]) -> None:
        self._p = p
        self._system = system
        self._tools = [
            {"name": t.name, "description": t.description, "input_schema": t.input_schema}
            for t in tools
        ]
        self._messages: list[dict[str, Any]] = [{"role": "user", "content": task}]

    def _create(self) -> Any:
        kwargs: dict[str, Any] = {
            "model": self._p.model,
            "max_tokens": self._p.max_tokens,
            "system": self._system,
            "tools": self._tools,
            "messages": self._messages,
            "output_config": {"effort": self._p.effort},
        }
        if self._p.cache:
            # The tool list and system prompt are the same every turn: cache them.
            kwargs["cache_control"] = {"type": "ephemeral"}
        if self._p.fallbacks:
            # On a safeguard decline the API re-runs the request on a fallback model.
            kwargs["betas"] = ["server-side-fallback-2026-07-01"]
            kwargs["fallbacks"] = "default"
            return self._p.client.beta.messages.create(**kwargs)
        return self._p.client.messages.create(**kwargs)

    def next_turn(self) -> Turn:
        while True:
            response = self._create()
            self._messages.append({"role": "assistant", "content": response.content})
            if response.stop_reason != "pause_turn":
                break
        text = "".join(b.text for b in response.content if getattr(b, "type", "") == "text")
        calls = [
            ToolCall(id=b.id, name=b.name, arguments=dict(b.input))
            for b in response.content
            if getattr(b, "type", "") == "tool_use"
        ]
        stop = {
            "tool_use": "tools",
            "end_turn": "done",
            "stop_sequence": "done",
            "max_tokens": "limit",
            "refusal": "refused",
        }.get(response.stop_reason, "done")
        if calls and stop != "refused":
            stop = "tools"
        return Turn(text=text, calls=calls, stop=stop)

    def add_results(self, outcomes: list[ToolOutcome]) -> None:
        self._messages.append({
            "role": "user",
            "content": [
                {"type": "tool_result", "tool_use_id": o.call.id, "content": o.content,
                 **({"is_error": True} if o.is_error else {})}
                for o in outcomes
            ],
        })


class AnthropicProvider:
    """Claude via the Messages API.

    ``effort`` trades thoroughness for cost (``low`` to ``max``). ``fallbacks`` turns on the
    server-side refusal fallback (Claude API only; set it to False on Bedrock, Vertex AI or
    Foundry). ``cache`` caches the tools and system prompt across turns.
    """

    def __init__(self, model: str = DEFAULT_MODEL, *, effort: str = "high",
                 max_tokens: int = 16_000, fallbacks: bool = True, cache: bool = True,
                 client: Any = None) -> None:
        if client is None:
            try:
                import anthropic
            except ImportError as e:  # pragma: no cover - depends on the environment
                raise RuntimeError(
                    "AnthropicProvider needs the anthropic package: pip install anthropic"
                ) from e
            client = anthropic.Anthropic()
        self.client = client
        self.model = model
        self.effort = effort
        self.max_tokens = max_tokens
        self.fallbacks = fallbacks
        self.cache = cache
        self.name = f"anthropic:{model}"

    def start(self, system: str, task: str, tools: list[ToolSpec]) -> Conversation:
        return _ClaudeConversation(self, system, task, tools)
