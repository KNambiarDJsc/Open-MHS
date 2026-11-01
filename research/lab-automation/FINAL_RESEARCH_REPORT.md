# Final research report: OpenMHS across laboratory-automation ecosystems

Date: 2026-11-01. Everything below is simulated; no physical instrument was used, and no
result says anything about a physical effect. Each claim points at an experiment report, and
each of those at raw results.

## What was done

| # | Ecosystem (version) | Boundary | Scenarios | Runs | Governed vs native, per operation | Conformance |
|---|---|---|---|---|---|---|
| 01 | PyLabRobot 0.2.2 | one composite action, `liquid.transfer` | 7 + latency | repeated, all passing | 34–36 ms vs 1.3 ms | L1 |
| 02 | Opentrons 10.1.0 (Flex simulator) | a whole reviewed protocol, pinned by SHA-256 | 7 + latency | repeated, all passing | ~131 ms vs ~88 ms | L1 |
| 03 | Micro-Manager (pymmcore-plus 0.18.1) | one device with components; positions, exposure, channel, snap | 7 + latency | 3, all passing | ~112 ms vs ~32 ms (a move and a snap) | L1 |
| 04 | SiLA 2 (`sila2` 0.14.0), two separately written servers | a generic adapter + a reviewed override file | 5 + latency, per server | 3 per server, all passing | ~31 ms vs ~1.2 ms | no fixture; the suite refuses it |
| 05 | MADSci 0.8.1 | one node of a closed loop | 9 (incl. 2 native baselines) | 3 complete, all passing | 4.55 s vs 4.55 s per iteration | n/a |

Per-experiment findings: [01](01_pylabrobot/EXPERIMENT_REPORT.md),
[02](02_opentrons/EXPERIMENT_REPORT.md), [03](03_micromanager/EXPERIMENT_REPORT.md),
[04](04_sila2/EXPERIMENT_REPORT.md), [05](05_madsci/EXPERIMENT_REPORT.md). Ecosystem notes:
[00](00_ECOSYSTEM_ASSESSMENT.md).

## The shared properties, by ecosystem

A check mark names the scenario that exercised it. A dash means **no scenario exercised it**, which
is a gap, not a pass.

