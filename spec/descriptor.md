# Device descriptors

A descriptor is a JSON document that tells the gateway what a device is and what it
can do. Schema: `schemas/descriptor.schema.json`. Examples: `examples/valid/`.

A descriptor is a **claim** made by whoever wrote it (device vendor, adapter author,
operator). It describes capabilities and physical constraints. It never grants
permissions — see [authorization.md](authorization.md).

## 1. Top level

| Field | Required | Meaning |
|---|---|---|
| `openmhs` | yes | Spec version, e.g. `"0.1"` |
| `device` | yes | `id`, `manufacturer`, `model`; optional `name`, `serial`, `firmware`, `description` |
| `adapter` | yes | `id` and `version` (semver) of the adapter that drives this device, plus adapter-specific `config`, and optionally the `conformance` level (`L0`–`L3`) the adapter's author claims. That level is a claim like the rest of the descriptor; the conformance report is the evidence ([conformance.md](conformance.md)) |
| `components` | no | Named parts of the device (`id`, `name`, `description`). Capabilities may reference one. |
| `capabilities` | yes | At least one capability |
| `extensions` | no | Vendor data, keys namespaced `x-<vendor>` |

## 2. Capabilities

A capability is something the device can report or do. There are three kinds:

| Kind | Allowed operations | Shape |
|---|---|---|
| `property` | `read`, `write`, `subscribe` | A single `value` |
| `action` | `invoke` | Named `params`, optional named `result` |
| `event` | `subscribe` | Optional named `payload` |

Telemetry streams are properties with `subscribe`; there is no separate stream kind.

Common fields:

| Field | Meaning |
|---|---|
| `name` | Dotted name, unique within the descriptor |
| `version` | Optional capability semantics version (`MAJOR.MINOR`) |
| `component` | Optional component id |
| `side_effect` | `none`, `reversible`, `irreversible`, `hazardous` |
| `idempotent` | Repeating the same write/invoke has the same effect as doing it once |
| `freshness.max_age_ms` | Readings older than this are stale |
| `rate_limit` | At most `max` mutating commands per `per_ms` |
| `slew` | Numeric properties only: at most `max_change` (canonical unit) per `per_ms` |
| `preconditions` | Conditions on other properties that must hold before a write/invoke |
| `verification` | How the gateway confirms a write/invoke took effect |
| `timing` | `timeout_ms` (required) and optional `expected_ms` for dispatch |

### 2.1 Side-effect classes

- `none` — cannot change device state. Required for capabilities with no `write` or
  `invoke`, and forbidden for capabilities that have one.
- `reversible` — changes state that can be changed back (a setpoint, a valve).
- `irreversible` — cannot be undone (dispense a reagent, cut material).
- `hazardous` — could harm people or equipment if misused (motion, high energy).

Policy can use the class (e.g. require approval for `hazardous`). The class is
information for policy, not a safety rating.

## 3. Value specs

Used by `value`, `params`, `result`, and `payload`.

| `type` | Required fields | Optional fields | Wire value |
|---|---|---|---|
| `number` | `quantity`, `unit` | `range` | `{"value": 5.0, "unit": "V"}` |
| `integer` | — | `quantity`, `unit`, `range` | `42` (or a quantity if it has a unit) |
| `boolean` | — | — | `true` |
| `enum` | `values` | — | `"idle"` |
| `string` | — | `max_length` | `"text"` |

Numbers always travel with their unit. A bare number is never interpreted as a
quantity. `range` bounds are in the spec's declared `unit`. See [units.md](units.md).

## 4. Preconditions

A precondition is a deterministic comparison against the current value of a readable
property. All preconditions of a capability must hold (logical AND).

```json
{ "id": "homed", "capability": "axis.homed", "op": "eq", "value": true, "max_age_ms": 500 }
```

- `op`: `eq`, `ne`, `lt`, `le`, `gt`, `ge`, or `in` (with an array `value`).
- Ordering operators apply only to numbers and integers.
- Quantity values may use any unit convertible to the target's unit.
- `max_age_ms` overrides the target's freshness. If no fresh `GOOD`-quality reading
  exists, the precondition **fails** (fail closed).

There is deliberately no expression language with variables, functions or loops.
Combining conditions with OR is not supported in v0.1.

## 5. Verification

Every capability that can `write` or `invoke` MUST declare `verification`:

- `{"mode": "readback", ...}` — after the device reports the change applied, the gateway
  reads a property and compares it to the expectation.
  - `capability`: the property to read. Defaults to the capability itself for
    properties. Actions MUST name one.
  - Expectation: for property writes, the written value. Otherwise `expect` (a fixed
    value) or `expect_param` (an action parameter's value). Actions MUST give one.
  - `tolerance`: `abs` and/or `rel`, for numbers.
  - `settle_ms`: wait before the first readback. `timeout_ms`: give up after this.
- `{"mode": "none", "reason": "..."}` — the effect cannot be read back. Commands that
  apply end in `UNVERIFIABLE`, not `VERIFIED`, and the receipt says so.

## 6. Validation rules

A descriptor is valid if it passes the JSON Schema **and** all of the following.
Each rule's error code is listed in [errors.md](errors.md).

1. `openmhs` is a supported spec version.
2. Component ids are unique; a capability's `component` refers to a declared component.
3. Capability names are unique.
4. Operations are allowed for the capability's kind (table in §2).
5. Properties have `value`. Fields that don't apply to a kind are absent.
6. Units are in the unit table; quantity kinds are known; a unit matches its quantity.
7. `range` only on numeric values, with `min ≤ max`. `values` only on enums,
   `max_length` only on strings, quantity/unit only on numeric values.
8. `slew` only on numeric properties.
9. `side_effect` is `none` exactly when the capability cannot write or invoke.
10. Capabilities that write or invoke declare `verification`; others do not.
11. Readback targets exist and are readable properties; actions name one and give an
    expectation; `expect_param` names a declared parameter; expectations are
    type-compatible with the target.
12. Precondition ids are unique per capability; targets exist and are readable
    properties; values are type-compatible with the operator and target.

Implementations MUST report all issues found, not only the first.
