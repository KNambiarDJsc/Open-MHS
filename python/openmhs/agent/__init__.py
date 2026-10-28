"""A model-agnostic agent harness for OpenMHS (docs/agent-harness.md).

The model proposes; the gateway decides; the harness keeps the loop honest. See
:class:`Harness` for what it enforces and :mod:`openmhs.agent.providers` for the model
interface. Independent project; not affiliated with Anthropic.
"""

from .harness import HARNESS_RULES, Harness, RunResult, Step, tool_specs
from .mcp import McpError, McpStdioClient, ToolResult
from .providers import (
    Conversation,
    ModelProvider,
    OpenAICompatibleProvider,
    ScriptedProvider,
    ToolCall,
    ToolOutcome,
    ToolSpec,
    Turn,
)

__all__ = [
    "HARNESS_RULES",
    "Conversation",
    "Harness",
    "McpError",
    "McpStdioClient",
    "ModelProvider",
    "OpenAICompatibleProvider",
    "RunResult",
    "ScriptedProvider",
    "Step",
    "ToolCall",
    "ToolOutcome",
    "ToolResult",
    "ToolSpec",
    "Turn",
    "tool_specs",
]