| Property | PyLabRobot | Opentrons | Micro-Manager | SiLA 2 | MADSci |
|---|---|---|---|---|---|
| Unauthorized action does not dispatch | — | — | lease holder excludes another agent (MM-03) | — | — |
| Invalid parameters do not dispatch | PLR-02, 03 | OT-02 | MM-02 | SILA-03 | inherited from 01 |
| Duplicate request follows idempotency | PLR-05 | OT-05 | — | — | MAD-04 (the step's key) |
| Lost acknowledgment is not blindly repeated | PLR-04 | OT-04 | MM-05 | SILA-05 | MAD-04 |
| Uncertain outcome is explicit | PLR-04 | OT-04 | MM-05 | SILA-05 | MAD-04, 06 |
| Stale observation not presented as fresh | — | — | — | — | — |
| Concurrent access respects ownership | PLR-06 | — | MM-03 | — | MAD-03 (weak: see 05, finding 4) |
| Restart preserves safe recovery | PLR-07 | OT-07 | MM-07 | — | MAD-06 |
| Evidence verifies offline | audit chain checked in every scenario | OT-06 | MM-06 | audit chain checked | audit chain checked |
| Simulator truth is not physical truth | stated in every report | stated | stated | stated | stated |

Three rows have gaps worth closing before any claim of generality: **unauthorized access**
(covered by core tests, not re-shown per ecosystem), **stale observations** (none of the five
exercised the freshness rule), and **restart** for SiLA 2.

## The ten questions

### 1. Can OpenMHS provide reusable execution governance across several ecosystems?

**Yes, for the properties exercised and in simulation.** In all five, with no change to the
OpenMHS core, a contract refused out-of-range requests before dispatch, a lost answer became
`UNKNOWN_OUTCOME` and held the capability until reconciled, a killed gateway recovered without
re-sending, and the evidence verified offline. Adapters are 294 to 376 lines. What is *not* free:
every one needed ecosystem-specific care (below); two (Opentrons, MADSci) had install problems that
pinning had to work around; and importing numpy or gRPC after the adapter SDK starts reading its
pipe hangs on Windows, which any scientific-Python adapter can hit.

### 2. Which abstractions generalize, and which stay device-specific?

**Generalize unchanged:** the typed descriptor (values with units, ranges, enums); the lifecycle
and its unknown outcome; idempotency keys; readback verification; receipts and the audit chain;
leases; and the scenario vocabulary itself (the same fault, a lost answer, took the same shape in
five stacks).

**Stay specific:**
- *The operation boundary.* A transfer (01), a whole protocol (02), a position and a snap (03), a
  reviewed SiLA command (04). No single granularity fits.
- *What is verified.* An id was recorded (01, 02, 03), a setpoint was read back (04). Nobody could
  express "the amount moved".
- *What reconciliation reads*, and whether that state survives a restart.
- *Reviewed material.* Pinned protocol bytes (02), an override file (04). Both are the same idea
  without a shared format.
- *Fault injection*, which lives in each simulator.

### 3. What does OpenMHS solve that native frameworks do not?

Observed, each with an execution record:
- **Refusing what the simulator accepts.** The Opentrons simulator ran 400 µL into 200 µL wells
  and negative volumes; Micro-Manager's demo stage wandered towards 10^9 µm; OpenMHS refused both
  before dispatch (02, 03).
- **A record of work in flight.** None of the native stacks keeps one. After a lost answer
  the ungoverned MADSci loop doubled a dose and fed the optimizer a measurement of 120 µL as if it
  were 60 (05).
- **Resolving what MADSci cannot.** MADSci ends a workflow when a step is `UNKNOWN`; OpenMHS
  supplies the resolution and the key that makes the retry safe (05).
- **Review gating.** An unreviewed SiLA command is not exposed; an edited Opentrons protocol is
  refused (02, 04).

What native frameworks already do well and OpenMHS only duplicates: PyLabRobot's tracker refusals,
SiLA's own constraint validation, MADSci's scheduling.

### 4. How much latency and complexity does the gateway add?

**Latency:** about 30 to 40 ms per governed operation on a release build, measured end to end
including HTTP from a Python client (see the table). Against a simulator that answers in a
millisecond that is a 25- to 30-fold slowdown; against anything that takes seconds, or MADSci's
polling, it vanishes (05). Not measured: CPU, memory, or anything on a Raspberry Pi.

**Complexity:** a gateway process, a manifest, a pinned environment, and an adapter of about 300
lines that has to get several things right that are easy to get wrong (below). The adapter must be
run with `--allow-unsigned-adapters` unless signed. That is real overhead, and not justified for a
script that talks to a bench instrument it alone uses.

### 5. Can developers reproduce this without hardware?

**On the machine used, yes.** Each experiment has pinned requirements, a quickstart and a harness
that reports PASS/FAIL per scenario, and ran repeatedly. **Not shown:** reproduction by someone
else, or on Linux or macOS. Every one of the runs was on one Windows 11 desktop. Two ecosystems
need Python 3.12 specifically (Opentrons, MADSci), and MADSci needs a pydantic pin.

### 6. Can it handle failures during multi-instrument procedures?

**For the governed device, yes** (05): a lost answer, a killed gateway and a down gateway were
each handled, with one dose in the well. **Not for the whole procedure.** MADSci left a workflow
`running` when the plate reader was down, and cannot resolve an unknown outcome itself; resolution
needed a person or a process holding the operator credential. OpenMHS makes the failure safe and
visible; it does not make the procedure recover on its own.

### 7. What remains unverified until real hardware is available?

Everything physical. In particular: that any `VERIFIED` corresponds to an effect (it means a
simulator's own software state agrees); that a real device's answer to a read after a lost
acknowledgment is truthful; timing, noise and drift; a real controller's behaviour under a killed
gateway; power loss and disk errors; and anything about a person standing at the equipment.

### 8. Which upstream project offers the strongest path for collaboration?

Judged by concrete, reportable gaps found, not by any contact (none was made):
- **MADSci:** a step status that pauses a workflow for reconciliation instead of failing it (05,
  finding 1), a way to say a step is held for a person, undeclared dependencies (`packaging`) and a
  pydantic incompatibility.
- **Opentrons:** an import failure on Python 3.11 despite declaring 3.10+, and a simulator that
  accepts overfills.
- **SiLA 2:** the shape of what a Feature Definition leaves unsaid (side effects, idempotency,
  verification, which errors mean nothing happened) is a concrete input to the standard.
- **PyLabRobot:** least to offer here: its trackers already refuse the right things.

### 9. What belongs in OpenMHS core, and what in adapters or examples?

**Core, each needing a design note and a failing test before any change:**
- *Preconditions over a request's own parameters*, so "this well holds the requested volume" is
  expressible (01, finding 2).
- *Verification of an amount or by command id*, instead of a parameter echoed back (01, finding 3).
- *A way to declare a parameter unique per request*, and for the conformance suite to generate one.
  This blocked L2 in three of three adapters that need it (01, 02, 03).
- *Reconciliation of an idempotent write that a newer write has superseded*, which reports "not
  applied" for a move that happened (03, finding 2).
- *One reviewed-overrides format* shared by the WoT importer and the SiLA adapter (04).
- Documentation: report acceptance only after the device commits; import native libraries before
  the SDK starts (a hang on Windows); `{manifest_dir}` is not expanded in `filesystem`.

**Adapters and examples:** everything under `adapters/python/{pylabrobot,opentrons,micromanager,sila2}`
and the experiments.

### 10. Is the evidence strong enough to justify remote, autonomous scientific laboratories?

**No, and this work does not claim to.** What it supports is narrower: the execution-boundary
design (contract, unknown outcome, idempotency, reconcile, evidence) held up across five
simulated stacks, and the most valuable behaviour is at the failure edges, not the happy path. What
stands between this and a remote autonomous laboratory:
- no physical instrument, no physical measurement, no safety interlock;
- one gateway on one machine, a single-user trust boundary, unsigned adapters and seals;
- no network adversary, no remote-access security review;
- no independent reproduction;
- the open items in question 9, one of which (unique ids) means OpenMHS's own conformance suite
  cannot yet fully test the adapters written for it.

The next gate is a supervised physical validation of one low-risk device, and an independent
reproduction of these runs.

## What I got wrong on the way

Kept here because they shaped the results:
- Early adapters held a lock while simulating "no response", which froze reads; conformance
  caught it, and the three adapters were fixed and every scenario re-run.
- Early adapters reported acceptance before the device committed, turning clean refusals into
  `FAILED` (01).
- Several harness errors looked like findings at first (a restarted gateway on a new port, two
  stacks sharing an environment variable); they were fixed and are noted as such.
- A reading of MAD-03 as proof OpenMHS serializes was wrong: MADSci had already separated the
  work.
- A flaw in my MADSci glue (a held step returns `failed` after 60 s) was found by reading it, not by
  a test, and is documented rather than fixed after the fact.
