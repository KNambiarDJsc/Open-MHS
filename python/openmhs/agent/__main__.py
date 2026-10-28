"""Run a task with a model of your choice: ``python -m openmhs.agent "set the supply to 5 V"``.

    python -m openmhs.agent --sim "set the bench supply to 5 V and confirm"
    python -m openmhs.agent --provider openai --base-url http://localhost:11434/v1 \\
        --model llama3.1 --sim "what devices are there?"

``--sim`` runs against a throwaway virtual lab (``openmhs mcp --sim``). Without it, the
harness starts ``openmhs mcp`` with your ``OPENMHS_TOKEN`` (an agent token). ``--confirm``
asks you at the terminal before each call that may change the physical world.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys

from .harness import Harness, Step
from .providers import OpenAICompatibleProvider, ToolCall


def main() -> int:
    ap = argparse.ArgumentParser(prog="python -m openmhs.agent", description=__doc__.split("\n\n")[0])
    ap.add_argument("task")
    ap.add_argument("--provider", choices=["anthropic", "openai"], default="anthropic")
    ap.add_argument("--model", help="model id (default: claude-opus-5-5 for anthropic)")
    ap.add_argument("--base-url", help="OpenAI-compatible server, e.g. http://localhost:11434/v1")
    ap.add_argument("--sim", action="store_true", help="use a throwaway virtual lab")
    ap.add_argument("--confirm", action="store_true",
                    help="ask before each call that may change the physical world")
    ap.add_argument("--max-steps", type=int, default=40)
    ap.add_argument("--openmhs", default=os.environ.get("OPENMHS_BIN") or shutil.which("openmhs")
                    or "openmhs", help="the openmhs binary")
    args = ap.parse_args()

    if args.provider == "openai":
        if not args.base_url or not args.model:
            ap.error("--provider openai needs --base-url and --model")
        provider = OpenAICompatibleProvider(args.base_url, args.model)
    else:
        from .anthropic_provider import DEFAULT_MODEL, AnthropicProvider
        provider = AnthropicProvider(args.model or DEFAULT_MODEL)

    command = [args.openmhs, "mcp"] + (["--sim"] if args.sim else [])
    if not args.sim and not os.environ.get("OPENMHS_TOKEN"):
        ap.error("set OPENMHS_TOKEN to an agent token, or use --sim")

    def confirm(call: ToolCall) -> bool:
        print(f"\nThe model wants to call {call.name} {json.dumps(call.arguments)}")
        return input("Send it? [y/N] ").strip().lower() == "y"

    def show(step: Step) -> None:
        mark = "held" if step.held else ("refused" if step.is_error else "ok")
        print(f"  [{mark}] {step.tool} {json.dumps(step.arguments)}", flush=True)

    harness = Harness(provider, command, confirm=confirm if args.confirm else None,
                      max_steps=args.max_steps, on_step=show)
    result = harness.run(args.task)
    print(f"\n{result.text}\n\n(stopped: {result.stopped}; {len(result.transcript)} tool calls; "
          f"{len(result.confirmed_commands())} confirmed by readback; provider {result.provider})")
    return 0 if result.stopped == "done" else 1


if __name__ == "__main__":
    sys.exit(main())
