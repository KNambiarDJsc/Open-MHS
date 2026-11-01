# Experiment 02: Opentrons protocol simulation through OpenMHS

## 1. Research objective

Can OpenMHS govern the execution of a laboratory protocol, the unit a protocol-based robot
runs, as it governed a single transfer? And where should the boundary be, when the robot's
native unit is a program?

## 2. Upstream project and version

`opentrons` **10.1.0** from PyPI (Apache-2.0), on Python **3.12.11** (installed with `uv`).
OpenMHS: this branch's release build. Each run records both.

**Installation is itself a finding.** The package declares Python ≥ 3.10, but here:
- **On 3.13** its pin `numpy<2` has no wheel, and the build from source fails.
- **On 3.11** it fails at import: `opentrons/protocol_engine/commands/vacuum_module/common.py`
  uses `typing.TypedDict`, which pydantic 2.14 refuses below Python 3.12.

That is an upstream bug worth reporting.

## 3. Simulator environment

The simulator built into the package, on its **Flex** robot. 10.1.0 refuses OT-2 protocols
and points at a separate OT-2 app. The reviewed protocol
([serial_dilution.py](../../../adapters/python/opentrons/protocols/serial_dilution.py),
SHA-256 `512e3443…f8671`) uses:
- a `flex_1channel_1000` pipette;
- a Flex 1000 µL tip rack in C1;
- a NEST 12-well reservoir in C2 (stock in A1, diluent in A2);
- a NEST 96-well 200 µL plate in C3;
- a trash bin in A3.

## 4. Hardware represented

An Opentrons Flex. Nothing physical.

## 5. Architecture

```text
agent ── protocol.run(run_id, protocol=serial_dilution, n_wells, volumes, mix_reps)
  ▼
OpenMHS gateway ── grants, approval, bounds, reviewed-digest precondition, journal,
  │                idempotency, timeout → UNKNOWN_OUTCOME, readback, audit
  ▼ adapter protocol
ot_adapter.py ── reads the pinned file, checks its SHA-256, executes exactly those bytes
  ▼
opentrons.simulate.get_protocol_api("2.20", robot_type="Flex") → protocol.dilute(ctx, params)
```

## 6. Integration boundary

The brief's three options:
- **A**, governing the submission of a whole protocol, is the only one this version supports.
  It is the one used.
- **B**, governing steps, would need incremental external execution.
- **C**, wrapping a simulation API, would need state inspection and resumption.

The simulator offers neither B nor C: one call runs a whole protocol or raises.

Protocols are code, so the adapter runs **reviewed protocols only**, each pinned by SHA-256.
An agent picks one by name and supplies numbers.

How the numbers reach the protocol was forced by the upstream API. In 10.1.0:
- `simulate()` passes runtime-parameter overrides as `None`;
- `custom_data_paths` raises `NotImplementedError` for API level ≥ 2.14, and the Flex needs
  ≥ 2.15.

The documented interactive context, `get_protocol_api`, is the remaining route. The
protocol keeps a standard `run()` with defaults, so the stock `opentrons_simulate` accepts
it too.

## 7. Native baseline (A)

[native_baseline/baseline.py](native_baseline/baseline.py). With the defaults (6 wells,
50 µL transfer, 100 µL diluent, 3 mixes), `simulate.simulate()` on the file and the
interactive context each give a **78-command** log in about 90 ms.

**What the simulator checks, by experiment:**

| Parameters | Simulator |
|---|---|
| Valid | Runs, 78 commands |
| 400 µL transfers into 200 µL wells | **Runs**: no liquid tracking |
| Diluent 190 µL + transfer 150 µL (overfill) | **Runs** |
| Negative transfer (−5 µL) | **Runs**, 33 commands: those steps silently vanish |
| 14 wells in a 12-well row | **Runs**: the protocol's own slice silently uses 12 |
| 0 wells | `IndexError` from the protocol, not a simulator check |

The simulator validates the protocol's structure against the robot, labware and pipette. It
says nothing about volumes, so it cannot be the place that refuses unsafe parameters.

## 8. OpenMHS implementation

