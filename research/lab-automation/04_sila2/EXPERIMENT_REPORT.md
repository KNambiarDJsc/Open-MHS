# Experiment 04: SiLA 2 instrument services through OpenMHS

## 1. Research objective

Can a standard instrument description, a SiLA 2 Feature Definition, reduce the engineering of
an adapter without losing OpenMHS's explicit authorization, side-effect and verification
semantics? Which information that OpenMHS needs is missing from the standard description?

## 2. Upstream project and version

The SiLA 2 Python SDK, `sila2` **0.14.0** (MIT,
[gitlab.com/SiLA2/sila_python](https://gitlab.com/SiLA2/sila_python)), on Python 3.12.11. It is
the Python implementation the SiLA 2 GitLab group maintains, and its PyPI metadata points
there. Other SDKs (Java, C#, C++) exist in the same group; they were not inspected.

## 3. Simulator environment

One Feature Definition written for this research,
[TemperatureController-v1_0.sila.xml](feature/TemperatureController-v1_0.sila.xml), with
originator `org.openmhs`. **It is not a SiLA standard feature.** It contains:
- properties `CurrentTemperature` (observable), `TargetTemperature` and `HeaterFaulted`;
- commands `SetTargetTemperature` (4..100 °C, with a unit constraint), `ResetHeater` and
  `ShutDown`;
- a defined execution error, `HeaterFault`.

Two servers were generated from it with the SDK's own code generator (`sila2-codegen
new-package`), and their implementations written by hand:
- **A**, [heater_a](servers/heater_a/heater_a/feature_implementations/temperaturecontroller_impl.py):
  a heating block with a first-order approach to the target;
- **B**, [heater_b](servers/heater_b/heater_b/feature_implementations/temperaturecontroller_impl.py):
  a Peltier block ramping at a fixed 4 °C/s.

**A and B are independent in code and device model, not in SDK.** Both use sila_python.
Faults are simulated **in the server** (a heater fault; applying a target and then answering
late), not in the adapter.

## 4. Hardware represented

A laboratory temperature controller (a heating block or a Peltier block). Nothing physical.

## 5. Architecture

```text
agent ── temperature_controller.set_target_temperature(target_temperature)
  ▼
OpenMHS gateway ── grants, bounds (narrowed by review), preconditions, journal, readback,
  │                unknown outcomes, audit
  ▼ adapter protocol
sila_adapter.py (generic) ── Feature Definitions read from the server; reviewed overrides
  ▼ SiLA 2 over gRPC (localhost, no TLS in this prototype)
SiLA 2 server A or B ── the SDK validates constraints and raises defined errors
```

## 6. Integration boundary

OpenMHS talks to the instrument's SiLA 2 interface and nothing else. The adapter is generic.
It has nothing specific to this instrument; everything instrument-specific is in two places:
- the server's own Feature Definition;
- a 32-line reviewed override file,
  [overrides/temperature_controller.json](overrides/temperature_controller.json).

## 7. Native baseline (A)

[native_baseline/baseline.py](native_baseline/baseline.py), the SDK's client alone, on both
servers:
- The client reads the server's features and its 3,130-byte Feature Definition.
- Setting 37 °C takes 0.7 ms on A and 1.4 ms on B.
- **SiLA itself validates**: 150 °C is refused with `ValidationError: Constraint violated:
  MaximalInclusive(100.0)`.
- An injected heater fault comes back as `DefinedExecutionError: HeaterFault` with each
  server's message, and `ResetHeater` clears it.

Unlike the Opentrons simulator, SiLA enforces its declared constraints natively.

## 8. OpenMHS implementation

[adapters/python/sila2/sila_adapter.py](../../../adapters/python/sila2/sila_adapter.py), a
294-line generic adapter, with no change to OpenMHS. It maps Feature Definitions as the
adapter README describes and writes a mapping report saying what became what and why each
other element was left out.

## 9. Commands and capabilities

| From the Feature Definition | OpenMHS capability | Where its contract comes from |
|---|---|---|
| `CurrentTemperature` (observable Real, degC) | `temperature_controller.current_temperature`, read, `Cel` | Definition. Observable, but exposed read-only (no subscription) |
| `TargetTemperature` (Real, degC) | `temperature_controller.target_temperature`, read, `Cel` | Definition |
| `HeaterFaulted` (Boolean) | `temperature_controller.heater_faulted`, read | Definition |
| `SetTargetTemperature(TargetTemperature 4..100 degC)` | `temperature_controller.set_target_temperature(target_temperature 10..60 Cel)` | Type and unit: definition. Range **narrowed by the override**. Side effect, idempotency, precondition (not faulted), readback verification, timing and the error rule: **override** |
| `ResetHeater` | `temperature_controller.reset_heater` | Override (side effect, idempotency, readback of `HeaterFaulted` = false) |
| `ShutDown` | **not exposed** | Not in the override file |
| `SiLAService` (core feature) | not exposed | Not in the override file |

**The unit was mapped by structure.** SiLA declares degC as Kelvin¹ with factor 1 and offset
273.15, and the adapter matched that to OpenMHS `Cel`. The label `degC` was not used.

## 10. Simulation and verification semantics

| Claim | Holds here? |
|---|---|
| Validated, authorized | Yes, by OpenMHS, and validated again by SiLA on the server |
| Command executed | The server returned without error |
| **`VERIFIED`** | `TargetTemperature`, read back over SiLA, equals the requested target |
| Temperature reached | **Not checked**: verification is of the setpoint, not of the block's temperature |
| Physical effect | **No** |

## 11. Test scenarios

[experiments/run_scenarios.py](experiments/run_scenarios.py) starts the chosen server, then a
gateway with the generic adapter. Its checks compare receipts with the server, read directly
with the SDK's client, outside OpenMHS. SILA-06 is the same suite run against the other
server.

## 12. Results

Six runs: three against server A ([A-1](results/run-A-1.json), [A-2](results/run-A-2.json),
[A-3](results/run-A-3.json)) and three against server B ([B-1](results/run-B-1.json),
[B-2](results/run-B-2.json), [B-3](results/run-B-3.json)). **All checks passed in all six.**

| Id | What happened |
|---|---|
| SILA-01 | The descriptor was built from the Feature Definition read over SiLA. Three properties and two reviewed commands were mapped. `ShutDown` and `SiLAService` were left out, and the mapping report says why. The unit is `Cel`. The command's range is 10..60 (the override) inside SiLA's 4..100. The report lists which fields came from the override: `side_effect`, `idempotent`, `preconditions`, `verification`, `timing`, `defined_errors_before_effect` |
| SILA-02 | A typed reading `{value, unit: Cel}` equal to the server's own value, read directly |
| SILA-03 | 37 °C `VERIFIED` by readback. 70 °C (allowed by SiLA, not by the review) and 150 °C (outside both) `DENIED`, never sent. The server still held 37 °C. Directly through the SDK, 70 °C is accepted, so the narrowing is OpenMHS's |
| SILA-04 | The server's `HeaterFault` became `REJECTED`, keeping the fully qualified identifier `org.openmhs/research/TemperatureController/v1/DefinedExecutionError/HeaterFault` and the server's own message. The next target was `DENIED` by the not-faulted precondition and not sent. `ResetHeater` was `VERIFIED` (`HeaterFaulted` read back false), then a target was `VERIFIED` |
| SILA-05 | The server applied 45 °C and answered 8 s later. After the 3 s timeout: `DISPATCHED → TIMED_OUT → UNKNOWN_OUTCOME` (`adapter.timeout`). Reconciliation read the target back: `RECONCILED_APPLIED` |
| SILA-06 | **The same adapter and the same override file, unchanged, on server B: every check passed, and the two descriptors' capabilities are identical.** Only the device name and serial differ, taken from each server's own `ServerName` and `ServerUUID` |

## 13. Performance

Release gateway, 20 set-and-verify operations per run:

| | A-1 | A-2 | A-3 | B-1 | B-2 | B-3 |
|---|---|---|---|---|---|---|
| Through OpenMHS, median (ms) | 31.4 | 31.5 | 29.9 | 30.7 | 31.0 | 33.4 |
| SDK client alone, set + read (ms) | 1.2 | 1.1 | 1.1 | 1.1 | 1.5 | 1.1 |
| Gateway start (s) | 0.90 | 0.90 | 0.94 | 1.18 | 1.00 | 1.05 |

About 30 ms per command is OpenMHS. Not measured: CPU, memory.

## 14. Reliability comparison

| | A. SDK client alone | B. Custom wrapper | C. Through OpenMHS |
|---|---|---|---|
| Out-of-range parameter | Refused by SiLA's own validation | — | Refused before any network call; a lab policy can narrow further (SILA-03) |
| Who may act | Anyone who can reach the port (TLS optional) | Would need its own | Grants, scoped tokens |
| A command not reviewed (`ShutDown`) | Callable | Would need an allow-list | Not exposed (SILA-01) |
| Native error | A typed exception | — | `REJECTED` with the SiLA identifier and message, recorded (SILA-04) |
| Lost answer | A client-side timeout; nothing records it | Would need intent records | `UNKNOWN_OUTCOME`, reconciled (SILA-05) |
| Evidence | None | Would need its own | Verifiable audit chain |

Column B was not built.

## 15. Failures and unexpected findings

1. **A Feature Definition answers about half of what OpenMHS needs.** It gives names, types,
   units, ranges and errors. It does not say:
   - whether a command is safe, reversible or idempotent;
   - how its effect could be checked;
   - which defined errors leave the instrument unchanged;
   - what a reasonable timeout is.

   Those had to come from review. Six fields per command here, and none was guessed from a
   name.
2. **Structural units work.** SiLA's unit model (SI base units, factor, offset) can be matched
   to OpenMHS's named units exactly. Unknown units are excluded, never guessed.
3. **OpenMHS cannot represent a Real without a unit**, which SiLA allows. Such properties would
   be excluded. None was in this feature, but standard features have them.
4. **Observable properties lose their observability.** SiLA streams `CurrentTemperature`; this
   adapter only reads it.
5. **The conformance suite cannot test an adapter whose faults live in the device.** It refuses
   without an adapter-side fixture, which is correct but leaves generic adapters unconformable.

   **Resolved 2026-11-02.** The adapter injects the suite's faults between itself and a real SiLA
   server, and reaches **L3** against the simulated servers ([results](../results/2026-11-02/SUMMARY.md)). The refusal had a
   point that a plain fixture would lose: the suite really sends commands, and the adapter cannot
   tell a simulated server from an instrument. So it offers a fixture only when its configuration
   says `reading_source: simulator`, and **SILA-08** shows the suite refuses otherwise and sends
   nothing. That setting also stops a simulated server's readings being reported as `device`, as
   they were in the runs above.

   The gateway restart this report left out is **SILA-07**, and it holds: `UNKNOWN_OUTCOME`
   before and after a hard restart, the same key sends nothing, and reconciliation finds the value.
   Also: reads now have threads of their own (a hung command held one of four shared threads, and
   four hung commands would have blocked the read that reconciliation needs).
6. **Validation is duplicated by design.** SiLA validates natively, so OpenMHS's bounds are a
   second check. What they add is refusing before any network traffic, and lab policy
   narrower than the instrument's own range.
7. **Verification is of the setpoint.** That the block reached the temperature would need a
   condition over time ("current within 0.5 °C of target within 60 s"), which readback with a
   timeout cannot express.

## 16. What OpenMHS contributes

- Review-based exposure: unreviewed commands are invisible.
- Policy narrower than the instrument's own range.
- Grants.
- Error translation that keeps SiLA's identifiers.
- Unknown outcomes, reconciliation and evidence.
- One adapter for any SiLA 2 server, ported unchanged (SILA-06).

## 17. What OpenMHS does not contribute

- SiLA's own validation and typed errors.
- Streaming of observable properties.
- TLS and SiLA's own authentication features (not used here).
- About 30 ms per command.

## 18. Reproduction

```sh
uv venv --python 3.12 .venv && uv pip install --python .venv "sila2[codegen]==0.14.0"
.venv/bin/python native_baseline/baseline.py heater_a 50061
export OPENMHS_BIN=$PWD/../../../target/release/openmhs
.venv/bin/python experiments/run_scenarios.py --server heater_a --out results/a.json
.venv/bin/python experiments/run_scenarios.py --server heater_b --out results/b.json
```

The servers' `generated/` code is the SDK generator's output, committed as generated.
`sila2-codegen update` regenerates it from the Feature Definition.

## 19. Open questions

- Should OpenMHS standardize a "reviewed overrides" format, shared by the WoT importer
  (OpenFlexure) and this SiLA adapter? Both needed the same kind of information.
- Should OpenMHS numbers allow a dimensionless quantity, so a unitless Real can be mapped?
- Could the conformance suite accept a device-side fault channel for generic adapters?
- A real third-party SiLA server (another SDK, or a vendor's) for a stronger SILA-06.

## 20. Recommended next experiment

MADSci (05). Two integrations, PyLabRobot and Opentrons, are reproducible (and Micro-Manager
and SiLA 2 as well), which satisfies the brief's precondition. The question there is the
boundary between an orchestrator and OpenMHS.
