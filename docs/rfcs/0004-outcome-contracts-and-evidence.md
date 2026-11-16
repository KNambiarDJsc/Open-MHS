# RFC 0004: outcome contracts, evidence records and verdicts

| | |
|---|---|
| Status | **Draft, with a v0 prototype.** Nothing here changes the command lifecycle, the API or any released contract |
| Prototype | `crates/openmhs-outcome`, `openmhs outcome verify` |
| Evaluation | [EVALUATION](../../research/outcome-verifier/EVALUATION.md), committed before any result |

## Summary

A command that ends `VERIFIED` says one thing: the device read back what was asked. It does not say the
physical outcome the request was for was achieved. "Motor reached 20°" is not "the sample is in the fixture",
and a robot action that reports `SUCCEEDED` can be wrong about the world.

This RFC adds an independent, offline check. An **outcome contract** names the evidence a declared outcome
needs and the conditions it must meet. A set of **evidence records**, from OpenMHS or from any other system,
is checked against it, and the result is one of three **verdicts**:

| Verdict | Meaning |
|---|---|
| `SATISFIED` | Every condition holds, on evidence that passed every validity rule |
| `VIOLATED` | Valid evidence shows that at least one condition does not hold |
| `UNKNOWN` | Neither can be established. The report says which evidence is missing or invalid, and why |

The check is deterministic: the same contract and evidence always give the same report, byte for byte, with
digests of both inputs. No model is involved. A model may help write a contract; it never decides a verdict.

## Why

- RFC 0003 §5 already found that `VERIFIED` on a liquid transfer means "the adapter recorded this id", and
  concluded that checking the physical quantity needs an independent measurement.
- The [hardware validation plan](../strategy/HARDWARE_VALIDATION_PLAN.md) already requires a second instrument
  that "does not go through OpenMHS". Nothing yet decides what such evidence establishes.
