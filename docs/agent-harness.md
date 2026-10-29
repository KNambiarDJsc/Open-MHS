# Agent harness (any model)

`openmhs.agent` runs a task with a model of your choice against an OpenMHS gateway. The
model proposes tool calls, the gateway decides what is allowed, and the harness keeps the
loop honest. It is part of the Python package and has no dependencies except an optional
`anthropic` for Claude.

```sh
pip install ./python                     # and: pip install anthropic, for Claude
python -m openmhs.agent --sim "set the bench supply to 5 V and confirm it"
python -m openmhs.agent --provider openai --base-url http://localhost:11434/v1 \
    --model llama3.1 --sim "what devices are there?"
```

`--sim` uses a throwaway virtual lab (`openmhs mcp --sim`). Without it the harness starts
`openmhs mcp` with your `OPENMHS_TOKEN`, which must be an agent token. `--confirm` asks you
before each call that may change the physical world.

## Providers

| Provider | Talks to | Needs |
|---|---|---|
| `AnthropicProvider` | Claude through the official `anthropic` SDK. Default model `claude-opus-5-5`, effort `high`, server-side refusal fallback on, tools and system prompt cached | `pip install anthropic` and Anthropic credentials (`ANTHROPIC_API_KEY` or `ant auth login`) |
| `OpenAICompatibleProvider` | Any OpenAI-style `/chat/completions` server, including local models under llama.cpp, vLLM or Ollama | A URL and a model name; `OPENAI_API_KEY` if the server wants one |
| `ScriptedProvider` | A function you write. Used by the tests | Nothing |

A provider is one small interface (`start`, then `next_turn` and `add_results`). Adding
another means implementing those three methods. Conversations are append-only: what a
model returned is sent back unchanged.

## What the harness enforces

- The model sees only the tools the gateway lists, with their risk stated
  (`[CHANGES THE PHYSICAL WORLD]`, `[read-only]`).
- `confirm(call)` is asked before every call to a tool that may change the physical world.
  A declined call is not sent, and the model is told a person declined.
- `submit_plan` is preceded by `check_plan`. A plan that does not pass is not submitted.
- Steps and wall time are bounded (`max_steps`, `max_seconds`).
- Every call and its result are kept in `RunResult.transcript`.
- Nothing the model writes is executed as code.

The harness only ever narrows. Permission is the gateway's: a model asking for something
it may not do is refused by the gateway (the tests check that the refusal comes from there).
The harness holds no operator credential.

## Results and procedures

```python
from openmhs.agent import Harness
from openmhs.agent.anthropic_provider import AnthropicProvider

r = Harness(AnthropicProvider(), ["openmhs", "mcp", "--sim"]).run("set the supply to 5 V")
r.stopped                 # done, refused, limit, max_steps or max_seconds
r.confirmed_commands()    # commands the gateway confirmed by readback
r.proposed_plan("ramp the supply to 5 V")   # those commands as a plan, for a person to review
```

`proposed_plan` turns a successful run into a reusable plan (spec/plans.md). It is a draft:
the harness never submits it. A person reviews it, and it is checked like any other plan
when it runs.

## Tested with

`python/tests/test_agent.py`, all against a real gateway through `openmhs mcp --sim`:

- a scripted model replans from a slew refusal and ramps (2, 4, 5 V, each confirmed);
- the gateway, not the harness, refuses an action outside the credential;
- a person declines a call and nothing is sent;
- a plan that fails `check_plan` is held;
- a model that never finishes is stopped;
- an OpenAI-compatible server (a local fake) drives the same tools;
- the Anthropic provider sends Messages API requests of the documented shape to a fake
  client and keeps history append-only. A refusal ends the run without running its calls.

### Against a real local model

On 2026-10-28 a local 9B Qwen-family model served by Ollama (`/v1/chat/completions`, Windows,
`openmhs mcp --sim`) was given "set sim.psu-01 to 5 V and confirm". It called
`describe_device`, `read_state` and `check_command` (all through the real gateway) and then
ended its turn without submitting or explaining, after 271 s. The harness worked end to end;
the task was not completed by that model. The run found three provider problems, now fixed and
tested:
- Ollama refuses an object schema without `properties`, which the argument-less tools had;
  they now get `{}`.
- A small model's malformed tool arguments come back as HTTP 500 for the whole request; the
  provider retries 5xx answers twice (a model call touches no device).
- The server's error text is now kept in the exception.

**Not tested:** Claude. No Anthropic request has been sent from this repository; the
Anthropic provider follows the documented request shapes and is tested against a fake client.
