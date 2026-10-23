# Agent loop examples

Both examples start a throwaway gateway with simulated devices, give the agent an agent
token, and connect it over MCP stdio to `openmhs mcp`. That is the same path an MCP host
such as Claude Code uses. Build first: `cargo build -p openmhs-cli`.

| File | What it is |
|---|---|
| `mock_agent.py` | A scripted agent, no model and no key. It replans only from what the gateway returns |
| `llm_agent.py` | The same, with Claude deciding. Optional: needs `pip install anthropic` and your own `ANTHROPIC_API_KEY` |
| `mcp_stdio.py` | A minimal MCP client (standard library): handshake, `tools/list`, `tools/call` |
| `lab.py` | Starts the gateway, sets a policy and creates the agent token, using the Python SDK as operator |

```sh
python examples/agent-loop/mock_agent.py
```

The scripted agent gets three tasks:

1. **Set the supply to 5 V.** The jump breaks the 2 V/s slew limit. The refusal carries
   `max_change` and `per_ms`, so the agent ramps 0 → 2 → 4 → 5 V. It checks each step
   with `check_command` before submitting, and every step is verified by readback.
2. **Set it to 15 V.** That is outside the declared range of 0..12 V. The agent does not
   retry or "get close"; it reports back.
3. **Home the stage.** There is no grant for it, so the agent stops and asks the human.

`python/tests/test_examples.py` runs it on every test run.

```sh
python examples/agent-loop/llm_agent.py --print-tools        # what Claude is given; no key needed
ANTHROPIC_API_KEY=... python examples/agent-loop/llm_agent.py --confirm
```

`--confirm` asks you at the terminal before each `submit_command` is sent. Without a
key, `llm_agent.py` explains what it needs and runs nothing. Tests never call the API.
