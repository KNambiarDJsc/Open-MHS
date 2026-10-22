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
| `descriptor.component.unknown` | Capability references an undeclared component |
| `descriptor.capability.duplicate` | Capability name declared twice |
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
| `command.invalid_state` | The command's state does not allow the requested action (approve, cancel, reconcile) |
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

A deadline that passes before dispatch is reported by the `EXPIRED` state, not an error code.

### Policy

| Code | Meaning |
|---|---|
| `policy.denied.no_grant` | No grant allows the operation |
| `policy.denied.explicit` | A `deny` grant matched |
| `policy.denied.side_effect` | Grant does not cover this side-effect class |
| `policy.approval.not_approver` | The principal is not configured as an approver |
| `policy.approval.self` | A principal tried to approve its own command |
| `policy.cancel.not_allowed` | Only the requester or an approver can cancel |

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
