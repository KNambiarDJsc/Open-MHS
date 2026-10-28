# Procedures

A procedure is a stored, versioned plan template with typed parameters. Running one fills
the template and submits it as an ordinary plan ([plans.md](plans.md)) as the caller. Every
step is the caller's command, with every check. **A procedure grants nothing**: a caller who
may not perform a step is refused, as with any plan.

## 1. Shape

```json
{
  "name": "psu.set-output",
  "version": 1,
  "description": "set the supply's voltage",
  "params": {
    "voltage": { "type": "quantity", "unit": "V", "min": 0, "max": 12 },
    "enable":  { "type": "boolean", "default": true }
  },
  "plan": { "steps": [
    { "id": "set", "request": { "target": { "device": "sim.psu-01", "capability": "power.output.voltage.setpoint" },
                                "operation": "write", "value": { "$param": "voltage" } } },
    { "id": "on", "after": ["set"],
      "request": { "target": { "device": "sim.psu-01", "capability": "power.output.enabled" },
                   "operation": "write", "value": { "$param": "enable" } } }
  ] }
}
```

| Field | Meaning |
|---|---|
| `name` | 1–64 letters, digits, `-`, `_`, `.` |
| `version` | From 1. Versions are never edited or replaced; a change is a new version |
| `params.<name>.type` | `quantity` (`{"value", "unit"}` in exactly `unit`), `integer`, `boolean` or `string` |
| `params.<name>.unit` | Required for a quantity |
| `params.<name>.min`, `max` | Inclusive bounds for numbers |
| `params.<name>.default` | Used when the caller gives none; without one, the parameter is required |
| `plan` | A plan in which an object `{"$param": "<name>"}` stands for that parameter's value |

A procedure is checked when it is stored:
- the name and version are valid;
- every placeholder names a declared parameter, and every parameter is used;
- a quantity has a unit and a default fits its parameter;
- the template expands into a valid plan.

## 2. Lifecycle

| Status | How | Who |
|---|---|---|
| `proposed` | `procedures.propose` | Anyone, agents included |
| `approved` | `procedures.approve` of a proposed version, or `procedures.add` directly | Operator only |

Only approved versions run. There is no MCP tool to approve. Each stored version records:
- `sha256`: the hash of its canonical JSON (keys sorted, no whitespace);
- `proposed_by` and `proposed_at`;
- `approved_by` and `approved_at`, once approved.

Procedures are kept in `procedures.json` in the gateway home and survive restarts.

## 3. Running

`procedures.check` and `procedures.run` take `name`, an optional `version` (default: the
newest approved one) and `params`.

1. Parameters are checked against their declarations: none unknown, none missing, the
   right type and exact unit, inside the range. A failure is refused before anything is
   created.
2. The template is filled. The plan's description starts with
   `procedure <name> v<version> (sha256 <first 16 hex digits>)`, so every step's
   commands trace back to the exact version.
3. `check` dry-runs the plan as `plans.preflight` does. `run` submits it as `plans.submit`
   does, and records `procedure.run` with the plan id.

The same version and parameters always produce the same plan.

## 4. Errors

`procedure.unknown`, `procedure.not_approved`, `procedure.exists`, `procedure.approved`
(approving an approved version), `procedure.version`, `procedure.name`, `procedure.param`
(a malformed declaration), `procedure.param.invalid`, `procedure.param.missing`,
`procedure.param.unknown`, `procedure.plan` (the template is not a plan). Errors from the
plan itself are those of [plans.md](plans.md).
