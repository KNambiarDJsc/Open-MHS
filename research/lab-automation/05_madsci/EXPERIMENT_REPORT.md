# Experiment 05: OpenMHS as the governed boundary of one device in a MADSci closed loop

## 1. Research objective

MADSci orchestrates; OpenMHS governs one node's physical action. In a closed loop that
decides the next dose from the last measurement, what does OpenMHS add, who owns which part
of failure handling, and what does a wrong "lost answer" do to the science?

## 2. Upstream project and version

MADSci **0.8.1** (`madsci.workcell_manager`, `.location_manager`, `.data_manager`,
`.node_module`, `.client`; MIT) on Python 3.12.11, with PyLabRobot 0.2.2 under the liquid
handler. Installation notes, all upstream: **pydantic must be pinned to 2.11.x** (2.14 breaks
MADSci's serialization at start-up); `packaging` and `colorama` are imported but not declared;
the workcell manager cannot run a workflow without a location manager and a data manager. The
brief requires two earlier integrations to be reproducible first; four were.

## 3. Simulator environment

- MADSci's own **in-memory cache and document handlers**, so no Redis, MongoDB or Docker. The
  workcell, location and data managers run as three processes ([stack/](stack/)); nothing
  persists across a restart.
- Two MADSci nodes ([stack/nodes.py](stack/nodes.py)): `liquid_handler` (PyLabRobot's
  chatterbox simulation, governed through OpenMHS or driven natively) and `plate_reader`.
- **The science is synthetic.** The reader measures the dose actually in the well, from the
  simulator's saved state, and returns a logistic response (EC50 42 µL, Hill 1.6) plus seeded
  noise. It is a system with a known ground truth, not a chemical model.

## 4. Hardware represented

A liquid handler and a plate reader. Nothing physical.

## 5. Architecture

```text
experiment driver (deterministic bisection)
   │ one workflow per iteration: dispense(well, dose, key) → measure(well)
   ▼
MADSci workcell manager ── scheduling, node state, workflow retry; location + data managers
   │ REST actions                              │
   ▼                                           ▼
liquid_handler node ── OpenMHS client      plate_reader node ── reads the simulator's state
   │ commands.submit (idempotency key = the step's key)
   ▼
OpenMHS gateway ── grants, contract, approval, journal, unknown outcomes, audit
   ▼ adapter protocol
PyLabRobot adapter (experiment 01) ── PyLabRobot simulation, state file
```

## 6. Integration boundary

MADSci stays the orchestrator. OpenMHS governs only `liquid_handler.dispense_dose`. The
boundary is a MADSci node that **is an OpenMHS client**: an action turns into one
`commands.submit` and the OpenMHS receipt turns into a MADSci `ActionResult`:

| OpenMHS | MADSci step |
|---|---|
| `VERIFIED`, `RECONCILED_APPLIED` | `succeeded` |
| `UNKNOWN_OUTCOME`, `TIMED_OUT`, `DISPATCHED`… (outcome open) | **`unknown`**, with the command id and an instruction to reconcile |
| `DENIED`, `REJECTED`, `FAILED` | `failed`, with the policy or adapter error |
| gateway unreachable before sending | `failed`, "nothing sent" |
| submitted but no answer back | **`unknown`** |
| `AWAITING_APPROVAL` | the step waits, bounded, for the operator. When the wait runs out it **withdraws** the command, and reports `failed` only once that is confirmed (`CANCELLED`, never sent); if the withdrawal got no answer, `unknown`; if the approver won the race, the command's real outcome (finding 7) |

No second scheduler: OpenMHS never decides what runs next. **The step's idempotency key is the
step's identity**: the workflow carries `key` as an argument, so a retried step reaches the same
OpenMHS command.

## 7. Native baseline (A)

The same loop with the same PyLabRobot simulation driven in-process by the node, with no
gateway, no authorization, and no identity for a transfer ([stack/nodes.py](stack/nodes.py),
`--mode native`). A transfer is a fresh transfer each call.

## 8. OpenMHS implementation

No change to OpenMHS or the PyLabRobot adapter beyond making its wells configurable
(`dest_wells`). The node is about 90 lines of glue.

## 9. Commands and capabilities

MADSci actions: `liquid_handler.dispense_dose(well, dose_ul, key)` and
`plate_reader.measure(well)`. The OpenMHS contract is experiment 01's `liquid.transfer`
(1 to 200 µL, enumerated wells, a tip precondition, readback of the transfer id).

## 10. Simulation and verification semantics

| Layer | What it holds | What it proves |
|---|---|---|
| Planned (the optimizer) | The dose to try | Nothing |
| MADSci (workflow, step status) | Whether the step reported success | That a node answered |
| OpenMHS (receipt, audit) | That the transfer was authorized, dispatched, and its id read back | That PyLabRobot's trackers recorded it |
| Simulator (state file) | The dose in the well, and its transfer counter | What PyLabRobot's software believes |
| Measurement | The response of that dose | A synthetic function of it |

**These are five different things and the runs keep them apart.** The measurement reads the
simulator's state, not the plan, so a lost or doubled transfer changes the science. Nothing
here is physical.

## 11. Test scenarios

[experiments/run_scenarios.py](experiments/run_scenarios.py) builds a fresh stack per
scenario (an OpenMHS gateway with the adapter, two nodes, three MADSci managers) and checks
three records against each other: MADSci's steps, OpenMHS's receipts, the simulator's state.

## 12. Results

Three complete runs, **every scenario passed in all three** (27 scenario runs):
[run-1](results/run-1.json), [run-2](results/run-2.json), [run-3](results/run-3.json).

| Id | What happened |
|---|---|
| MAD-01 | Six iterations of bisection, each a two-step MADSci workflow, all completed. For every iteration the planned dose, the OpenMHS receipt (`VERIFIED`), the MADSci step and the measured dose agree. **EC50 estimate 42.35 µL, bracket [41.2, 43.5]; the truth is 42.** The ungoverned loop gives the identical estimate. Simulator: one transfer per iteration |
| MAD-02 | (a) Gateway down: the dispense step `failed` with "gateway unreachable, nothing sent"; nothing transferred. (b) Plate reader down: the dispense still happened, once, and the workflow was still `running` 15 s later with `measure` `not_started`: MADSci neither failed nor completed it |
| MAD-03 | Two experiments at once on the one liquid handler: six transfers, each in its own well, every dose as planned, no overlap in OpenMHS's dispatch windows. **Caveat (§15, finding 4)**: the dispatches were about 3 s apart, so MADSci's own scheduling had already separated them and OpenMHS's lock was not contended |
| MAD-04 | The transfer happened and its answer was lost. The step went `unknown` and the workflow `failed`. A retry from MADSci **before** reconciling returned the same command: still `unknown`, no second transfer. After `commands.reconcile` in OpenMHS (`RECONCILED_APPLIED`) a retry succeeded from the existing command and the workflow completed. The well holds **60 µL; the measurement is the response of 60 µL (0.646)** |
| MAD-04 native | The same fault without OpenMHS: MADSci was told the step failed, the retry transferred again, and the well holds **120 µL**; the plate reader reports 0.837, the response of a dose twice the one the experiment believes it ran |
| MAD-05 | The transfer is held for an operator: OpenMHS `AWAITING_APPROVAL`, nothing transferred, MADSci shows the step running. Approved within the node's 60 s wait: `VERIFIED` and the workflow completes. What happens when nobody approves in time is MAD-08 |
| MAD-06 | The gateway (and its adapter) were killed mid-step. The step went `unknown`. After restart OpenMHS held the command as `UNKNOWN_OUTCOME`, nothing re-sent. Reconciled: `RECONCILED_APPLIED`. The MADSci retry completed from the existing command; one 55 µL dose in the well |
| MAD-07 | The same experiment twice, fixed seed, two fresh stacks: identical planned doses, measured doses and responses (0.73453, 0.48412, 0.63383, 0.56575, 0.52589, 0.50658), identical estimate, identical simulator state, identical OpenMHS transition sequences per command, and both audit chains verify. The audit hashes themselves differ because they cover timestamps |

## 13. Performance

A MADSci iteration (two steps) took a median of **4.55 s with OpenMHS and 4.55 s without**
(three runs: 4.57/4.55/4.53 s governed, 4.55/4.53/4.57 s native). MADSci's polling and
scheduling dominate; OpenMHS's cost per transfer (about 35 ms in experiment 01) is far below
what this loop can resolve. Six iterations take about 27 s (6 x 4.55 s) either way; a scenario
takes about 60 s because it also starts the whole stack.

## 14. Reliability comparison

| | A. MADSci + PyLabRobot, no OpenMHS | B. Careful custom wrapper | C. Through OpenMHS |
|---|---|---|---|
| Lost answer, retried | The dose doubles; the measurement is wrong (MAD-04 native) | Would need a durable intent record and a key the retry reuses | One dose; the retry is the same command (MAD-04) |
| Gateway or process killed mid-step | The transfer's state is unknown to everyone | Would need recovery | `UNKNOWN_OUTCOME`, not re-sent, reconciled (MAD-06) |
| Approval | None | Would need one | Held, shown as running, then completes (MAD-05) |
| Concurrent experiments | MADSci's node scheduling | MADSci's | MADSci's (MAD-03); OpenMHS's lock is the backstop |
| Evidence | MADSci's workflow records | MADSci's | MADSci's plus a verifiable per-command audit chain |

Column B was not built.

## 15. Failures and unexpected findings

1. **MADSci cannot resolve an unknown outcome.** A step that returns `UNKNOWN` ends the
   workflow as failed (`workcell_engine.py`: the step's `UNKNOWN` branch sets `failed`), and a
   retry re-runs the step. Resolution has to live somewhere else. Here it lives in OpenMHS: the
   reconcile call is operator-only and reads the device, and the step's idempotency key makes the
   MADSci retry safe. Reconciliation is **not automated**: a person or a process with the
   operator credential must reconcile before the retry succeeds.
2. **The key is the glue.** Without the step's key as the OpenMHS idempotency key, MADSci's retry
   would be a new transfer. Everything in MAD-04 and MAD-06 depends on the workflow carrying it.
   Nothing enforces that a workflow author does.
3. **Partial-failure semantics differ by node-down case.** A gateway that is down fails the
   dispense fast; a plate reader that is down leaves the workflow `running` indefinitely (observed
   for 15 s, not longer). Different failures, different MADSci behaviour, and neither is OpenMHS's.
4. **MAD-03 did not test OpenMHS's lock.** The two experiments' dispatches were seconds apart.
   MADSci's node scheduler, not OpenMHS, kept them apart. What the run shows is that nothing about
   OpenMHS got in the way. The close-contention case, two clients at the same moment, is PLR-06,
   and needs no MADSci.
5. **Who owns what.** MADSci owns scheduling, the workflow and the retry. OpenMHS owns whether a
   transfer is allowed, whether it happened, and its evidence. The seam is the idempotency key and
   the `unknown` status. Resource ownership is **not** shared cleanly: both would serialize a
   device, and here MADSci did it first.
6. **MADSci's identity registry** locks a node name per machine (30 s lease): a second stack with
   the same node names was refused. It also writes `~/.madsci` unless `MADSCI_REGISTRY_PATH` is set.
7. **A flaw in this glue, found by reading it while writing this report, not exercised by any
   run.** The node waits up to 60 s for an operator. After that it returns `failed` although the
   OpenMHS command is still `AWAITING_APPROVAL`: approving it later would run a transfer MADSci has
   already recorded as failed. It should return `unknown`. More generally MADSci has no status for
   "held for a person", so a held step looks like a running one.

   **Resolved 2026-11-02; "return `unknown`" would not have been enough.** A transfer that can
   still run after its workflow has moved on is an uncontrolled transfer whatever status it is
   given. So the step now takes it off the table first: when the wait runs out it withdraws the
   command. `CANCELLED` means it can never run, and `failed` is then true. If the approver won the
   race, the step follows the command to its real outcome. If the withdrawal got no answer, the
   step is `unknown`, and the deadline the step set at submission still bounds it, so a command
   whose node has vanished still cannot start late.

   Reading the node for this found four more of the same shape, each fixed with a test:
   - a gateway lost *during* the wait raised an exception, which MADSci records as `failed`;
   - `UNVERIFIABLE` (applied, unconfirmed) was reported `failed`;
   - a definite refusal by the gateway (bad parameters, a key conflict) was reported `unknown`;
   - a retry re-chose its source well, so once the first well ran low the same key carried a
     different request and OpenMHS refused it.

   A retry now looks the key up and attaches to what exists. It makes a new attempt, linked in the
   step's record, only where the gateway's record shows nothing was sent, and a person's denial
   stands. All of it is decided in `stack/governed.py`, free of MADSci, with deterministic tests on
   a fake clock plus tests on a real gateway (`tests/test_governed.py`); **MAD-08** and **MAD-09**
   exercise it on the real stack ([results](../results/2026-11-02/SUMMARY.md)). MADSci still has no status for "held for a person".
8. **Harness bugs fixed on the way** (not findings about the systems): the gateway restarted on a
   new port the node did not follow; two stacks shared one process-wide environment variable.

## 16. What OpenMHS contributes

- A single dose in the well after a lost answer and a retry (MAD-04, MAD-06), where the ungoverned
  loop doubles it and feeds the optimizer a wrong measurement.
- Fast, clean failure when it is down, and a refusal to send what it cannot record.
- Approval as a held step.
- A verifiable record of every transfer, tied by key to the workflow step.

## 17. What OpenMHS does not contribute

- Orchestration, scheduling, optimization or the workflow.
- Resolution of an unknown outcome without a person or process holding the operator credential.
- A faster loop or a slower one: its cost is invisible next to MADSci's polling.
- Any physical truth.

## 18. Reproduction

[README.md](README.md). Three complete runs reproduced every outcome. A full run takes about
12 minutes.

## 19. Open questions

- A MADSci-native way to say "unknown, awaiting reconciliation" that pauses a workflow rather than
  failing it.
- A reconciler node: a small trusted MADSci node that holds the operator credential and
  reconciles on request. That would automate finding 1, and needs a trust discussion.
- Whether OpenMHS should be a MADSci node type, not a node that happens to call it.
- Both systems' resource models when two orchestrators share a device.

## 20. Recommended next experiment

The same loop on a real liquid handler or a vendor's own simulator, and the reconciler node
above. After that, the cross-project report's open items.
