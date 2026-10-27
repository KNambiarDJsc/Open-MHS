# Errors

## Error object

Schema: `schemas/common.schema.json#/$defs/error`.

```json
{
  "code": "command.value.out_of_range",
  "message": "48 V is outside the allowed range 0..12 V",
  "pointer": "/value",
  "rule_id": "grant.lab-assistant.psu-setpoint",
  "details": { "min": 0, "max": 12, "unit": "V" }
}
```

- `code` is stable and machine-readable. Clients branch on `code`, never on `message`.
- `pointer` is a JSON Pointer into the document the error is about, when applicable.
- `rule_id` names the policy rule involved, when applicable.
- Codes are only ever added. A code's meaning never changes once released.

## Code registry

### Descriptor validation (M1)

Every code below is produced by at least one fixture in `examples/invalid/`. A test
enforces this in both directions.

| Code | Rule |
|---|---|
| `descriptor.schema` | Structural JSON Schema violation |
| `descriptor.version.unsupported` | Spec version not supported |
| `descriptor.component.duplicate` | Component id declared twice |
| `descriptor.component.unknown` | Capability or relationship references an undeclared component |
| `descriptor.capability.duplicate` | Capability name declared twice |
| `descriptor.relationship.duplicate` | The same relationship declared twice |
| `descriptor.relationship.field_missing` | Relationship without the field its kind needs (`target`, `resource` or `frame`) |
| `descriptor.relationship.field_not_applicable` | Relationship field its kind does not use |
| `descriptor.relationship.self` | Relationship whose target is the device itself |
| `descriptor.relationship.unknown_capability` | Frame relationship names a capability that does not exist |
| `descriptor.capability.invalid_operation` | Operation not allowed for the capability kind |
| `descriptor.capability.value_missing` | Property without `value` |
| `descriptor.capability.field_not_applicable` | Field not used by this kind (e.g. `params` on a property) |
| `descriptor.capability.side_effect_mismatch` | `side_effect` inconsistent with whether the capability can change state |
| `descriptor.capability.verification_missing` | Writable/invokable capability without `verification` |
| `descriptor.quantity.unknown` | Unknown quantity kind |
| `descriptor.unit.unknown` | Unit not in the unit table |
| `descriptor.unit.quantity_mismatch` | Unit measures a different quantity kind |
| `descriptor.constraint.range_inverted` | `range.min` > `range.max` |
| `descriptor.constraint.not_applicable` | Constraint used on a value type it doesn't apply to |
| `descriptor.stop.not_action` | `stop` declared on a property or event |
| `descriptor.stop.guarded` | A stop declares `preconditions` or a `rate_limit` |
| `descriptor.verification.not_applicable` | `verification` on a capability that can't change state |
| `descriptor.verification.unknown_capability` | Readback capability doesn't exist |
| `descriptor.verification.not_readable` | Readback capability is not a readable property |
| `descriptor.verification.capability_missing` | Action readback without a `capability` |
| `descriptor.verification.expectation_missing` | Action readback without `expect` or `expect_param` |
| `descriptor.verification.ambiguous_expectation` | Both `expect` and `expect_param` given |
| `descriptor.verification.unknown_param` | `expect_param` names an undeclared parameter |
| `descriptor.verification.type_mismatch` | Expectation not comparable with the readback property |
| `descriptor.precondition.duplicate_id` | Precondition id repeated within a capability |
| `descriptor.precondition.unknown_capability` | Precondition target doesn't exist |
| `descriptor.precondition.not_readable` | Precondition target is not a readable property |
| `descriptor.precondition.type_mismatch` | Value or operator incompatible with the target |

### Commands

Produced by the engine (M2). Errors that deny a command appear in the receipt's
`policy.errors`; errors about the request itself are returned to the caller.

| Code | Meaning |
|---|---|
| `command.schema` | Structural violation of the command schema; no command is created |
| `command.not_a_command` | A `read` was submitted as a command; reads use the read API |
| `command.unknown` | No command with that id |
| `command.invalid_state` | The command's state does not allow the requested action (approve, cancel, reconcile, resolve) |
| `command.idempotency.conflict` | Idempotency key reused with a different request |
| `command.device.unknown` | Target device not registered |
| `command.capability.unknown` | Target capability not on the device |
| `command.operation.unsupported` | Capability does not support the operation |
| `command.value.type_mismatch` | Value has the wrong type (including a bare number where a quantity is required) |
| `command.value.unit_unknown` | Value uses an unknown unit |
| `command.value.unit_mismatch` | Value's unit is not convertible to the capability's |
| `command.value.out_of_range` | Value outside the declared range (or string longer than `max_length`) |
| `command.value.not_allowed` | Enum value not among the declared values |
| `command.param.missing` | A declared action parameter was not given (all are required in v0.1) |
| `command.param.unknown` | A parameter the action does not declare |
| `command.rate_limited` | Rate limit exceeded |
| `command.slew_exceeded` | Change exceeds the slew limit |
| `command.precondition.failed` | A precondition does not hold; `rule_id` is the precondition id |
| `command.state.stale` | Required state is missing, stale, or not `GOOD`; `rule_id` names the precondition when there is one |
| `command.verification.invalid` | The descriptor's verification cannot be applied to this request (indicates a descriptor defect) |
| `command.not_reconcilable` | `UNKNOWN_OUTCOME` cannot be resolved automatically (no readback declared, or no usable reading) |
| `command.outcome_unresolved` | An earlier write or invoke to this capability is still `UNKNOWN_OUTCOME`; reconcile or resolve it first ([lifecycle.md](lifecycle.md) §3 rule 3). `details.unresolved` lists the command ids |
| `command.resolve.reason` | `commands.resolve` without a reason |
| `policy.resolve.not_human` | An agent credential tried to resolve an unknown outcome |

