# Ecosystem assessment (Phase 0, revised after the experiments)

- **Date:** first written 2026-11-01 before any integration; revised the same day from what each
  experiment found. Every statement below was observed by installing and running the package,
  not read from marketing material.
- **OpenMHS baseline:** `b64532a` (main), 565 Rust tests in 89 binaries passing, clippy clean.
- **Method:** each package was installed at a pinned version into its own virtual environment on
  the same Windows 11 desktop, its simulator run natively first, then governed. Licences are the
  package metadata's.

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
- **Lifecycle** (`spec/lifecycle.md`): `DISPATCHED` is journaled before the adapter is called; a
  timeout becomes `UNKNOWN_OUTCOME` and holds the capability until reconciled; recovery never
  re-sends. `ACCEPTED` may only be followed by `APPLIED`, `FAILED`, `TIMED_OUT` or
  `UNKNOWN_OUTCOME`.
- **What OpenMHS does not offer an integration:** fault injection into a hosted adapter in a
  running gateway (only the conformance fixture can inject); preconditions that refer to an
  action's own parameters; verification of a change by an amount; a value that is a vector.

## The five projects

| | PyLabRobot | Opentrons | Micro-Manager | SiLA 2 | MADSci |
|---|---|---|---|---|---|
| Package and version used | `pylabrobot` 0.2.2 | `opentrons` 10.1.0 | `pymmcore-plus` 0.18.1 over `pymmcore` (MMCore 12.5.1); device adapters from `mmcore install` | `sila2` 0.14.0 (gitlab.com/sila2/sila_python) | `madsci.*` 0.8.1 (workcell, location, data, node module, client) |
| Licence (package metadata) | MIT | Apache-2.0 | BSD-3-Clause (pymmcore-plus), LGPL-2.1 (pymmcore) | MIT | MIT |
| Python that worked | 3.13 | **3.12 only**: 3.13 has no wheel for its `numpy<2` pin, and on 3.11 it fails at import (a pydantic model uses `typing.TypedDict`) | 3.12 | 3.12 | 3.12, and **pydantic must be pinned to 2.11.x** (2.14 breaks its serialization); `packaging` and `colorama` are used but not declared |
| What it is | Liquid handlers, plate readers, pumps, scales and more, behind one resource and tracker model | A protocol API and a simulator for the Flex (OT-2 protocols are refused) | A device-abstraction core and a Python wrapper | A standard (feature definitions over gRPC) and a Python SDK with a code generator | A laboratory-orchestration framework: nodes, workcell, workflows, locations, data |
| Simulation | A chatterbox backend per device class; it prints and models no physics | `simulate()` and `get_protocol_api()`: checks a protocol's structure, tracks no liquid | `DemoCamera` and `Utilities` test adapters, 1 MB | The user writes the server; the SDK generates its skeleton | In-memory cache and document handlers ship with it, so no Redis, MongoDB or Docker is needed |
| Native checks that matter | Tip and volume trackers refuse a pickup from an empty spot (`NoTipError`) and an aspirate beyond what a well holds (`TooLittleLiquidError`) | Labware, pipette and robot compatibility. **Not** volumes: it ran a 400 uL transfer into a 200 uL well and a negative volume | The demo stage has **no limits**: a request for 10^9 um wanders | Constraint validation on the server (`MaximalInclusive` etc.), typed defined execution errors | `ActionStatus` includes `UNKNOWN`, but the workflow engine treats it as a failed workflow |
| Stable integration point used | The `LiquidHandler` API in-process | `opentrons.simulate.get_protocol_api` and the protocol's own function | `CMMCorePlus` | The SDK's client and the Feature Definitions a server reports | A node's REST actions, and the workcell's workflow API |
| Experiment | [01](01_pylabrobot/EXPERIMENT_REPORT.md) | [02](02_opentrons/EXPERIMENT_REPORT.md) | [03](03_micromanager/EXPERIMENT_REPORT.md) | [04](04_sila2/EXPERIMENT_REPORT.md) | [05](05_madsci/EXPERIMENT_REPORT.md) |

## What an integration needs that the upstream does not give

Found by building all five:

- **Acceptance semantics.** Whether a call returned is not whether an effect is durable. Every
  adapter reports acceptance only after its simulator committed (experiment 01, finding 1).
- **A unique id per effectful request**, so a readback can tell this request's effect from an
  earlier one. The conformance suite cannot generate one (01, 02, 03).
- **Durable device state for reconciliation.** Reconciliation asks the device; if the device
  restarts empty, it answers wrongly (01, finding 4).
- **Review.** None of the five describes whether a command is safe, idempotent or how to check
  it. SiLA's description answers about half of what OpenMHS needs (04, finding 1).

## Blocking limitations found

| Project | Limitation |
|---|---|
| Opentrons | No step-level execution or inspection from outside; a run is one call. The simulator is not a safety check |
| Micro-Manager | One core per microscope, so one OpenMHS device with components; no vector values (an XY move is two writes) |
| SiLA 2 | No unitless numbers in OpenMHS; observable properties are read, not subscribed; the generic adapter has no conformance fixture, so `openmhs conformance` refuses it |
| MADSci | Its identity registry locks node names per machine; a node's `UNKNOWN` ends the workflow rather than being resolved |
| All | No physical hardware, so every `VERIFIED` is a statement about software state |

## Not inspected

- Other SDKs for SiLA 2 (Java, C#, C++) and any third-party SiLA server.
- Opentrons' robot-server HTTP API, and a real Flex or OT-2.
- Micro-Manager's acquisition engine and any real device adapter.
- MADSci's resource manager, event manager, experiment manager and Docker deployment.
- PyLabRobot's other backends and its visualizer.