- Outside OpenMHS: a simulation study of two bimanual tasks found that, of episodes the policy flagged as
  successful, 47.6% (cube transfer) and 31.8% (peg insertion) had failed; independent signals recovered most
  of them ([Bedi 2026](https://arxiv.org/abs/2606.03134), single seed, simulated, numbers the author calls
  optimistic). Vision-language models judging success reached at best 0.77 balanced accuracy, with a bias
  toward "success" when evidence is ambiguous ([FailBench](https://arxiv.org/abs/2609.03611)). That bias is
  the reason the verdict here is not left to a model, and why `UNKNOWN` is a first-class answer.

## What it is not

- **Not a workflow language.** A contract describes what must be true afterwards and what evidence counts.
  Planning and execution stay in MoveIt, ROS 2, MADSci, vendor software, or OpenMHS plans.
- **Not in the control path.** It runs after the fact, offline, on files. In this prototype it never sends
  anything to a device. That is the "evidence-only" mode: it can sit beside a system that does not use OpenMHS.
- **Not a judge of independence.** A record's `source.kind` (say, `independent_sensor`) is the word of whoever
  produced it. Like `simulated_source` in the ROS 2 adapter, nothing checks it. Signed evidence is future work.
- **Not a safety system,** and not validation for any regulated use.

## Evidence records (v0)

One JSON object per record, one record per line (JSONL). Unknown fields are refused, so a misspelt field is an
error rather than silently ignored.

```json
{
  "id": "dmm-17",
  "signal": "output.voltage",
  "value": { "value": 1.998, "unit": "V" },
  "source": { "system": "bench", "device": "dmm-1", "kind": "independent_sensor", "provenance": "device" },
  "time": { "source": "2026-11-10T09:00:01.250Z", "ingest": "2026-11-10T09:00:01.300Z",
            "clock_domain": "bench-host", "uncertainty_ms": 5 },
  "quality": "GOOD",
  "refs": { "attempt": "attempt-7", "command": "0193…", "external": "ros:action:8f23" },
  "relations": [ { "type": "wasInformedBy", "id": "cmd-0193" } ]
}
```

| Field | Rule |
|---|---|
| `id` | Unique in the set. A duplicate is an input error, not evidence |
| `signal` | What was measured or reported. Matched exactly |
| `value` | A number, string or boolean, or `{value, unit}` with a UCUM unit ([units.md](../../spec/units.md)) |
| `source.kind` | `controller` (a report of what a system did), `device_state` (the device's own reading), `independent_sensor`, `operator`, `calibration`, `configuration` |
| `source.provenance` | `device` or `simulator`. Carried into the report |
| `time.source` | When the thing was measured, on `clock_domain`'s clock. Required for anything a window applies to |
| `time.uncertainty_ms` | How far that clock may be from true. Missing means unknown, not zero |
| `quality` | `GOOD`, `UNCERTAIN` or `BAD`, as in OpenMHS readings. Only `GOOD` counts |
| `refs` | Identifiers that tie the record to an attempt, a command, or an id in another system |
| `frame` | The coordinate frame the value is in (a ROS `header.frame_id`), when it has one. Added after v0, for ROS 2 data |
| `relations` | Causal links, named after W3C PROV: `used`, `wasGeneratedBy`, `wasDerivedFrom`, `wasInformedBy`. Kept in the report; not interpreted in v0 |

A **calibration** record is a record with `source.kind: calibration`, naming what it applies to and when:

```json
{ "id": "cal-dmm-1", "signal": "calibration", "value": "dc-voltage",
  "source": { "system": "bench", "device": "dmm-1", "kind": "calibration", "provenance": "device" },
  "calibration": { "name": "dc-voltage", "applies_to": "dmm-1",
                   "valid_from": "2026-06-01T00:00:00Z", "valid_until": "2027-06-01T00:00:00Z" },
  "quality": "GOOD" }
```

Records from OpenMHS itself come from an audit export (`openmhs audit export`): each audited read becomes a
`device_state` record, and each command receipt a `controller` record whose value is its state.

## Contracts (v0)

```json
{
  "contract": "psu-setpoint", "version": "0.1",
  "binding": { "correlation": "attempt-7" },
  "timing": { "anchor": "command", "default_window_ms": [0, 2000], "max_clock_uncertainty_ms": 50 },
  "evidence": {
    "command":  { "signal": "command:power.output.voltage.setpoint", "kind": "controller" },
    "readback": { "signal": "power.output.voltage.measured", "kind": "device_state", "bound": false },
    "dmm":      { "signal": "output.voltage", "kind": "independent_sensor", "calibration": ["dc-voltage"] }
  },
  "conditions": [
    { "evidence": "readback", "op": "between", "min": { "value": 1.95, "unit": "V" }, "max": { "value": 2.05, "unit": "V" } },
    { "evidence": "dmm",      "op": "between", "min": { "value": 1.95, "unit": "V" }, "max": { "value": 2.05, "unit": "V" } }
  ]
}
```

- `evidence` names each item the contract needs: its signal, its source kind, optionally a device, a window, and
  required calibrations.
- An item may set `max_unchanged_ms`, for a sensor that must show some variation (a motion-capture pose, a noisy
  measurement): a record whose value has been identical for longer than that is stale, however fresh its timestamp
  (`evidence.stale`, rule 8). Opt-in, because a setpoint or a settled simulator legitimately repeats itself.
- `binding` lists `refs` every record must carry with these values. A record with a different value, or none,
  is not evidence for this attempt. An item with `"bound": false` is exempt: a device's own reading does not know
  which attempt it belongs to, so its device and window identify it instead.
- `timing.anchor` names the item whose time defines the window. Every other item must fall within
  `window_ms` (or `default_window_ms`) of it: `[0, 2000]` is "within two seconds after". A precondition is just
  an item with a window before the anchor.
- `conditions` are predicates on items: `eq`, `ne`, `lt`, `le`, `gt`, `ge`, `between` (inclusive), `in`.
  Quantities are converted to the condition's unit with OpenMHS's unit table; anything else must match exactly.
  Note that the controller's own report is *not* a condition here: a contract author decides whether
  "the controller said so" counts.

## The rules, in order

For each evidence item, the verifier takes every record with the item's signal and kind (and device, if given),
and discards one when, in this order:

1. the item is bound and its `refs` do not carry the contract's `binding` (`evidence.identity_mismatch`,
   `evidence.identity_unbound`);
2. the item names a `frame` and the record is in another frame, or in none (`evidence.frame`). Nothing is
   transformed on the verifier's behalf: a value in the wrong frame is not evidence;
3. its quality is not `GOOD` (`evidence.quality`);
4. it has no source time and a window applies (`time.missing`);
5. its clock uncertainty is stated and larger than `max_clock_uncertainty_ms` (`time.uncertainty`), or its clock
   domain differs from the anchor's and either clock's uncertainty is unknown (`time.clock_domain`). On the same
   clock, two readings are compared directly, and their absolute uncertainty does not matter;
6. its time, widened by the uncertainty of both clocks, is not wholly inside the window
   (`evidence.outside_window`, or `time.ambiguous` if it straddles an edge);
7. a required calibration has no record for its device, or is not valid at the record's time
   (`calibration.missing`, `calibration.expired`, `calibration.not_yet_valid`).

8. the item sets `max_unchanged_ms` and the record's value has been unchanged for longer (`evidence.stale`). Records are
   taken in source-time order; a run is a stretch of consecutive records holding an equal value, and a record's
   unchanged time is its time minus the start of its run. The run is measured over the records the verifier is
   given, so evidence trimmed to a window should keep enough history before it.

The anchor itself passes rules 1 to 5, 7 and 8 (8 only if it opts in). If no record survives for the anchor, the window is undefined and
every other item is undetermined (`timing.anchor_missing`).

Each condition is then evaluated on **every** surviving record for its item. If they all hold, it is true; if
none does, it is false; if they disagree, it is undetermined (`evidence.conflict`). No surviving record: undetermined
(`evidence.missing` if none arrived, `evidence.rejected` if some arrived and every one failed a rule). A unit that cannot be converted, or a value of the wrong type: undetermined (`evidence.unit`,
`evidence.type`).

The verdict:

- `VIOLATED` if any condition is false, because valid evidence then establishes a failure;
- otherwise `UNKNOWN` if any condition is undetermined;
- otherwise `SATISFIED`.

So a single trustworthy contradiction is decisive, and "I cannot tell" is never rounded up to success.

## The report

Deterministic JSON: the contract name and version, a SHA-256 of the canonical contract and of the canonical
evidence set, the verdict, each condition with its result and the records it used, each item with every record
considered and why any was discarded, and the verifier's version. Same inputs, same bytes. `openmhs outcome verify`
exits 0 for `SATISFIED`, 3 for `VIOLATED`, 4 for `UNKNOWN`, matching the CLI's existing codes for verified,
failed and uncertain.

## Trust and limits

- **Evidence is only as good as its source.** The verifier checks consistency, time, identity, quality and
  calibration validity. It cannot tell a sensor that lies from one that does not, or a simulator from a device
  if the record says otherwise.
- **A stale sensor is only caught if the contract says it must vary.** A reading repeated with a fresh timestamp passes
  rules 1 to 7. Rule 8 catches a long enough repeat for an item that opts in; it cannot catch a freeze that begins at
  the claim, a stale value that is still changing, or a sensor that legitimately holds its value. On real motion-capture
  data (RESULTS_CORESENSE.md) it was added after seeing three frozen landings, so its limit was chosen after the fact;
  the results report how many claims each limit rejects rather than one value.
- **Agreement is required, not averaged.** Two valid readings that disagree give `UNKNOWN`. A noisy sensor
  will therefore produce `UNKNOWN` more often; tolerances belong in the condition.
- **No causal reasoning yet.** `relations` are recorded, not used. The window and binding do the work.
- **No domain predicates yet.** "Inside the fixture" is expressed as numeric ranges. A geometry or image check
  would be a plug-in, later.

## What simulation can and cannot show

In a simulation the "independent" measurement comes from the same harness that injects the fault, so it agrees
or disagrees by construction. A simulated run can show that the rules are applied correctly and deterministically,
across a corpus of faults. It **cannot** show that this catches failures that matter, or that it does better than a
short script. That needs a physical device and a separate instrument: the ESP32 and bench-PSU plan. Until then,
results here are about the verifier, not about the world.

## Open questions

1. Should a controller's report be allowed to settle an `UNKNOWN_OUTCOME` command? A verdict could inform
   reconciliation, but that would put this check in the lifecycle, which v0 deliberately avoids.
2. Is "all surviving records must agree" too strict for noisy signals? The alternative, latest-wins, hides conflicts.
3. Contracts and evidence will need signing before multi-site use (RFC 0002's unsigned-seal limit applies).
4. The identity set (`attempt`, `command`, `external`) is the minimum. Sample and resource identity belong with a
   domain, not in v0.
