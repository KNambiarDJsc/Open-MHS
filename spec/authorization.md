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

An approval is recorded by an authorized human principal and stored in the receipt
(`policy.approved_by`). Approvals expire with the command's deadline. A principal may
not approve its own command.

## 6. Out of scope for v0.1

Time windows, attribute conditions, roles and delegated administration are planned for
M8, where a policy language (Cedar) is re-evaluated.
