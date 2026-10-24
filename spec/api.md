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
| `gateway.info` | any | — | version, spec version, your principal and role, your credential id and scopes, simulation flag, device count |
| `devices.list` | any | — | descriptors |
| `devices.describe` | any | `device` | descriptor |
| `graph.get` | any | — | `devices`, `relationships` (each with `from` and `origin`), `resources`, `frames`, `issues` ([topology.md](topology.md)) |
| `leases.acquire` | any⁵ | `devices?`, `resources?`, `ttl_ms`, `reason?` | lease (`id`, `holder`, `claim`, `expires_at`, …); all or nothing ([topology.md](topology.md) §Leases) |
| `leases.get` / `leases.list` | any | `id` / `active?`, `limit?` | lease / leases, newest first |
| `leases.renew` | holder | `id`, `ttl_ms` | lease with the new deadline |
| `leases.release` | holder or a human approver | `id`, `reason?` | lease, released |
| `state.read` | any | `device`, `capability` | `state` (value, timestamp, source, quality), `age_ms`, `max_age_ms`, `usable` |
| `commands.submit` | any | `request` (`command.schema.json`), `approval_id?` | receipt (waits until terminal, `UNKNOWN_OUTCOME`, or `AWAITING_APPROVAL`). With `approval_id`, spends an approved request for exactly this command |
| `commands.preflight` | any | `request` | `decision`, `rules`, `errors?`, `checked_at`: what a submission would get now. Creates no command and runs nothing |
| `commands.get` | any¹ | `command_id` | receipt |
| `commands.list` | any¹ | `states?`, `limit?` (default 50, max 500) | receipts, newest first |
| `commands.cancel` | any² | `command_id` | receipt |
| `commands.reconcile` | any¹ | `command_id` | receipt |
| `commands.approve` | op | `command_id` | receipt (runs the command) |
| `approvals.request` | any | `request`, `ttl_ms?` | approval (`PENDING`); only for commands policy holds for approval ([authorization.md](authorization.md) §5) |
| `approvals.get` / `approvals.list` | any³ | `id` / `statuses?`, `limit?` | approval / approvals |
| `approvals.approve` / `approvals.deny` | op⁴ | `id` / `id`, `reason?` | approval. Approving one bound to a held command runs it |
| `approvals.revoke` | any³ | `id`, `reason?` | approval: withdrawn, if not yet used |
| `grants.list` / `grants.add` / `grants.revoke` | op | — / `grant` / `id` | grants / grant / removed grant |
| `tokens.create` | op | `principal`, `role`, `scopes`, `ttl_ms?` | credential (secret shown once). No scopes: it authorizes nothing ([authorization.md](authorization.md) §7) |
| `tokens.list` / `tokens.revoke` / `tokens.rotate` | op | — / `id` / `id`, `ttl_ms?` | credential infos / revoked (its open approvals withdrawn) / replacement (secret shown once; old one revoked) |
| `audit.verify` | op | — | `records`, `valid`, `first_invalid?` |
| `audit.tail` | op | `limit?` | audit records |
| `sim.fault` | op | `device`, `fault` | message; only with `serve --sim` |

¹ An agent sees only its own commands. Other commands answer `command.unknown`, so
their existence isn't revealed.
² The requester or an approver.
³ The requester sees its own approvals and an approver sees all. Others get
`approval.unknown`.
⁴ Also: an approver configured in policy, not the requester, whose credential's scopes
reach the command.

⁵ Only for devices the caller could operate: within its credential's scopes and with a
grant short of `deny` for some write or invoke on each claimed device.

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
| `token.created`, `token.revoked`, `token.rotated` | Credential changes (never the secret) |
| `credential.rejected` | First refused use of an expired or revoked credential (once per credential and reason) |
| `approval.requested`, `approval.approved`, `approval.denied`, `approval.expired`, `approval.revoked`, `approval.consumed` | Approval records and every change to them |
| `topology.set` | The operator's topology was applied at start |
| `lease.acquired`, `lease.renewed`, `lease.released`, `lease.expired` | Leases and every change to them (`token.revoked` and `token.rotated` list the leases they released) |
| `plan.submitted`, `plan.step.started`, `plan.step.succeeded`, `plan.step.failed`, `plan.step.compensating`, `plan.step.compensated`, `plan.waiting_approval`, `plan.awaiting_decision`, `plan.compensating`, `plan.decision`, `plan.recovered`, `plan.succeeded`, `plan.halted`, `plan.compensated`, `plan.aborted`, `plan.cancelled` | Every change to a plan ([plans.md](plans.md)). Each step's command has its own command records as usual |
| `read` | An allowed read, with the reading or the adapter's error |
| `read.denied` | A refused read: unknown device or capability, no grant, or not readable |
| `request.rejected` | A command request that failed schema validation, before it became a command |
| `command.preflight` | A preflight check and its decision |
| `command.created`, `command.transition` | A command and every state change, including approvals (`APPROVED`, with the approver) |
| `sim.fault` | A fault injected into a simulator |
