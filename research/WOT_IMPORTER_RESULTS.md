# W3C Thing Description importer: results

Phase 3. Can a generic importer turn a Thing Description into a validated OpenMHS capability
contract and an MCP tool with no per-device code? **Partly, and the part it cannot do is the part
that matters for safety.** All results are from simulators.

```text
Thing Description ─▶ td_import.map_thing ─▶ draft descriptor + bindings + per-affordance findings
                                                     │
                       human review (overrides) ─────┘   what a TD does not say
                                                     ▼
                       OpenMHS validates the descriptor ─▶ MCP tools ─▶ deny-by-default grants
```

Code: [`adapters/python/wot/`](../adapters/python/wot) (`td_import.py` mapping, `import_td.py`
command line, `wot_adapter.py` runtime). Tests: `python -m unittest adapters/python/wot/test_td_import.py`
(25 tests, offline, pinned to the captured OpenFlexure descriptions).

## The rules the importer follows

A Thing Description describes an interface. It does not say what an action does to the world, so:

| Affordance | Rule |
|---|---|
| Readable property | Becomes a **read-only** capability. |
| Writable property | Still read-only; reported `writable_not_exposed`. Writing changes the world. |
| Action | **Not exposed** unless the TD declares `safe: true` or a reviewer supplies `side_effect`, `idempotent` (if the TD omits it), a time bound and parameter limits. A name that sounds harmless (`get_position`) is not trusted. |
| Numeric action parameter | Must have a range (from the TD or the reviewer) or the reviewer must accept it as unbounded by name (`unbounded_ok`). |
| Parameter with a default | Left to the device's default; reported. |
| Undeclared keyword arguments | Reported; the action cannot be called until a reviewer declares them. |
| Nullable / compound value | **Unsupported**, with the reason; never approximated. |
| Event | **Unsupported**, reported (events are not mapped yet). |
| Security | `nosec`, `basic`, `bearer` in a header are supported (credentials come from the gateway's environment, never the manifest). Any other scheme **blocks the import** and says why. |
| Anything listed | Grants nothing. OpenMHS stays deny-by-default; an exposed capability still needs a grant. |

## Mapping coverage: the OpenFlexure simulator

Inputs: the simulator's 14 Things, **95 properties, 50 actions, 0 events**
(`research/fixtures/openflexure-sim-thing-descriptions.json`). Maps are probed once with a read
to learn their keys (`--probe`).

| | Result |
|---|---|
| Properties that became capabilities | **83 of 95** properties' worth (4 axis maps flattened into per-axis capabilities) |
| Properties unsupported | 23 (12 nullable, 11 compound: arrays, free objects) |
| Capabilities carrying a numeric range from the TD | 29 |
| Capabilities carrying a **real unit** | **0** (no TD property declares one; 37 numeric ones fall back to dimensionless `1` and are flagged `unit_missing`) |
| Actions exposed automatically | **0 of 50** (none declares `safe` or `idempotent`) |
| Actions needing review | 50 (5 of them accept undeclared keyword arguments) |
| Writable properties held back | 42 |
| Events reported as unsupported | 0 present |

Without probing, 71 map and 27 are unsupported. The four probed maps are flagged
`keys_from_sample`: their keys were taken from one live read and could change.

### What the numbers say

- **Reading a LabThings device needs no custom code.** A stage with 12 capabilities became a
  registered, grantable, auditable OpenMHS device from a URL and one command.
- **None of the safety-relevant facts come from the description.** Units, physical effect,
  idempotency and verification are all absent from OpenFlexure's descriptions, so every action stays
  behind review. This is the intended outcome, not a failure: the importer fails *closed*.
- **Compound and nullable values are a real coverage cost** (23 of 106 property mappings), concentrated in
  camera settings. A richer OpenMHS value model would recover some; that is a design decision, not
  an importer fix.

## Onboarding effort: measured as decisions, not minutes

Time was not measured, and I do not claim a speed-up. What can be counted:

| | Hand-written OpenMHS profile | Imported |
|---|---|---|
| Transcribing names, types, paths, JSON pointers for readable properties | by hand, for each | **generated** (83 capabilities, ~1,790 lines of descriptor JSON across the 14 Things) |
| Deciding side effect, idempotency, limits, timing, verification for each action | by hand | **still by hand**, in an override file: 44 lines for one bounded stage move (two per-axis variants) |
| For comparison: a small hand-written HTTP profile (`led-controller.json`) | 91 lines for a few capabilities | |

The importer removes **transcription** work and leaves the **judgement** work, and makes the judgement
work visible and reviewable in one file. The judgement work is most of the safety value.

## Dynamic capability updates

Not implemented. The mapping is produced at import time; a Thing that changes its description needs
a new import and a gateway restart. A descriptor must not change between restarts (the adapter
contract says so), which is why this is deliberate. Detecting drift (a Thing whose description no
longer matches the imported contract) is the useful next step and is not built.

## Bugs found by running it against real servers

Every one of these passed review by reading and failed on contact:

1. **A server-supplied absolute URL pointed past the proxy**; the adapter's permission check
   refused it, and the refusal was raised as `not_sent` for an action already accepted (see
   ASYNC_ACTION_RELIABILITY.md). Fixed: only the URL's *path* is used, and no error after acceptance
   is `not_sent`.
2. **`x_wot` is not a valid extension key** (`x-wot` is): caught by OpenMHS's own validator.
3. **`{"mode": "none"}` needs a `reason`**: the validator rejected the importer's default.
4. **`safe: true` cannot be `side_effect: none`** in OpenMHS (an `invoke` is at least `reversible`):
   mapped accordingly, with the TD's claim kept as provenance.
5. **A numeric reading with a unit must be a quantity object**, not a bare number: found only by the
   second device, because every OpenFlexure property read earlier was a boolean or integer.
6. **A Thing that rejects the credentials (401) was reported `ONLINE`.** Now `OFFLINE` with the
   reason.
7. **Events and security were silently ignored** on the first run against an independently authored
   description. Now reported; unsupported security blocks the import.

OpenMHS's validator caught three of the seven before any device saw them, which is the argument for
importing into a strict contract rather than passing descriptions through.

## What this does not show

- Only the HTTP binding, JSON content and the property/action affordances. No MQTT, CoAP, or events.
- Two source devices (a LabThings server and one hand-written description). A wider survey of real
  Thing Descriptions would very likely find more shapes the importer rejects.
- The importer's value judgements (`reversible` for a declared-safe action, dimensionless for a
  missing unit) are conservative choices, not truths about any device.
- A reviewer who rubber-stamps `unbounded_ok` defeats the limit check; the rule makes the decision
  visible, not correct. The generalization run shows what that costs (GENERALIZATION_RESULTS.md).