[adapters/python/opentrons/ot_adapter.py](../../../adapters/python/opentrons/ot_adapter.py),
on the existing SDK with no change to OpenMHS. It follows the PyLabRobot adapter's
conventions, and so inherits its fixes: acceptance only after the run completes, a new id
per run, a persisted run record, and an opt-in fault hook. The new pieces are the reviewed
digest and the per-run evidence file.

The shared harness ([shared/harness.py](../shared/harness.py)) is new and reused from here
on.

## 9. Commands and capabilities

| Capability | Contract |
|---|---|
| `protocol.run` | `run_id`; `protocol` ∈ reviewed names; `n_wells` 2..12; `transfer_ul` 10..100; `diluent_ul` 20..100; `mix_reps` 0..5. Irreversible, not idempotent. Precondition: `protocol.serial_dilution.reviewed` is true. Readback: `run.last_id` = `run_id`. Result: `protocol_sha256`, `commands`, `runlog_sha256` |
| `run.last_id`, `run.count`, `run.last_commands`, `protocol.*.reviewed`, `protocol.*.sha256` | Read |

The bounds keep transfer + diluent ≤ 200 µL, the well's capacity. The descriptor cannot
bound a sum of two parameters, so each is bounded on its own, which is conservative.

## 10. Simulation and verification semantics

| Claim | Holds here? |
|---|---|
| Validated, authorized, approved where required | Yes, by OpenMHS |
| Protocol identity | Yes: the receipt carries the SHA-256 of the exact bytes run |
| Simulator completed the protocol | Yes |
| **`VERIFIED`** | The simulation completed and was recorded under this `run_id` |
| Volumes, composition, contamination, pipetting precision, sample integrity | **Not checked by anything here.** The simulator does not track liquid (§7) |
| Physical effect | **No** |

## 11. Test scenarios

[experiments/run_scenarios.py](experiments/run_scenarios.py), each on a fresh gateway, checked
against the adapter's run record and the per-run command logs.

## 12. Results

Three runs on the release gateway, **all checks passed in all three**:
[run-1](results/run-1.json), [run-2](results/run-2.json), [run-3](results/run-3.json).

| Id | What happened |
|---|---|
| OT-01 | `VERIFIED`. The 78 commands are identical to the simulator run without OpenMHS. The receipt carries the protocol's SHA-256 and the SHA-256 of the command log written to disk |
| OT-02 | 400 µL, −5 µL, 14 wells, 190 µL diluent: `DENIED` (`command.value.out_of_range`), and an unreviewed protocol name `DENIED` (`command.value.not_allowed`), none dispatched; **four of these the simulator would have run**. The protocol file edited after review: `DENIED` by the precondition (`command.precondition.failed`). Restored: runs. Only the restored run reached the simulator |
| OT-03 | Under a `require_approval` grant: `AWAITING_APPROVAL`, nothing run. Approved by the operator: `VERIFIED`. A second held run cancelled: `CANCELLED`, never dispatched |
| OT-04 | The run completed and the answer was lost. After the 5 s timeout: `TIMED_OUT → UNKNOWN_OUTCOME`. The next run was `DENIED` (`command.outcome_unresolved`). Reconciliation: `RECONCILED_APPLIED`. Run once |
| OT-05 | The same idempotency key: the same command, one run. The same `run_id` under a new key: `REJECTED`, not run |
| OT-06 | `audit export` + `audit verify-file` offline: valid and complete. The receipt in the export holds the requested parameters, the protocol's SHA-256 (`512e3443…`), 92 commands and the log's SHA-256 (`e4318448…`). **Replay**: simulating the same protocol and parameters again without OpenMHS gives a log with the same SHA-256 |
| OT-07 | The gateway was killed after the run. On restart: `UNKNOWN_OUTCOME`, not re-run. Reconciled: `RECONCILED_APPLIED` |

Conformance: **L1** (39 pass, 5 fail; [conformance.json](results/conformance.json)), with
the same five failures and the same cause as experiment 01: the suite reuses one `run_id`
(§15). After the lock fix found in experiment 03, every scenario was re-run and passed
([final](results/final.json)).

