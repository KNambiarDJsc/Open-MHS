# Ecosystem assessment (Phase 0)

- **Date:** 2026-11-01
- **OpenMHS baseline:** `b64532a` (main), 565 Rust tests in 89 binaries passing, clippy clean
- **Status:** PyLabRobot inspected in depth and integrated (experiment 01). The other four
  are **not yet inspected** beyond confirming their packages and current versions on PyPI;
  every cell marked "not inspected" is a gap, not a finding.

## OpenMHS: what an integration plugs into

Read from the code, not the docs:

- **Adapter boundary.** An adapter is a separate process the gateway starts from a permission
  manifest and talks to over stdin/stdout (`spec/adapter-protocol.md`). The Python SDK
  ([python/openmhs/adapter.py](../../python/openmhs/adapter.py)) handles the protocol; an
  adapter implements `describe`, `read` and `execute`, and returns `Applied`, `Rejected` or a
  classified `AdapterError`. Each request runs as its own task, so reads are served while an
  `execute` hangs, which is what makes reconciliation after a lost answer possible.
- **Contracts live in the descriptor** (`spec/descriptor.md`): typed values with units and
  ranges, enums, side-effect class, idempotency, preconditions on other properties,
  verification by readback of one property against a fixed value or an action parameter, and
  timeouts. The gateway enforces all of it before dispatch.
- **Lifecycle** (`spec/lifecycle.md`): `DISPATCHED` is journaled before the adapter is called;
  a timeout becomes `UNKNOWN_OUTCOME` and holds the capability until reconciled; recovery never
  re-sends. `ACCEPTED` may only be followed by `APPLIED`, `FAILED`, `TIMED_OUT` or
  `UNKNOWN_OUTCOME`.
- **What OpenMHS does not offer an integration:** fault injection into a hosted adapter in a
  running gateway (only the conformance fixture can inject); preconditions that refer to an
  action's own parameters; verification of a change by an amount (a delta).

## The five projects

| | PyLabRobot | Opentrons | Micro-Manager | SiLA 2 | MADSci |
|---|---|---|---|---|---|
| Version checked | **0.2.2** (PyPI, installed, read) | `opentrons` 10.1.0 on PyPI | `pymmcore` 12.5.1.75.0, `pymmcore-plus` 0.18.1 on PyPI | `sila2` 0.14.0 on PyPI (which SDK is maintained: not established) | `madsci.client` / `madsci.common` 0.8.1 on PyPI |
| Licence | MIT (package metadata) | not inspected | not inspected | not inspected | not inspected |
| Instrument categories | Liquid handlers, plate readers, pumps, scales, centrifuges, heater-shakers, thermocyclers, storage, sealers and more (package directories) | Not inspected | Not inspected | Not inspected | Not inspected |
| Simulation | `LiquidHandlerChatterboxBackend` (prints operations, models no physics) and chatterbox backends for most device classes; also an `opentrons_simulator` backend (not tried) | Not inspected | Not inspected | Not inspected | Not inspected |
| State and evidence | Tip and volume **trackers** (software state, with commit/rollback); full state serializable (`serialize_all_state` / `load_all_state`); native errors such as `NoTipError`, `TooLittleLiquidError` | Not inspected | Not inspected | Not inspected | Not inspected |
| Install | `pip install pylabrobot` pulls only `typing_extensions` and `websockets` | Not inspected | Not inspected | Not inspected | Not inspected |
| Integration point used | The `LiquidHandler` API in-process, inside an OpenMHS Python adapter | Proposed: govern submission of a whole validated protocol (option A), to be verified | Proposed: `pymmcore-plus` with the demo devices, to be verified | Proposed: a client of one simulated server, mapping features to a descriptor | Proposed: OpenMHS as the execution boundary of one MADSci node |
| New capability OpenMHS adds | Measured in experiment 01: authorization and contracts before dispatch, durable identity and idempotency, explicit unknown outcomes, restart-safe reconciliation, hash-chained evidence | To be measured | To be measured | To be measured | To be measured |
| Blocking limitations | Preconditions cannot depend on which well a transfer names; readback verifies an id, not a volume change | Not known | Not known | Not known | Not known |

## Notes that change the plan

- PyLabRobot ships an Opentrons backend. If experiment 02 shows the Opentrons simulator is
  hard to drive from outside, the same adapter boundary through PyLabRobot is an alternative
  to compare. This has not been tried.
- MADSci is to start only after two earlier integrations reproduce, as the brief requires.
