# Local gateway API (v0.1)

Decision records: [ADR-0005](../docs/adr/0005-local-api-transport.md),
[ADR-0011](../docs/adr/0011-gateway-api-and-roles.md). Wire types:
`crates/openmhs-api`.

## Transport

- JSON-RPC 2.0, `POST /rpc`, `Content-Type: application/json`.
- The gateway listens on loopback by default. Binding any other address requires
  `--allow-remote`.
- Every request carries `Authorization: Bearer <token>`. A missing or unknown token
  gets HTTP 401 with error code `gateway.unauthorized`.
- A local client finds the gateway through the runtime file `gateway.json` in the
  gateway home:

```json
{ "url": "http://127.0.0.1:7400", "pid": 1234, "principal": "operator:local",
  "token": "omhs_…", "started_at": "2026-10-23T10:00:00.000Z" }
```

## Errors

Gateway errors use JSON-RPC code `-32000`. The stable OpenMHS code
([errors.md](errors.md)) is in `error.data.code`. Clients branch on that code, never on
the message.

```json
{ "code": -32000, "message": "…",
  "data": { "code": "command.idempotency.conflict", "errors": [], "command_id": "…" } }
```

A command that policy or validation refuses is **not** an error. `commands.submit`
returns its receipt with state `DENIED` and the reasons in `policy.errors`. Errors are
for requests that could not become a command at all: schema violations, idempotency
conflicts, unknown ids, missing permissions for the call itself.

Standard JSON-RPC codes are also used:

| Code | Meaning |
|---|---|
| `-32700` | Parse error |
| `-32600` | Invalid request |
| `-32601` | Method not found |
| `-32602` | Invalid params |

## Methods

Role column: **any** means agents and operators; **op** means operator only. Grants
apply to everyone. A role never bypasses authorization for device access.

| Method | Role | Params | Result |
|---|---|---|---|
| `gateway.info` | any | — | version, spec version, your principal and role, simulation flag, device count |
| `devices.list` | any | — | descriptors |
| `devices.describe` | any | `device` | descriptor |
| `state.read` | any | `device`, `capability` | `state` (value, timestamp, source, quality), `age_ms`, `max_age_ms`, `usable` |
| `commands.submit` | any | `request` (`command.schema.json`) | receipt (waits until terminal, `UNKNOWN_OUTCOME`, or `AWAITING_APPROVAL`) |
| `commands.preflight` | any | `request` | `decision`, `rules`, `errors?`, `checked_at`: what a submission would get now. Creates no command and runs nothing |
| `commands.get` | any¹ | `command_id` | receipt |
| `commands.list` | any¹ | `states?`, `limit?` (default 50, max 500) | receipts, newest first |
| `commands.cancel` | any² | `command_id` | receipt |
| `commands.reconcile` | any¹ | `command_id` | receipt |
| `commands.approve` | op | `command_id` | receipt (runs the command) |
| `grants.list` / `grants.add` / `grants.revoke` | op | — / `grant` / `id` | grants / grant / removed grant |
| `tokens.create` / `tokens.list` / `tokens.revoke` | op | `principal`, `role` / — / `id` | token (secret shown once) / token infos / removed |
| `audit.verify` | op | — | `records`, `valid`, `first_invalid?` |
| `audit.tail` | op | `limit?` | audit records |
| `sim.fault` | op | `device`, `fault` | message; only with `serve --sim` |

¹ An agent sees only its own commands. Other commands answer `command.unknown`, so
their existence isn't revealed.
² The requester or an approver.

The principal of a command always comes from the token, never from the request body.

## Guarantees

- `commands.submit`, `commands.approve` and `commands.reconcile` keep running if the
  client disconnects. The command still reaches a defined state; fetch it later with
  `commands.get`.
- Every call that changes something is recorded in the hash-chained audit log, with the
  principal that made it:
  - grant and token changes
  - approvals
  - reads, including denied reads
  - preflight checks
  - injected faults
- `commands.preflight` runs the same checks as a submission: schema, grants, units and
  ranges, rate and slew limits, preconditions, and freshness against new readings, under
  the same per-device lock. Its answer is advisory, because state can change before a
  submission, which is checked again.
- Token principals must be valid principal names (spec/authorization.md §2), or
  `tokens.create` fails with `-32602`.

## Audit record kinds

| Kind | Recorded when |
|---|---|
| `gateway.started`, `gateway.stopped`, `gateway.recovered` | Gateway lifecycle and crash recovery |
| `device.registered` | A device is registered at start |
| `policy.updated`, `grant.added`, `grant.revoked` | Policy changes |
| `token.created`, `token.revoked` | Token changes (never the secret) |
| `read` | A read, allowed or denied |
| `command.preflight` | A preflight check and its decision |
| `command.created`, `command.transition` | A command and every state change, including approvals (`APPROVED`, with the approver) |
| `sim.fault` | A fault injected into a simulator |
