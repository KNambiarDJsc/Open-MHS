# `openmhs` command line

All commands except `serve`, `demo`, `conformance`, `profile check`, `adapter check` and `adapter new` are clients of a running gateway. They find it
through the runtime file in the gateway home: `--home`, else `$OPENMHS_HOME`, else the
per-user local data directory.

By default a command acts as the operator, using the token from the runtime file. Pass
`--token <T>` or set `OPENMHS_TOKEN` to act as another principal. Add `--json` to any
command to print the raw API result.

## Quick start

```sh
openmhs demo                          # throwaway gateway + virtual lab, guided tour

openmhs serve --sim                   # terminal 1: gateway with the virtual lab
openmhs devices list                  # terminal 2
openmhs grants add --id me-read --principal operator:local --device '*' --capability '*' --ops read --effect allow
openmhs read sim.thermo-01 temperature.ambient
```

## Commands

| Command | What it does |
|---|---|
| `serve [--sim] [--port N] [--listen ADDR --allow-remote] [--operator P] [--seed N]` | Run the gateway (same as `openmhsd`) |
| `serve --profile FILE`… | Also register the real device each protocol profile describes; connects to it ([profiles](adapters/profiles.md)) |
| `serve --adapter-manifest FILE`… | Also register an out-of-process adapter, started from its [permission manifest](../spec/permission-manifest.md) and supervised ([adapter host](architecture/adapter-host.md)) |
| `serve --profile-sim FILE`… | Also register each profile's simulated device, served in-process on a loopback port |
| `profile check FILE…` | Validate protocol profiles; connects to nothing. Exit 0 when all are valid |
| `demo [--keep]` | Guided scenario; exits non-zero if anything behaves unexpectedly |
| `info` | Your principal and role, gateway version, device count |
| `devices list` / `devices inspect <id>` | Registered devices / one descriptor |
| `read <device> <capability>` | Read from the device; flags stale or bad-quality readings |
| `exec <device> <capability> --value 5V` | Write a property |
| `exec <device> <action> --param position=120mm --param speed=50mm/s` | Invoke an action |
| `exec … --idempotency-key K --deadline-ms N --correlation-id C` | Request options |
| `exec … --check` | Ask whether it would be allowed now (preflight); runs nothing. Exit 0 allow, 2 deny, 5 needs approval |
| `commands show <id>` / `commands list [--state S]… [--limit N]` | Receipts |
| `approve <id>` (or `commands approve <id>`) | Approve a held command (operator) |
| `commands cancel <id>` / `commands reconcile <id>` | Cancel before dispatch / resolve `UNKNOWN_OUTCOME` by readback |
| `grants list` / `grants add …` / `grants revoke <id>` | Policy (operator) |
| `tokens create <principal> [--role agent\|operator]` / `tokens list` / `tokens revoke <id>` | API tokens (operator) |
| `audit verify` / `audit tail [--limit N]` | Audit log (operator) |
| `sim fault <device> <fault>` / `sim faults` | Fault injection (operator, `serve --sim` only) |
| `conformance [--device ID]… [--profile FILE]… [--adapter-manifest FILE]… [--report-dir DIR] [--seed N]` | Run the conformance suite against the built-in simulators, protocol profiles' simulators, or out-of-process adapters started in fixture mode (their manifests must point at a simulator, never a real device); no gateway needed. Exit 0 when each reaches its declared level ([spec/conformance.md](../spec/conformance.md)) |
| `adapter check FILE…` | Validate adapter permission manifests; starts nothing. Exit 0 when all are valid |
| `adapter new <name> [--path DIR] [--openmhs-path CHECKOUT]` | Create an adapter crate with a fake device and conformance tests ([adapters/README.md](adapters/README.md)) |
| `mcp [--url U] [--allow-operator]` | Serve MCP on stdin/stdout for an AI agent; needs an agent `--token` ([mcp.md](mcp.md)) |

### Values

| You type | It means |
|---|---|
| `--value 5V` | The quantity 5 V |
| `--value "5 V"` | The quantity 5 V |
| `--value 5 --unit V` | The quantity 5 V |
| `--value true` | A boolean |
| `--value idle` | Text |
| `--value 5` | A bare number. It is sent as-is, and the gateway rejects it wherever a quantity is required: units are never guessed. |

### Grants

```sh
openmhs grants add --id agent-psu --principal 'agent:*' --device sim.psu-01 \
  --capability 'power.output.*' --ops write --effect allow --max-side-effect reversible
openmhs grants add --id agent-move --principal 'agent:*' --device sim.axis-01 \
  --capability axis.move_absolute --ops invoke --effect require_approval
```

The effect is one of:

- `allow`
- `require_approval`
- `deny`, which always wins over the others

With no matching grant, the operation is denied.

### Simulator faults

Generic faults:

- `clear`
- `link-down`, `link-up`
- `stale=<ms>`
- `bad-quality`
- `readback-offset=<x>`
- `latency=<ms>`
- `hang`
- `apply-then-hang`
- `apply-then-malformed`
- `reject`
- `disconnect`
- `crash`

Device-specific faults:

| Device | Fault |
|---|---|
| `sim.psu-01` | `trip` |
| `sim.axis-01` | `jam` |
| `sim.thermo-01` | `overheat` |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success. For commands, `succeeded` (confirmed by readback). |
| 1 | Error: usage, connection, or a request that could not become a command |
| 2 | Denied by policy or validation, or the call needs the operator role |
| 3 | Command failed, or the audit log failed verification |
| 4 | Uncertain (`UNKNOWN_OUTCOME`): reconcile before acting again |
| 5 | Pending, usually `AWAITING_APPROVAL` |
| 6 | Unverified: the device says it applied the command, but no readback confirmed it |
| 7 | Not executed: cancelled or expired before dispatch |
