# Refused requests in the audit log

**The invariant.** A request from an authenticated caller that asks the gateway to act, or to read device
state, and is refused, leaves exactly one durable audit record, whether or not a command, plan or lease came
to exist.

Found by the [read-only robot evaluation](../../examples/ros2-readonly-robot/RESULTS.md), which reported two
refusals with no record.

## Why requests disappeared

There were two separate causes, and neither was a failure of the audit log.

**1. Clients refused on the gateway's behalf.** The MCP server is a client of the gateway ([ADR-0001](../adr/0001-single-trusted-gateway.md)).
To choose between a write and an invoke, `submit_command` fetched the descriptor, and when the capability was
not there it answered `command.capability.unknown` itself. The request never crossed the boundary, so nothing
inside the gateway could have recorded it. An unknown device, and a command to an event capability, went the same
way. `openmhs exec` had the same shape. The SDK's `submit` did not, which is why the SDK left a receipt and MCP did not.

**2. Refusals outside the command lifecycle returned an error and wrote nothing.** Commands record denials because a
denied command is a receipt. Reads record them explicitly. Everything else simply returned `Err`: a plan or lease
refused (`lease.not_authorized`), any operator-only method called by an agent (`gateway.forbidden`, including
`tokens.create`, `grants.add` and `audit.export`), a watch on an ungranted capability, cancelling another caller's command,
deciding an approval without authority, and an idempotency conflict. The plan case the evaluation found was
one instance of this, and it was the least serious one.

## The boundary

| Step | Where | Identity known | Intent known | Trusted |
|---|---|---|---|---|
| MCP tool call, CLI argument parsing | Client | No | Yes | **No** |
| Authentication | `handle` | Yes, from here on | | Yes |
| JSON-RPC decode | `handle` | Yes | Method and params | Yes |
| Method dispatch | `dispatch` | Yes | Yes | Yes |
| Command lifecycle, reads | Engine | Yes | Yes | Yes |

The earliest point with identity and intent, inside the gateway, is `handle`, right around `dispatch`. So:

- **Clients do not refuse on the gateway's behalf.** What the descriptor cannot answer, the client forwards.
- **`handle` records what `dispatch` refused,** in one place, for the methods listed below. No method has to remember to.

This also corrects a small leak: a credential scoped away from a capability now gets `auth.scope.denied` through
MCP, as it does through the SDK, instead of being told the capability does not exist.

## One record for each refusal

| What was refused | Record | Why |
|---|---|---|
| A command that passed schema validation, then was denied | `command.created`, `command.transition` to `DENIED` | A command existed. Unchanged. Includes unknown devices and capabilities, because the engine creates a receipt for those |
| A read | `read.denied`, or `read` with the adapter's error | Unchanged |
| Anything else that asks to act | `request.rejected` | No command came to exist, or this request did not create one |
| An expired or revoked credential | `credential.rejected`, once per credential and reason | Unchanged |

`request.rejected` already meant "refused before it became a command", and its retention class is already `SECURITY`, so
no new kind was needed. Its scope widens from schema failures to all such refusals, and it is never used for a
command that exists. `command.transition` is not touched.

```json
{
  "kind": "request.rejected",
  "subject": "agent:robot-observer",
  "data": {
    "method": "plans.submit",
    "code": "lease.not_authorized",
    "principal": "agent:robot-observer",
    "credential_id": "tok_01",
    "target": { "devices": ["ros2.sim-robot-01"], "steps": 1 }
  }
}
```

## What is recorded and what is not

The record is evidence that an attempt happened, not a copy of it.

- **Recorded:** the method, the stable code, the principal and credential id from the authenticated session, and
  named identifiers from the request: device, capability, operation, ids, procedure name and version, correlation id.
- **Never recorded:** the bearer token, `value`, `params`, `reason`, descriptions, grant bodies, and the free text of an
  error `message`. Error messages can quote what the caller sent.
- **Identifiers are normalized.** A caller controls them, so each is kept only if it is at most 128 characters of
  `A-Z a-z 0-9 . _ : / @ * + -`. Otherwise it is dropped and `target_omitted` is set. A list keeps at most 16 entries.
  So a prompt, a payload or a huge string in `capability` does not enter the durable record.
- **The schema-failure record changed too.** It carried each error's `message`, which can quote the rejected input, and
  now carries `code` and `pointer` only.
- A caller who puts its own secret in a field that looks like an identifier will see that string recorded. The same is
  true of `read.denied`, and of any accepted command.

## Deliberately not recorded

| Request | Why |
|---|---|
| No credential, or an unknown one | There is no identity. Anyone can send these, so recording them would let an unauthenticated stranger fill the journal |
| Valid credential, non-JSON body, or wrong `jsonrpc` version | No operation can be identified |
| Valid credential, unknown method | No operation can be identified, and a client and gateway of different versions do this innocently |
| Queries refused or answering "not found": `*.get`, `*.list`, `devices.describe`, `devices.status`, `graph.get`, `events.poll`, `procedures.get`, `access.allowed_operations`, `gateway.info` | They ask about the gateway's own records and cannot act or read a device. Another caller's command answers `command.unknown` here on purpose, so a lookup cannot tell that apart from a mistyped id |
| Advisory checks: `commands.preflight`, `plans.preflight`, `procedures.check` | They ask, they do not attempt. A successful `command.preflight` or `plan.preflight` is already recorded |
| Infrastructure failures: `gateway.journal`, `gateway.storage`, `gateway.internal` | The gateway failed; it did not refuse. These may be the failure to write the record |
| An MCP argument mistake: a property with no `value` or with `params`, an action with a `value` | No operation could be formed, so nothing is sent. The CLI's equivalents are the same |

A client that sends nothing cannot be audited. A caller can always skip the MCP server and call the gateway, which is why
nothing here relies on the client.

## Limits

- **The interface is not known.** The record says which method was called, not whether it came through MCP, the CLI or the
  SDK. They look the same to the gateway.
- **No rate limit or deduplication.** Each attempt writes a record, by design, so two attempts are two records. An
  authenticated caller can grow the journal by being refused, as it already could with reads.
- **Near the journal's limit, refusals are not recorded.** Once only the reserve is left ([ADR-0021](../adr/0021-journal-quota-and-reserve.md)),
  a refused request logs a warning and writes nothing, so a refused caller cannot spend the reserve kept for
  commands already running. If a write fails, the request stays refused, and the failure is logged. In both cases the
  invariant does not hold, and the caller still gets the refusal.
- **`read.denied` keeps the caller's capability string as sent.** It predates this work and was left alone.
- **The audit log is local and unsigned**, as before ([limitations](../limitations.md)).
