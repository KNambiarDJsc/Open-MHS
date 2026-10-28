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
| `devices.status` | any | `device?`, `refresh?` | statuses: `device`, `availability` (`ONLINE`, `DEGRADED`, `OFFLINE`), `reason?`, `since`, `checked_at`, `busy`, `lease?` (`id`, `holder`, `expires_at`). `refresh` probes devices not checked in the last second first ([ADR-0019](../docs/adr/0019-device-availability.md)) |
| `events.poll` | any⁸ | `stream?`, `after?`, `devices?`, `kinds?` (`observation`, `availability`, `command`), `limit?` (default 50, max 500), `wait_ms?` (max 30 000) | `stream`, `events` (each `seq`, `at`, `kind` and its fields), `next`, `missed`, `reset?`. Without `after`, only events from now on. Waits up to `wait_ms` when there are none ([ADR-0023](../docs/adr/0023-state-fabric.md)) |
| `watches.create` | any⁸ | `device`, `capability`, `interval_ms` (min 200), `ttl_ms?` (default 300 000, max 3 600 000) | watch (`id`, `principal`, `device`, `capability`, `interval_ms`, `created_at`, `expires_at`). Samples the capability into `events.poll`; one per principal and capability (a new one replaces it), at most 16 per principal |
| `watches.list` / `watches.cancel` | any⁹ | — / `id` | watches / the cancelled watch |
| `graph.get` | any | — | `devices`, `relationships` (each with `from` and `origin`), `resources`, `frames`, `issues` ([topology.md](topology.md)) |
| `leases.acquire` | any⁵ | `devices?`, `resources?`, `ttl_ms`, `reason?` | lease (`id`, `holder`, `claim`, `expires_at`, …); all or nothing ([topology.md](topology.md) §Leases) |
| `leases.get` / `leases.list` | any | `id` / `active?`, `limit?` | lease / leases, newest first |
| `leases.renew` | holder | `id`, `ttl_ms` | lease with the new deadline |
| `leases.release` | holder or a human approver | `id`, `reason?` | lease, released |
| `plans.submit` | any | `plan` ([plans.md](plans.md) §1) | plan; returns when it ends or waits. Each step is an ordinary command of the caller |
| `plans.preflight` | any | `plan` | `ok`, `lease`, `outcome`, `order`, `steps` (verdicts): a dry run against twins ([plans.md](plans.md) §8). Creates no command, lease or plan |
| `plans.get` / `plans.list` | any¹ | `id` / `limit?` | plan / plans, newest first |
| `plans.decide` | op⁶ | `id`, `decision` (`resume`, `retry`, `compensate`, `abort`), `reason?` | plan |
| `plans.cancel` | any² | `id`, `reason?` | plan |
| `procedures.list` / `procedures.get` | any | — / `name`, `version?` | procedures / one (the newest version if none named) ([procedures.md](procedures.md)) |
| `procedures.propose` | any | `procedure` | procedure, `proposed`; cannot run until approved |
| `procedures.add` / `procedures.approve` / `procedures.remove` | op | `procedure` / `name`, `version` / `name`, `version` | procedure, approved / approved / removed |
| `procedures.check` / `procedures.run` | any | `name`, `version?`, `params` | plan preflight / plan, run as the caller; parameters checked first |
| `state.read` | any | `device`, `capability` | `state` (value, timestamp, source, quality), `age_ms`, `max_age_ms`, `usable` |
| `commands.submit` | any | `request` (`command.schema.json`), `approval_id?` | receipt (waits until terminal, `UNKNOWN_OUTCOME`, or `AWAITING_APPROVAL`). With `approval_id`, spends an approved request for exactly this command |
| `commands.preflight` | any | `request` | `decision`, `rules`, `errors?`, `checked_at`: what a submission would get now. Creates no command and runs nothing |
| `commands.get` | any¹ | `command_id` | receipt |
| `commands.list` | any¹ | `states?`, `limit?` (default 50, max 500) | receipts, newest first |
| `commands.cancel` | any² | `command_id` | receipt |
| `commands.reconcile` | any¹ | `command_id` | receipt |
| `commands.resolve` | op⁷ | `command_id`, `applied` (bool), `reason` | receipt: `RECONCILED_APPLIED` or `RECONCILED_NOT_APPLIED` on the operator's word, with the reason recorded. Contacts no device ([lifecycle.md](lifecycle.md) §5) |
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

⁶ And a human approver configured in policy ([plans.md](plans.md) §5).

⁷ A human (not an `agent:` principal) whose credential's scopes reach the command.

⁸ Observations are delivered, and watches created, only for capabilities whose descriptor
declares `subscribe`, and only to a caller whose credential's scopes and grants allow
`subscribe` on them, decided when each event is delivered and before each sample.
Availability events follow the credential's device scopes. Command events go to the
command's principal, and all of them to the operator.

⁹ An agent sees and cancels its own watches; the operator, all of them. Revoking or
rotating a credential ends the watches created with it.

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
| `device.registered` | A device is registered at start, with its availability (`OFFLINE` if it did not answer) |
| `device.availability` | A device's availability changed (`from`, `to`, `reason`); unchanged probes write nothing |
| `policy.updated`, `grant.added`, `grant.revoked` | Policy changes |
| `token.created`, `token.revoked`, `token.rotated` | Credential changes (never the secret) |
| `credential.rejected` | First refused use of an expired or revoked credential (once per credential and reason) |
| `approval.requested`, `approval.approved`, `approval.denied`, `approval.expired`, `approval.revoked`, `approval.consumed` | Approval records and every change to them |
| `topology.set` | The operator's topology was applied at start |
| `lease.acquired`, `lease.renewed`, `lease.released`, `lease.expired` | Leases and every change to them (`token.revoked` and `token.rotated` list the leases they released) |
| `plan.preflight` | A plan was dry-run against twins; records the verdicts and how many real readings were taken |
| `plan.submitted`, `plan.step.started`, `plan.step.succeeded`, `plan.step.failed`, `plan.step.compensating`, `plan.step.compensated`, `plan.waiting_approval`, `plan.awaiting_decision`, `plan.compensating`, `plan.decision`, `plan.recovered`, `plan.succeeded`, `plan.halted`, `plan.compensated`, `plan.aborted`, `plan.cancelled` | Every change to a plan ([plans.md](plans.md)). Each step's command has its own command records as usual |
| `read` | An allowed read, with the reading or the adapter's error |
| `read.denied` | A refused read: unknown device or capability, no grant, or not readable |
| `procedure.proposed`, `procedure.added`, `procedure.approved`, `procedure.removed`, `procedure.run` | Procedure changes (version, `sha256`, who) and runs (with the plan id) ([procedures.md](procedures.md)) |
| `watch.created`, `watch.ended` | A watch was created (principal, credential, capability, interval, expiry) or ended (`expired`, cancelled, replaced, no longer allowed, credential revoked, gateway stopping). Its samples are not recorded one by one; they reach only callers allowed to subscribe ([ADR-0023](../docs/adr/0023-state-fabric.md)) |
| `request.rejected` | A command request that failed schema validation, before it became a command |
| `command.preflight` | A preflight check and its decision |
| `command.created`, `command.transition` | A command and every state change, including approvals (`APPROVED`, with the approver) |
| `sim.fault` | A fault injected into a simulator |
