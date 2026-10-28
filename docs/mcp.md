# Connecting an AI agent over MCP

`openmhs mcp` is a [Model Context Protocol][mcp] server on stdin/stdout. Any MCP host
(Claude Code, Claude Desktop, an IDE agent, your own client) can launch it. It is a
client of a running gateway. It holds one **agent** token, so whatever the agent does
is authorized, checked, journaled and audited by the gateway like any other client's.
It never sees hardware credentials. Design: [ADR-0012](adr/0012-mcp-server-and-sdk.md).

> The examples use the virtual lab. For your own device, start the gateway with its
> profile or adapter instead ([Connect your own device](connect-a-device.md)). OpenMHS is
> not a safety system: hardware interlocks and emergency stops keep final authority.

## Fastest start: one command, no setup

To see an agent drive OpenMHS from your MCP host, with nothing else running:

```sh
claude mcp add openmhs-sim -- openmhs mcp --sim
```

or, in any host's JSON config, `{"command": "openmhs", "args": ["mcp", "--sim"]}`.

`--sim` starts a throwaway gateway with the virtual lab inside the same process, grants
the agent read access to every simulated device and write access to the power supply
(`sim.psu-01`, `power.output.*`), and serves MCP for it. There is no token to copy and
nothing to start first. Everything is discarded on exit, and no real hardware is touched.
The agent still cannot do more than that (the axis is refused, 48 V is refused, a fast
jump is refused with numbers it can act on), and it cannot approve its own commands: the
operator token stays in the process and never reaches the MCP server. It cannot be mixed
with `--url` or `--allow-operator`.

This is for trying OpenMHS. To use your own devices, follow the steps below.

## 1. Start a gateway and give the agent a token

```sh
cargo build -p openmhs-cli                  # puts openmhs in target/debug/
openmhs serve --sim                         # terminal 1: gateway with the virtual lab
```

In a second terminal, as the operator:

```sh
# What the agent may do: read everything, write the power supply. Nothing else.
openmhs grants add --id agent-read --principal 'agent:*' --device '*' --capability '*' --ops read --effect allow
openmhs grants add --id agent-psu  --principal 'agent:*' --device sim.psu-01 --capability '*' --ops write --effect allow

# The credential is limited to the same devices and operations (a scope is a ceiling:
# the grants above must also allow the call). It prints the token once.
openmhs tokens create agent:claude --scope 'sim.*:*:read' --scope 'sim.psu-01:*:write' --ttl 7d
```

Keep the token secret: it is the agent's identity. Revoke it with
`openmhs tokens revoke <id>` (or replace it with `openmhs tokens rotate <id>`) (`openmhs tokens list` shows ids, never secrets).

## 2. Register the server with your MCP host

### Claude Code

```sh
claude mcp add openmhs -e OPENMHS_TOKEN=<token> -- openmhs mcp
```

Use the full path to `openmhs` if it is not on `PATH`, for example
`target/debug/openmhs.exe` on Windows. If the gateway runs with a non-default
`--home`, also pass `-e OPENMHS_HOME=<dir>`. The server finds the gateway's URL in that
home's runtime file.

Or put the same thing in a JSON config and pass it with `--mcp-config`:

```json
{
  "mcpServers": {
    "openmhs": {
      "command": "openmhs",
      "args": ["mcp"],
      "env": { "OPENMHS_TOKEN": "<token>" }
    }
  }
}
```

### Any other MCP host

Configure a stdio server with command `openmhs`, argument `mcp`, and the environment
variable `OPENMHS_TOKEN`. Options:

| Option | Meaning |
|---|---|
| `--token` / `OPENMHS_TOKEN` | Agent token. Required; the server will not fall back to the operator token |
| `--home` / `OPENMHS_HOME` | Gateway home, to find its URL |
| `--url` | Gateway URL instead of the runtime file (needs `--token`) |
| `--allow-operator` | Accept an operator token. Don't: a model could then approve its own commands and change policy |

Logs go to stderr; stdout carries only MCP.

## 3. What the agent gets

Tools (all return `structuredContent` with an `outputSchema`):

