# `openmhs` command line

All commands except `serve`, `demo`, `conformance`, `profile new`, `profile check`, `adapter check` and `adapter new` are clients of a running gateway. They find it
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
| `profile new PROTOCOL [--id ID] [--out FILE]` | Start a profile for your device from the tested example of `modbus-tcp` (or `modbus`), `mqtt`, `scpi` or `http`, and check it. Never overwrites ([connect a device](connect-a-device.md)) |
| `profile check FILE…` | Validate protocol profiles; connects to nothing. Exit 0 when all are valid |
| `demo [--keep]` | Guided scenario; exits non-zero if anything behaves unexpectedly |
| `info` | Your principal and role, gateway version, device count |
| `devices list` / `devices inspect <id>` | Registered devices / one descriptor |
| `graph` | Relationships, shared resources and frames ([spec/topology.md](../spec/topology.md)) |
| `leases acquire --device D [--resource R] --ttl 10m [--reason …]` | Exclusive use of devices (and what they contain or share) until released or due; others' writes and invokes are refused meanwhile |
| `leases list [--active]` / `leases show <id>` | Every lease, or one: holder, claim, deadline |
| `leases renew <id> --ttl 10m` / `leases release <id> [--reason …]` | Move your lease's deadline / end it (an approver can end anyone's) |
| `plans preflight FILE` | Dry-run a plan (JSON, [spec/plans.md](../spec/plans.md)) against twins of its devices; changes nothing real. Exit 0 every step would run, 5 some need approval, 2 otherwise |
| `plans submit FILE` | Run a plan. Exit 0 `SUCCEEDED`, 3 `HALTED` or `COMPENSATED`, 4 `AWAITING_DECISION`, 5 waiting for approval, 7 `ABORTED` or `CANCELLED` |
| `plans show <id>` / `plans list [--limit N]` | A plan's steps, commands and decisions / recent plans (agents see their own) |
| `plans decide <id> --decision resume\|retry\|compensate\|abort [--reason …]` | Decide how a waiting plan continues (operator; a human approver) |
| `plans cancel <id> [--reason …]` | Stop a plan at the next step boundary: its submitter or an approver |
| `read <device> <capability>` | Read from the device; flags stale or bad-quality readings |
| `exec <device> <capability> --value 5V` | Write a property |
| `exec <device> <action> --param position=120mm --param speed=50mm/s` | Invoke an action |
| `exec … --idempotency-key K --deadline-ms N --correlation-id C` | Request options |
| `exec … --check` | Ask whether it would be allowed now (preflight); runs nothing. Exit 0 allow, 2 deny, 5 needs approval |
| `commands show <id>` / `commands list [--state S]… [--limit N]` | Receipts |
| `approve <id>` (or `commands approve <id>`) | Approve a held command (operator) |
| `approvals request <exec arguments> [--ttl 15m]` | Ask for approval of a command before submitting it; prints the approval id. Only for commands policy holds for approval ([spec/authorization.md](../spec/authorization.md) §5) |
| `approvals show <id>` / `approvals list [--status S]… [--limit N]` | Requesters see their own; approvers see all |
| `approvals approve <id>` / `approvals deny <id> [--reason R]` | Decide (an approver who is not the requester; agents cannot). Approving a held command runs it |
| `approvals revoke <id> [--reason R]` | Withdraw an approval that has not been used: your own, or any as approver |
| `exec … --approval <id>` | Submit the command an approved request covers. The approval is spent once, and only for exactly that command |
| `commands cancel <id>` / `commands reconcile <id>` | Cancel before dispatch / resolve `UNKNOWN_OUTCOME` by readback |
| `grants list` / `grants add …` / `grants revoke <id>` | Policy (operator) |
| `tokens create <principal> [--role agent\|operator] (--scope DEVICE:CAPABILITY:OPS … \| --any-scope) [--ttl 12h]` / `tokens list` / `tokens revoke <id>` / `tokens rotate <id> [--ttl …]` | Credentials (operator). A credential reaches only its scopes, and only what grants also allow; it expires at `--ttl`; revoked credentials stay listed ([spec/authorization.md](../spec/authorization.md) §7) |
| `audit verify` / `audit tail [--limit N]` | Audit log (operator) |
| `sim fault <device> <fault>` / `sim faults` | Fault injection (operator, `serve --sim` only) |
| `conformance [--device ID]… [--profile FILE]… [--adapter-manifest FILE]… [--report-dir DIR] [--seed N]` | Run the conformance suite against the built-in simulators, protocol profiles' simulators, or out-of-process adapters started in fixture mode (their manifests must point at a simulator, never a real device); no gateway needed. Exit 0 when each reaches its declared level ([spec/conformance.md](../spec/conformance.md)) |
| `adapter check FILE…` | Validate adapter permission manifests; starts nothing. Exit 0 when all are valid |
| `adapter keygen <out>` | Make an Ed25519 signing key: `<out>.key` (secret, keep private) and `<out>.pub` |
| `adapter sign <manifest> --key <out>.key --artifact FILE… [--out FILE]` | Sign a manifest, binding it to the adapter's files ([signatures](../spec/permission-manifest.md#signatures)) |
| `trust add <key.pub \| hex> --name N [--adapter ID]…` / `trust list` / `trust revoke <key_id>` | The signers whose adapters may start (operator, local files only). Revoking does not stop an adapter that is already running; restart the gateway |
| `serve … --allow-unsigned-adapters` | Development only: start adapters whose manifests are not signed. The default refuses them |
| `serve … --topology FILE` | The installation's topology: how registered devices relate. A topology naming an unknown device stops the start ([spec/topology.md](../spec/topology.md)) |
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
