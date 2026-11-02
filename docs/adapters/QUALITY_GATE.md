# Adapter quality gate

What an adapter must provide before it is accepted, and how each item is checked. It reuses
what already exists: the descriptor and manifest validators, `openmhs conformance`, the Python SDK
and the registry. There is no second framework.

**Passing this gate is not a safety certification.** It says an integration behaves as it declares
in front of a test fixture. It says nothing about whether a device is safe to operate.

## The requirements

| # | Requirement | Provided by OpenMHS | The adapter must |
|---|---|---|---|
| 1 | **Versioned manifest** | Validated at load ([permission-manifest.md](../../spec/permission-manifest.md)) | Ship `manifest.json` naming its id, version and the narrowest capabilities, endpoints and paths it needs |
| 2 | **Clear capability descriptions** | `L0.descriptor` validates the shape; not the prose | Describe what each capability does, in terms of the device. Say what a simulated one is not |
| 3 | **Typed inputs and outputs** | Descriptor schema; `L1.read.value`; requests validated before dispatch | Declare a type for every value and parameter. Nothing free-form where the device has a type |
| 4 | **Units and constraints** | Units normalized to the declared unit before the adapter sees them | Declare the unit, range and enum the device truly has. Narrow rather than copy a wide native range |
| 5 | **Approval and authorization** | Grants, scopes, leases, approvals, in the gateway | Declare `side_effect` honestly: it decides what a grant's `max_side_effect` allows. Never reimplement authorization |
| 6 | **Idempotency semantics** | Idempotency keys, the unresolved hold | Set `idempotent` from what the device does, not its name. Nothing may be inferred from a name |
| 7 | **Explicit uncertainty and timeouts** | `L2.timeout`, `L2.lost_ack`, `L2.malformed`, `L2.no_retry` | Return `Rejected` only when nothing was applied; raise `not_sent` only if nothing reached the device; **call `progress.accepted()` only after the device has committed**; never retry on its own |
| 8 | **Reconciliation and its limits** | `commands.reconcile`, supersession records ([lifecycle.md](../../spec/lifecycle.md) §5) | Declare a readback where one exists; `verification: none`, with a reason, where it does not. Say what a readback does *not* show. State what survives an adapter restart |
| 9 | **Audit evidence** | The hash-chained audit, receipts, `audit verify-file` | Pass the device's own reference to `progress.accepted(external_ref)` when it has one. Never log credentials |
| 10 | **Simulator-based testing** | `openmhs conformance --adapter-manifest`, fixture mode | Offer a fixture (`fixture_faults`, deliveries counted). Report `source: "simulator"` when it is one, and never `"device"` |
| 11 | **Reproducible quickstart** | | A README that takes someone with no hardware to a passing run, with versions pinned |
| 12 | **Compatibility and conformance report** | `--report-dir`, [registry](../../registry/), `tools/check_registry.py`, `tools/gen_matrix.py` | Commit the report the suite produced, with its environment. State what it was run on |

## Required level

| Adapter kind | Required |
|---|---|
| Read-only | **L1** |
| Anything that writes or invokes | **L2**, and **L3** unless the report explains why a capability cannot take part in leases or plans |

A level is reached only with no failed and no untested checks (`spec/conformance.md` §2). An
adapter must not be changed to get past the suite: if the suite is wrong, change the suite, in the
open, as was done for read-back ids ([RFC 0003](../rfcs/0003-reconciliation-and-verification.md) §2).

## Python adapters: the mistakes this gate exists to catch

Each of these was found in a real adapter in this repository. The first five are not fully
checkable by the suite, so reviewers look for them.

- **Accepting too early.** `progress.accepted()` before the device commits turns a clean refusal into
  `FAILED`, possibly partial.
- **A lock held across a hang.** A simulated or real hang inside the adapter's lock freezes the
  reads that reconciliation needs. Hang outside the lock.
- **Blocking the event loop.** Use `asyncio.to_thread`, and keep reads on threads hung commands
  cannot use up (`adapters/python/sila2/sila_adapter.py`).
- **Checking a path by string prefix.** `/data/lab` also matches `/data/lab-other`. Use
  `permissions.check_read` and `permissions.check_write`.
- **Claiming the wrong source, or testing the wrong thing.** A generic client cannot tell a simulated
  server from an instrument: make it a setting that defaults to `"device"`, and offer no conformance
  fixture unless it says `"simulator"`, or the suite's real commands could reach an instrument.
- **A reused id verifying against an earlier effect.** Where verification reads an id back, refuse a
  reused one. The suite sends new ones.

Handled by the SDK, so an adapter no longer needs to work around them: importing a native extension
after start-up on Windows, and a blocking device call keeping the process alive after its gateway
has gone.

## Checking an adapter

```sh
python research/lab-automation/shared/run_conformance.py --ecosystem <name> --report-dir out/   # lab adapters
openmhs conformance --adapter-manifest adapters/python/<name>/manifest.json --report-dir out/   # others
python tools/check_registry.py
```

## Where the lab adapters stand

Levels are from the suite, against simulators, on one Windows desktop unless the report says
otherwise. See [research/lab-automation/results/](../../research/lab-automation/results/).

| Requirement | PyLabRobot | Opentrons | Micro-Manager | SiLA 2 |
|---|---|---|---|---|
| Manifest, typed capabilities, units | yes | yes | yes | yes, from Feature Definitions plus a reviewed override |
| Fixture, deliveries counted | yes | yes | yes | yes; faults injected in the adapter, against a real SiLA server |
| Verification | an id recorded, not the volume | an id recorded, not the run's effect | position read back; image id recorded | set-point read back |
| Survives an adapter restart | saved simulator state | saved simulator state | saved simulator state | the server keeps its own state |
| Signed | no: run with `--allow-unsigned-adapters` | no | no | no |

Signing is the one requirement none of them meets. It is available
([trust](../architecture/adapter-host.md)) and should be done before any of these is listed in the
registry as anything more than research.