| Tool | Changes the world? | Use |
|---|---|---|
| `list_devices`, `describe_device` | No | Discover devices, units, ranges, limits. `list_devices` also gives each device's `availability`, `reason`, `busy` and `lease_holder`; an `OFFLINE` device refuses commands with `device.unavailable` |
| `get_insights` | No | What your earlier commands on a device did, from the journal: confirmed values, refusals with the limits behind them, how far readings landed from the request, readback latency, unresolved outcomes. Evidence to iterate on; it never changes what you may do ([ADR-0022](adr/0022-insights-from-the-journal.md)) |
| `list_procedures`, `check_procedure` | No | Reviewed, versioned plans with typed parameters, and a dry run of one ([spec/procedures.md](../spec/procedures.md)) |
| `run_procedure` | **Yes** | Run an approved procedure: an ordinary plan, as the agent, with every check |
| `propose_procedure` | No | Store a plan that worked as a proposed procedure; only a person can approve it |
| `find_capabilities` | No | Which devices can do what you need: by capability name pattern, quantity, unit, operation, a value inside the declared range, side effect, availability. Candidates, not permissions |
| `watch_state` | Reads the device | Sample a capability that declares `subscribe` (needs a `subscribe` grant) every `interval_ms`, into the event stream ([ADR-0023](adr/0023-state-fabric.md)) |
| `poll_events` | No | Observations you may subscribe to, availability changes and your commands' state changes since `after`; `wait_ms` waits for one. `missed` > 0 means you fell behind |
| `cancel_watch` | No | Stop a watch you created |
| `read_state` | No (audited) | Read now; check `usable` before relying on it |
| `check_command` | No | Ask whether a command would be allowed right now |
| `submit_command` | **Yes** | Write a property (`value`) or invoke an action (`params`) |
| `get_command` | No | Fetch a command; `wait_ms` waits out an approval |
| `list_pending_approvals` | No | Your commands waiting for a human |
| `reconcile_command` | Reads the device | Settle `UNKNOWN_OUTCOME` without re-sending |
| `cancel_command` | Withdraws | Cancel before dispatch |
| `check_plan` | No (reads the devices) | Dry-run a plan against twins ([spec/plans.md](../spec/plans.md) §8); advisory, no physics |
| `submit_plan` | **Yes** | Run several commands in order under one lease; each step is an ordinary command |
| `get_plan` | No | Fetch a plan; `wait_ms` waits until it ends or stops for a human |
| `cancel_plan` | Withdraws | Stop your plan at the next step boundary; completed steps stay |

There is no tool to decide how a waiting plan continues (`resume`, `retry`,
`compensate`, `abort`). That is a human approver's decision, made with
`openmhs plans decide`.

A refused command is a normal result with `isError: true`. It names the reason code
and rule, the numbers that bound it, and `next_steps`:

```json
{
  "state": "DENIED", "outcome": "denied", "confirmed": false, "reached_device": false,
  "summary": "power.output.voltage.setpoint on sim.psu-01: refused (change of 9 V exceeds the limit of 2 V per 1000 ms); nothing was sent to the device.",
  "reasons": [{ "code": "command.slew_exceeded", "message": "…",
                "details": { "max_change": 2.0, "per_ms": 1000, "unit": "V" } }],
  "next_steps": ["Change by at most 2 V per 1000 ms: approach the target in steps of at most 2 V, waiting at least 1000 ms between steps, and read the value back between steps."]
}
```

Only `confirmed: true` means the effect was verified by reading the device back.
`AWAITING_APPROVAL` is not an error: a human approves with `openmhs approve <id>` while
the agent waits in `get_command`.

Text that came from a device or descriptor author arrives wrapped as
`<<device-text … device-text>>`. The server's instructions tell the model it is never
an instruction.

## 4. Verified with

On 2026-10-23, Claude Code 2.1.306 on Windows 11 ran headless (`claude -p`) with
`--mcp-config` and `--strict-mcp-config`, against `openmhs mcp` and the virtual lab.
Only the `mcp__openmhs__*` tools were allowed. The task was "set the supply to 5 V and
confirm, then home the stage". The gateway's own records show what happened:

- Claude learned of the 2 V/s slew limit from `check_command`.
- It made three VERIFIED writes (2, 4, then 5 V), each with a matching readback.
- `axis.home` was DENIED with `policy.denied.no_grant`, and Claude did not retry it.
- The audit chain was valid, and no token appeared in the transcript or the audit log.

The `claude mcp add` command above follows Claude Code's documented syntax, but it was
not run separately. The JSON config path is the one exercised. Other MCP hosts have not
been tried.

## Without a model

`examples/agent-loop/` has a scripted agent that drives `openmhs mcp` over stdio with
a small (about 140 lines) standard-library MCP client. It hits the slew limit, reads the numbers from
the refusal, and ramps. It also has an optional variant that lets Claude decide, which
runs only with your own `ANTHROPIC_API_KEY`.

[mcp]: https://modelcontextprotocol.io/specification/2025-11-25