A deadline that passes before dispatch is reported by the `EXPIRED` state, not an error code.

### Policy

| Code | Meaning |
|---|---|
| `policy.denied.no_grant` | No grant allows the operation |
| `policy.denied.explicit` | A `deny` grant matched |
| `policy.denied.side_effect` | Grant does not cover this side-effect class |
| `policy.approval.not_approver` | The principal is not configured as an approver |
| `policy.approval.self` | A principal tried to approve its own command |
| `policy.approval.not_human` | An agent credential tried to decide an approval |
| `policy.cancel.not_allowed` | Only the requester or an approver can cancel |
| `policy.grant.invalid` | A grant has an empty id, pattern or operation list |
| `policy.grant.duplicate` | A grant with that id already exists |
| `policy.grant.unknown` | No grant with that id |

### Credentials and scopes (M8)

| Code | Meaning |
|---|---|
| `auth.scope.denied` | The credential's scopes do not reach this device, capability and operation; checked before grants, and does not reveal whether the device exists |
| `credential.expired` | The credential is past its expiry (HTTP 401) |
| `credential.revoked` | The credential was revoked (HTTP 401) |
| `credential.principal.invalid` | Not a valid principal id |
| `credential.principal.reserved` | The gateway operator's own principal cannot be issued |
| `credential.role.invalid` | The operator role for an `agent:*` principal |
| `credential.role.conflict` | The principal already holds the other role |
| `credential.scope.invalid` | A scope with an empty pattern or operation list |

### Approvals (M8)

| Code | Meaning |
|---|---|
| `approval.unknown` | No approval with that id that the caller may see |
| `approval.pending` | The approval has not been decided yet |
| `approval.denied` | The approval was denied |
| `approval.expired` | The approval ran out before it was used |
| `approval.revoked` | The approval was withdrawn |
| `approval.consumed` | The approval was already used |
| `approval.mismatch` | The command is not the one approved (fingerprint, requester or binding) |
| `approval.invalid_transition` | The change is not allowed from the approval's current status |
| `approval.not_required` | Policy allows the command without approval; submit it directly |
| `approval.ttl.invalid` | `ttl_ms` outside 1 ms to 24 h |

### Adapter signatures (M8)

Reported by the adapter host when it refuses to start an adapter (spec/permission-manifest.md).

| Code | Meaning |
|---|---|
| `manifest.unsigned` | The manifest has no signature and unsigned adapters are not allowed |
| `manifest.signature.invalid` | The signature does not verify over the manifest's canonical form |
| `manifest.signature.algorithm` | An algorithm other than `ed25519` |
| `manifest.signature.no_artifacts` | The signature covers no files |
| `signer.unknown` | The signing key is not in the trusted-key registry |
| `signer.revoked` | The signing key was revoked |
| `signer.not_allowed` | The key is not trusted for this adapter id |
| `signer.invalid` | The registry holds an unusable public key |
| `artifact.missing` | A signed file is missing |
| `artifact.modified` | A signed file's contents do not match its digest |
| `artifact.invalid` | A path outside the package, a link, or not a regular file |
| `artifact.uncovered` | The launch command runs a package file the signature does not cover |

### Graph and topology (M9)

| Code | Meaning |
|---|---|
| `graph.target.unregistered` | A relationship names a device that is not registered (reported by `graph.get`; does not stop the gateway) |
| `graph.contains.cycle` | A device contains itself through other devices (reported by `graph.get`) |
| `topology.device.unknown` | A topology relationship starts from a device that is not registered (stops the gateway) |
| `topology.target.unknown` | A topology relationship targets a device that is not registered (stops the gateway) |

A topology relationship that breaks a descriptor relationship rule is reported with the
code the descriptor relationship rule uses (see Descriptor validation above).

### Availability (ADR-0019)