## 13. Performance

Release gateway, 20 runs of the default protocol per run:

| | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| Through OpenMHS: median / p95 / max | 131 / 164 / 197 ms | 122 / 169 / 204 ms | 130 / 206 / 423 ms |
| Simulator alone: median / p95 / max | 86 / 101 / 161 ms | 87 / 105 / 179 ms | 90 / 108 / 162 ms |
| Gateway start with the adapter | 2.6 s | 2.7 s | 2.8 s |

About 40 ms a run is OpenMHS. Start-up is mostly importing opentrons (2.4 s on its own). Not
measured: CPU, memory.

## 14. Reliability comparison

| | A. Simulator alone | B. Custom wrapper | C. Through OpenMHS |
|---|---|---|---|
| Unsafe parameters | Runs them (§7) | Would need bounds | Refused before dispatch (OT-02) |
| Arbitrary code | A protocol is code; whoever submits runs anything | Would need a review gate | Reviewed digests only; an edited file is refused (OT-02) |
| Approval | None | Would need one | `require_approval` grant (OT-03) |
| Lost answer, crash | Nothing records a run in flight | Would need intent records | `UNKNOWN_OUTCOME`, reconciled, never re-run (OT-04, OT-07) |
| Duplicate submission | Runs twice | Would need idempotency | Once per key (OT-05) |
| Evidence | A run log, if kept | Would need its own | Digest-bound receipts in a verifiable audit chain, replayable (OT-06) |

Column B was not built (as in experiment 01).

## 15. Failures and unexpected findings

1. **Importing opentrons inside a running Python adapter never returns on Windows.** A
   faulthandler dump showed the hang in numpy's core import, while the SDK's stdin reader
   thread was blocked on the pipe. It was reproduced with a minimal adapter whose
   `initialize` only imports `opentrons.simulate`, and fixed by importing at module level,
   before `run()`.
   The mechanism in the interpreter was not established. The SDK should document this, or
   read stdin differently. This would bite any adapter built on numpy: Micro-Manager, SiLA
   clients, most of scientific Python.
2. **The simulator is not a safety check** (§7): it ran overfills and negative volumes. In
   this stack, the parameter bounds exist only in OpenMHS's contract.
3. **The conformance gap of experiment 01 recurs, identically** (L1, the same five checks). Two
   independent adapters needed a new id per request to make readback meaningful, and the
   suite cannot generate one. This is now a property of OpenMHS, not of one adapter.
4. **No cross-parameter bounds.** transfer + diluent ≤ capacity is not expressible, so the
   bounds are stricter than needed.
5. **`{manifest_dir}` is substituted in `runtime` but not in `filesystem`** (found by
   `openmhs conformance`); relative paths work, and the manifest uses them.
6. **Opentrons writes `~/.opentrons`** unless `OT_API_CONFIG_DIR` is set; the harness sets it.

## 16. What OpenMHS contributes

All with execution records above:
- parameter bounds the simulator lacks;
- review pinning of protocol code;
- approval;
- explicit unknown outcomes and crash-safe reconciliation;
- idempotency;
- digest-bound, offline-verifiable evidence that replays.

## 17. What OpenMHS does not contribute

- Protocol semantics, labware and pipette checks: those are Opentrons'.
- Any liquid truth.
- The ability to govern individual steps, because the simulator runs whole protocols.
- It adds about 40 ms per run and a pinned-digest workflow: any protocol change needs a new
  review.

## 18. Reproduction

[README.md](README.md). Three runs reproduced every outcome.

## 19. Open questions

- Does a real Flex (or the Opentrons HTTP API on a robot) allow step-level governance
  (option B)? That needs a robot or the robot-server's simulator.
- Should OpenMHS carry a "reviewed artifact" concept, a digest-pinned program, instead of
  each adapter building one?
- Should the descriptor allow cross-parameter constraints?

## 20. Recommended next experiment

Micro-Manager (03): many devices behind one core, which tests whether the abstractions so
far, built around one composite action, generalize to a stage, a camera and a shutter.
