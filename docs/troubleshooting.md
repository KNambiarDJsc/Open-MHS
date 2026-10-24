# Troubleshooting

Messages below were produced by the real CLI. If yours differs, the code in brackets
(`policy.denied.no_grant`) is stable and is what to search for; the full list is in
[spec/errors.md](../spec/errors.md).

## `no running gateway found`

```text
error: no running gateway found (<home>: ...); start one with `openmhs serve`
```

There is no gateway for the home directory this command is using. Start one with
`openmhs serve --sim` in another terminal. If you did, the two commands are using
different homes: pass the same `--home`, or set `OPENMHS_HOME` for both
([configuration](configuration.md#the-gateway-home)).

## `another gateway is already running with home ...`

You started a second `openmhs serve` against a home that already has a gateway. Use the
running one, stop it (Ctrl+C), or give the new one its own `--home` and `--port 0`.

## `unauthorized: the token was rejected`

The `OPENMHS_TOKEN` (or `--token`) is wrong, expired, or revoked, or belongs to a
previous gateway home. Create a new one as the operator with `openmhs tokens create`
(unset `OPENMHS_TOKEN` first, so you act as the operator). A token is shown once; if you
lost it, make another and revoke the old one with `openmhs tokens revoke`.

## `policy.denied.no_grant`

```text
error [policy.denied.no_grant]: no grant allows 'agent:demo' to read 'temperature.ambient' on 'sim.thermo-01'
```

Working as designed: nothing is allowed until the operator grants it. Add a grant for
that principal, device, capability and operation with `openmhs grants add`, and make sure
the credential's own scope also covers it (a scope is a ceiling; the grants must also
allow the call). The operator is not exempt from this either.

## `gateway.forbidden: changing grants requires an operator token`

You are acting as an agent. Agents cannot change policy, approve commands, or mint
tokens. Unset `OPENMHS_TOKEN` to act as the operator.

## A command is `DENIED`

A denied command is a receipt, not a crash. Read the line after `policy`:

| Code | Meaning | What to do |
|---|---|---|
| `command.value.out_of_range` | The value is outside the declared range | Choose a value inside it (`openmhs devices inspect`) |
| `command.value.unit_mismatch` | The unit is of another kind (A for V) | Use the unit the capability declares |
| `command.value.type_mismatch` | A bare number where a quantity with a unit is required | Write `5V`, not `5` |
| `command.slew_exceeded` | Too large a change too fast | Step toward the target, waiting between steps |
| `command.state.stale` | A precondition needs a fresher reading than the device provides | Check the device; stale telemetry is refused on purpose |
| `policy.denied.no_grant` | No grant covers it | See above |

Run `openmhs exec ... --check` to ask whether a command would be allowed without sending it.

## `UNKNOWN_OUTCOME`

The gateway sent the command and did not learn what happened (a timeout, a lost link, a
crashed adapter). It is not retried. Check the device, then resolve it by reading back:

```sh
openmhs commands reconcile <command id>
```

The result is `RECONCILED_APPLIED` or `RECONCILED_NOT_APPLIED`. See
[Core concepts](concepts.md#command-outcomes).

## `AWAITING_APPROVAL`

The grant requires a human. An operator approves it (`openmhs approve <id>`); the agent
cannot approve its own command.

## `openmhs conformance` is slow or fails to start an adapter

The suite really sends commands and injects faults and takes about 15 seconds per device.
For `--adapter-manifest`, the manifest's `runtime.command` must be runnable (for example
`python` on your `PATH`), and fixture mode refuses adapters that offer no simulator.

## The MCP server cannot reach the gateway

`openmhs mcp` is a client: a gateway must be running, and the MCP host must pass the
agent's `OPENMHS_TOKEN` (and `OPENMHS_HOME` if you changed it). See
[Connecting an AI agent](mcp.md).

## Windows notes

- Run PowerShell commands as written; set the token with `$env:OPENMHS_TOKEN = "omhs_..."`.
- Serial SCPI is untested on Windows.
- If a build fails with a linker error and the disk is nearly full, free space
  (`cargo clean`); a debug build of the workspace needs several gigabytes.

## Still stuck

Search the [issues](https://github.com/KNambiarDJsc/Open-MHS/issues), then open one with the
bug report template: include `openmhs --version`, your platform, and whether you used
simulators ([SUPPORT.md](../SUPPORT.md)).