| Code | Meaning |
|---|---|
| `device.unavailable` | The device did not answer a probe made just before the command, so nothing was sent. Writes and invokes are refused until it answers; stops never are. `details.since` is when it went offline |

### Leases (M9)

| Code | Meaning |
|---|---|
| `lease.held` | The device is claimed by someone else's live lease; writes and invokes are refused until it ends (stops are not leased) |
| `lease.conflict` | Something the lease would claim is already claimed by a live lease; nothing was taken |
| `lease.unknown` | No lease with that id |
| `lease.ended` | The lease was released or is past its deadline |
| `lease.not_holder` | Only the holder renews a lease; only the holder or an approver releases it |
| `lease.not_authorized` | The caller could not operate every device the lease would claim |
| `lease.device.unknown` | A requested device is not registered or not within the credential's scopes |
| `lease.resource.unknown` | No registered device shares the requested resource |
| `lease.ttl.invalid` | The lease duration is zero or longer than 24 hours |
| `lease.empty` | The request names no device and no resource |

### Plans (M9)

| Code | Meaning |
|---|---|
| `plan.empty` | The plan has no steps |
| `plan.too_many_steps` | More than 100 steps |
| `plan.step.id_invalid` | A step id is empty, too long, or uses other characters than letters, digits, `-`, `_`, `.` |
| `plan.step.duplicate` | Two steps share an id |
| `plan.step.unknown_dependency` | `after` names a step that does not exist (or the step itself) |
| `plan.cycle` | Steps depend on each other in a cycle |
| `plan.step.invalid_request` | A step's request or compensation is not a valid write or invoke, or carries its own `idempotency_key` |
| `plan.precondition.unknown` | A step precondition reads something that is not a readable property of a registered device |
| `plan.precondition.failed` | A step precondition did not hold, or its reading was missing or stale, just before the step; nothing was sent |
| `plan.step.failed` | A step's command ended without success and carried no more specific error |
| `plan.step.uncertain` | A step's command may or may not have taken effect; the plan waits for a decision |
| `plan.unknown` | No plan with that id visible to the caller |
| `plan.decision.not_approver` | Only a human approver decides how a plan continues |
| `plan.decision.not_awaiting` | The plan is not waiting for a decision |
| `plan.decision.uncertain` | `resume`, `retry` and `compensate` wait until the uncertain step is reconciled |
| `plan.decision.not_allowed` | The decision does not fit the step's result (e.g. resuming past a failed step) |
| `plan.cancel.not_allowed` | Only the plan's submitter or an approver cancels it |

A step that fails reports its command's own error (for example `lease.held`,
`policy.denied.no_grant` or `adapter.rejected`). Those codes are not wrapped.

### Adapter

The class decides the lifecycle state (spec/lifecycle.md §3).

| Code | Meaning | Resulting state |
|---|---|---|
| `adapter.timeout` | No response within the deadline | `TIMED_OUT` → `UNKNOWN_OUTCOME` |
| `adapter.disconnected` | Connection lost | `UNKNOWN_OUTCOME` |
| `adapter.malformed_response` | Response could not be interpreted | `UNKNOWN_OUTCOME` |
| `adapter.internal` | Adapter fault (crash, panic, bug) | `UNKNOWN_OUTCOME` |
| `adapter.rejected` | Device refused the command | `REJECTED` (or `FAILED` after `ACCEPTED`) |
| `adapter.device_failure` | Device reported it could not carry out the command | `FAILED` |
| `adapter.not_sent` | Adapter guarantees nothing reached the device | `FAILED` |
| `adapter.unsupported` | Adapter does not implement the capability or operation | `FAILED` |

### Registry and gateway

| Code | Meaning |
|---|---|
| `registry.invalid_descriptor` | An adapter's descriptor failed validation; details carry the descriptor validation issues |
| `registry.adapter_mismatch` | Descriptor names a different adapter than the one serving it |
| `registry.duplicate_device` | A device with that id is already registered |
| `gateway.restarted` | Command denied because the gateway restarted before validation finished |
| `gateway.journal` | The journal could not be read or written |
| `gateway.unauthorized` | Missing or unknown bearer token (HTTP 401) |
| `gateway.forbidden` | The call needs the operator role |
| `gateway.storage` | The policy or token file could not be written |
| `gateway.internal` | Internal gateway fault (e.g. a panicked task) |
| `gateway.token.unknown` | No token with that id |
| `gateway.not_simulation` | Fault injection requested on a gateway not started with `--sim` |
| `sim.fault.unknown` | Unknown fault name, or a fault that needs a value was given none |
| `mcp.operator_token` | The MCP server was given an operator token and refuses to act without `--allow-operator` (an MCP tool error, not an API error) |

API-level errors and the JSON-RPC envelope are described in [api.md](api.md).
