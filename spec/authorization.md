# Authorization

Decision record: [ADR-0003](../docs/adr/0003-authorization-engine.md).

## 1. Principles

1. **Deny by default.** With no grants, every principal can do nothing.
2. **Discovery is not permission.** A device appearing in the registry grants nothing.
3. **Descriptors cannot grant.** Permissions come only from the operator's policy.
   Any permission-like field in a descriptor is rejected by the schema.
4. **Deterministic.** The same request, policy and state always produce the same
   decision. No language model participates.
5. **Explainable.** Every decision lists the rule IDs that produced it, so an agent can
   replan and a human can audit.
6. **Fail closed.** Missing policy, missing limits, or missing/stale required state
   produce a denial.

## 2. Principals

A principal is an authenticated caller: an agent, a user, or a service. Its identifier
comes from the authenticated session, never from the request body. v0.1 principals are
configured by the operator; credentials and rotation are specified in M8.

A principal name is an optional lowercase kind and a colon, then an identifier:
`operator:local`, `agent:lab-assistant`, or a bare `agent.lab-assistant`
(`common.schema.json#/$defs/principal`). The gateway refuses to create a token, or start
with an operator, whose name does not match. Grant `principal` fields are patterns over
these names, so `agent:*` matches every `agent:` principal.

## 3. Grants

A grant is an operator-written rule:

```json
{
  "id": "grant.lab-assistant.psu-setpoint",
  "principal": "agent.lab-assistant",
  "device": "sim.psu-*",
  "capability": "power.output.*",
  "operations": ["read", "write"],
  "effect": "allow"
}
```

- `device` and `capability` are exact names or patterns where `*` matches within the
  remainder of the name (`power.output.*` matches `power.output.voltage.setpoint`).
- `effect`: `allow`, `require_approval`, or `deny`.
- Optional `max_side_effect` limits an `allow` to capabilities whose side-effect class
  is at most the given one (`none` < `reversible` < `irreversible` < `hazardous`).

## 4. Evaluation

For a command `(principal, device, capability, operation)`:

1. Collect the grants that match all four.
2. If any matching grant is `deny` → **deny**.
3. Else if any is `require_approval` → **require approval**.
4. Else if any is `allow` (and satisfies `max_side_effect`) → **allow**.
5. Else → **deny** (`policy.denied.no_grant`).

Authorization runs first. If it allows (or requires approval), constraint validation
runs next — schema, units, range, rate, slew, preconditions, freshness. Both must pass.
A command requiring approval is validated before it waits for approval, and its
preconditions are evaluated again at dispatch.

## 5. Approval

An approval is a durable record, one per approved command, kept in the journal with the
command (ADR-0017). It has a status:

```text
PENDING ──► APPROVED ──► CONSUMED
   │            │
   ├──► DENIED  ├──► EXPIRED
   ├──► EXPIRED └──► REVOKED
   └──► REVOKED
```

`CONSUMED`, `DENIED`, `EXPIRED` and `REVOKED` are final. No other transition exists, and
nothing returns to `PENDING`.

- **Bound to the command.** The record holds a fingerprint: the SHA-256 of the canonical
  JSON (sorted keys, no whitespace) of `{v:1, requester, device, capability, operation,
  value, params}`. Idempotency key, correlation id and deadline are not part of it. The
  fingerprint is recomputed from the stored command when the approval is spent, and a
  mismatch spends nothing.
- **Who decides.** A principal that is not an agent and that policy lists as an approver,
  other than the requester, with a credential whose scopes reach the device, capability and operation.
  An agent credential never decides. The decision is stored in the record and in the
  receipt (`policy.approved_by`).
- **Expiry.** An approval for a held command expires with the command's deadline. One
  requested ahead of time lives for `ttl` (default 15 minutes, at most 24 hours). Expiry
  is checked when the approval is spent, so a late approval is never used.
- **Spent once.** Spending is one transaction: check `APPROVED`, unexpired, same requester
  and fingerprint; mark `CONSUMED`; move the command on. A second attempt, or a concurrent
  one, finds `CONSUMED`.
- **Policy still applies.** An approval answers "may this proceed despite a
  `require_approval` grant". It does not skip authorization, scopes, constraints or
  preconditions, all evaluated again at the moment of use.
- **Two ways in, one record.** A command policy holds gets an approval bound to it, which
  an approver approves by command id or approval id. Or the requester asks first
  (`approvals request`), an approver approves, and the requester submits the same command
  naming the approval (`exec --approval`).
- **Withdrawal.** The requester may revoke their own approval; an approver may revoke any.
  Revoking a credential revokes its open approvals and cancels the commands waiting on
  them. A command already dispatched is not recalled.

Every transition is recorded in the audit chain in the same transaction as the change.

## 6. Out of scope for v0.1

Time windows, attribute conditions, delegated administration and an external policy
language (Cedar). M8 added scoped credentials (section 7) and approval records (section
5); it did not add a policy language, and Cedar remains a candidate for a later milestone.

## 7. Credentials and scopes

Every request is attributed to a principal proved by a credential. Nothing in a request
body names the caller.

- **Format.** `omhs_` followed by 64 hex digits (256 bits from the OS random source).
  Opaque: not a JWT, nothing can be read from it. Only its SHA-256 is stored, compared in
  constant time over every record. Each credential has a random id (`tok_…`) unrelated to
  the secret. The secret is shown once, when created or rotated, and is never logged.
- **Kinds.** The principal id says what it is: `agent:…` is an agent, `service:…` is a service,
  and any other prefix (`operator:…`, `user:…`) is human. An agent cannot decide approvals or
  administer the gateway. One principal has one role.
- **Scopes.** A scope is `device:capability:operations` (patterns allowed). A credential
  can reach only what its scopes list. The scope is a ceiling intersected with policy: a
  call needs a scope **and** a grant, and neither widens the other. A credential with no
  scopes authorizes nothing, and so do credentials written before M8. `--any-scope` is an
  explicit, visible choice for the operator.
- **Lifetime.** Optional expiry (`--ttl`); revocation; rotation (a new secret, the same
  principal and scopes, the old one revoked). Expiry and revocation are checked on every
  request with no cache, so **revocation takes effect on the next request**. Revoked
  credentials stay on record so that using one is distinguishable from a guess; the first
  refused use is audited, once.
- **Fail closed.** An unknown, malformed, expired or revoked credential gets 401 before
  any method runs. A token file that cannot be read stops the gateway from starting.
- **Evaluation order.** Credential → scope → grants (section 4) → constraints → approval →
  dispatch. A scope denial is `auth.scope.denied` and does not reveal whether the device
  exists.
- **Audit.** Command and approval records carry the principal, the credential id and the
  authentication method. In-process callers (tests, conformance, embedding) are recorded
  as method `in_process`.
