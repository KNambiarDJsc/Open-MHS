# Experiment 01: PyLabRobot through OpenMHS

## 1. Research objective

Can OpenMHS govern a liquid-handling operation that PyLabRobot performs, adding
authorization, contracts, durable command identity, explicit uncertain outcomes,
restart-safe reconciliation and evidence, without taking over PyLabRobot's own
abstractions? And what does that cost?

## 2. Upstream project and version

PyLabRobot **0.2.2** from PyPI (MIT), Python 3.13.4, Windows 11. OpenMHS at `b64532a` plus
this branch's adapter (`research/lab-automation`). Every run records both in its results
file.

## 3. Simulator environment

PyLabRobot's `LiquidHandlerChatterboxBackend` with one channel. It prints each operation and
**models no physics**. Deck: a Hamilton STARlet deck (`STARLetDeck`), a
`TIP_CAR_480_A00` carrier holding a `hamilton_96_tiprack_300uL` at rail 1, a
`PLT_CAR_L5AC_A00` carrier holding two `cor_96_wellplate_360uL_Fb` plates (`source`,
`dest`) at rail 10. Tip and volume tracking are on.

## 4. Hardware represented

A single-channel liquid handler. Nothing physical: no instrument was connected.

## 5. Architecture

```text
agent / CLI / Python client
        │  commands.submit (liquid.transfer)
        ▼
OpenMHS gateway ── authorization, descriptor contract, preconditions,
        │          journal (DISPATCHED before the call), idempotency, timeout,
        │          readback verification, reconciliation, hash-chained audit
        │  adapter protocol over stdin/stdout
        ▼
plr_adapter.py (out-of-process) ── one composite operation, snapshot/rollback,
        │                           simulator state file, opt-in fault hook
        ▼
PyLabRobot LiquidHandler ── deck, labware, tip and volume trackers, native errors
        │
        ▼
LiquidHandlerChatterboxBackend ── prints; no physics
```

## 6. Integration boundary

One action, `liquid.transfer` = fresh tip, aspirate, dispense, discard tip. It is the
narrowest boundary that is still a whole operation, and no pipette movement is exposed on
its own. An agent cannot aspirate without dispensing, or keep a tip between transfers. The
wells an agent may name are enumerated in the descriptor. PyLabRobot remains responsible
for labware geometry, tip handling and liquid bookkeeping; the adapter does not reimplement
any of it.

## 7. Native baseline (A)

[native_baseline/baseline.py](native_baseline/baseline.py): PyLabRobot alone, one 50 uL
transfer. Output: source A1 200 → 150 uL, destination B1 0 → 50 uL, the tip picked up and
returned. **Which state is reported by what:** the backend reports nothing but its printed
log; every volume and tip state is held by PyLabRobot's trackers, which also raise the
native refusals (`NoTipError`, `TooLittleLiquidError`). There is no instrument state
independent of the trackers in this simulation.

## 8. OpenMHS implementation

[adapters/python/pylabrobot/plr_adapter.py](../../../adapters/python/pylabrobot/plr_adapter.py)
(337 lines including the descriptor), on the existing Python adapter SDK with no change
to OpenMHS. Choices:

- **Rollback on refusal.** A transfer spans several PyLabRobot calls. If PyLabRobot refuses
  part-way (say, after the tip pickup), the adapter restores the whole handler, head
  included, from a snapshot taken just before. So `REJECTED` means nothing was applied.
- **Acceptance is reported only after the transfer completed** (finding 1).
- **`transfer_id` must be new.** The readback compares `transfer.last_id` with the request's
  `transfer_id`. A reused id would match an earlier transfer, so the adapter refuses it.
- **Simulator state file** (opt-in): the trackers are saved after every completed transfer
  and reloaded at start. Without it, a restart starts a fresh deck.
- **Fault hook** (opt-in, simulation only): a file the harness writes makes the next
  transfer apply and never answer, or hang before applying. OpenMHS can inject faults into
  a hosted adapter only in conformance mode, not in a running gateway.
- **The SDK's conformance fixture** (`link_down`, `no_response`, `lost_ack`, `garbled`,
  `refuse`), reachable only from `openmhs conformance`, so OpenMHS's own suite can test it.

## 9. Commands and capabilities

| Capability | Contract |
|---|---|
| `liquid.transfer` (invoke) | `transfer_id` string ≤ 64; `source` ∈ {A1, A2, A3}; `dest` ∈ {B1, B2, B3}; `volume` 1 to 200 uL. `irreversible`, `idempotent: false`, timeout 3 s. Precondition `tips.available > 0` (fresh within 2 s). Verification: readback of `transfer.last_id` = `transfer_id` |
| `tips.available`, `transfer.last_id`, `transfer.count`, `source.a1..a3.volume`, `dest.b1..b3.volume` (read) | Tracker state, `source: simulator`, fresh for 2 s |

## 10. Simulation and verification semantics

| Claim | Holds here? |
|---|---|
| Request validated, authorized | Yes, by OpenMHS before dispatch |
| Command dispatched | Yes, journaled before the adapter call |
| Simulator accepted / reported completion | Yes, PyLabRobot returned without error |
| Simulator state changed | Yes, the trackers changed, and the state file records it |
| **`VERIFIED`** | Means: PyLabRobot's trackers recorded this `transfer_id` as the last transfer. It does not check the volumes |
| Independent measurement confirmed the effect | **No.** There is no measurement; the harness compares receipts with the simulator's own state file, which is the same software state |
| Physical effect verified | **No** |

## 11. Test scenarios

All in [experiments/run_scenarios.py](experiments/run_scenarios.py). Each starts a real
gateway in a fresh home with the adapter as its child process and checks receipts against
the simulator's state file, whose `count` is the simulator's own execution counter.

## 12. Results

Three consecutive runs, **all checks passed in all three**:
[run-1](results/run-1.json), [run-2](results/run-2.json), [run-3](results/run-3.json)
(debug gateway). After the conformance fixture was added to the adapter (§15, finding 7), a
fourth run on the release gateway passed every check too ([final](results/final.json)).

| Id | What happened (run 1; identical outcomes in runs 2 and 3) |
|---|---|
| PLR-01 | `VERIFIED`. Simulator count +1; trackers: A1 150 uL, B1 50 uL, one tip fewer; the gateway's reads equal the simulator's state |
| PLR-02 | 500 uL and 0.5 uL: `DENIED` (`command.value.out_of_range`); well C7, and a destination named as source: `DENIED` (`command.value.not_allowed`). None dispatched; simulator state unchanged |
| PLR-03 | With the rack empty: `DENIED` by OpenMHS's precondition (`command.precondition.failed`), never dispatched. Without OpenMHS, PyLabRobot itself refuses the same pickup with `NoTipError: Tip spot does not have a tip.` So OpenMHS refuses first, and PyLabRobot would have refused anyway |
| PLR-04 | The simulator applied the transfer and never answered. After the declared 3 s timeout the receipt went `ACCEPTED → TIMED_OUT → UNKNOWN_OUTCOME`. The next transfer was `DENIED` (`command.outcome_unresolved`) and not sent. Reconciliation read the simulator and gave `RECONCILED_APPLIED`; transfers then worked again. The simulator executed the lost transfer exactly once |
| PLR-05 | The same idempotency key returned the same command, executed once. The same `transfer_id` under a new key: `REJECTED` by the adapter, nothing applied. Without a key, two submissions are two transfers, as documented |
| PLR-06 | Two agents, 80 uL each, from a well holding 100 uL, at the same moment. The gateway serialized them on the device (dispatches 27 ms apart). One was `VERIFIED`; the other `REJECTED` with PyLabRobot's `TooLittleLiquidError: Not enough liquid in container: 80.0uL > 20.0uL`. The well ended at 20 uL; one transfer executed |
| PLR-07 | The gateway (and its adapter) were killed mid-transfer, once after the effect and once before it, then restarted. Both came back `UNKNOWN_OUTCOME` and neither was re-sent. Reconciliation gave `RECONCILED_APPLIED` and `RECONCILED_NOT_APPLIED` respectively. The simulator count moved +1 and +0. The audit chain verified across both crashes |

## 13. Performance

Release build of `openmhs` from this branch, same desktop as docs/benchmarks.md (i7-10700,
Windows 11), warm. 30 transfers of 1 uL per run, three runs
([release-1](results/release-1.json), [-2](results/release-2.json), [-3](results/release-3.json)).
The client is the Python SDK over HTTP on localhost, so client and HTTP cost are included.

| | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| Through OpenMHS (C): median / p95 / max | 34.0 / 49.3 / 61.2 ms | 34.4 / 48.9 / 49.5 ms | 35.6 / 50.5 / 50.8 ms |
| PyLabRobot alone, in-process (A): median / p95 / max | 1.24 / 1.54 / 1.71 ms | 1.30 / 1.55 / 1.61 ms | 1.29 / 1.59 / 1.71 ms |
| Gateway start with the adapter | 684 ms | 681 ms | 735 ms |
| Lost answer: wait until `UNKNOWN_OUTCOME` (PLR-04) | 3,020 ms | 3,026 ms | 3,023 ms |
| Reconciliation (PLR-04) | 3 ms | 2 ms | 3 ms |
| Restart after a kill (PLR-07) | 680 / 731 ms | 732 / 681 ms | 682 / 683 ms |

- **About 33 ms per transfer is OpenMHS:** HTTP, authorization, a synced journal write for
  each of the six lifecycle transitions, the adapter's process boundary, the precondition
  read and the readback. The debug build of the gateway gave 40 to 48 ms (runs 1 to 3 in
  §12). This is a governed transfer against an in-memory simulator: a real liquid handler
  takes seconds per transfer, but that was not measured.
- **The lost-answer wait is the timeout the descriptor declares** (3 s), not overhead. A
  real transfer would declare a longer one, and wait that long before it is uncertain.
- **Journal:** 278,528 bytes after the PERF scenario: 218 audit records in all, about seven
  per transfer.
- **Not measured:** gateway CPU, peak memory, and the simulator-side overhead split out.

## 14. Reliability comparison

| | A. PyLabRobot alone | B. With a careful custom wrapper | C. Through OpenMHS |
|---|---|---|---|
| Out-of-contract request | Refused only if PyLabRobot's trackers see a problem (more than the well holds, PLR-06; no tip, PLR-03); it has no notion of which wells or volumes this workcell permits | Would need its own validation | Refused before dispatch (PLR-02) |
| Who may act | Anyone holding the Python object | Would need its own authorization | Grants and scoped tokens |
| Lost answer | Not applicable in-process; across a process boundary nothing records that a transfer was in flight | Would need a durable intent record and reconciliation | `UNKNOWN_OUTCOME`, the capability held, reconciled from the device state (PLR-04) |
| Duplicate request | Executes twice | Would need idempotency storage | Once per idempotency key (PLR-05) |
| Crash mid-operation | Tracker state is in memory and lost unless the script saves it (`save_state_to_file`); nothing records that an operation was in flight | Would need persistence and recovery | Journaled; recovered as `UNKNOWN_OUTCOME`, never re-sent (PLR-07) |
| Evidence | Printed log | Would need its own log | Hash-chained audit, verifiable offline |

**Column B was not built.** Its cells say what such a wrapper would have to do; nothing in
this experiment measures one. Building it is the honest next comparison: it may well get
most of the way with less machinery.

## 15. Failures and unexpected findings

1. **Accepting too early turns a clean refusal into a "maybe partial" failure.** The first
   version of the adapter reported `accepted` before calling PyLabRobot. When PyLabRobot
   then refused (PLR-06), the gateway recorded `FAILED`, because the lifecycle does not
   allow `ACCEPTED → REJECTED`. That was correct of the gateway: after acceptance a failure
   may be partial. The rollback had in fact restored everything, so the adapter now reports
   acceptance only after the operation completes. Any adapter author can make this mistake,
   and the SDK documentation should say so.
2. **Preconditions cannot depend on the request's own parameters.** OpenMHS can require "a
   tip is available" but not "the source well this request names holds at least the
   requested volume". In PLR-06 the over-subscription was caught by PyLabRobot's tracker, not
   by OpenMHS. Candidate for the core: parameter-bound preconditions. Not changed here; it
   needs a design note and a failing test first.
3. **Verification is of an id, not of an amount.** A readback compares one property with a
   fixed value or one parameter, so "the destination gained `volume` uL" is not expressible.
   `transfer.last_id` is the strongest check the descriptor allows. It proves that the
   simulator recorded this transfer, nothing more.
4. **Reconciliation is only as good as the device's own durable state.** PLR-07 works because
   the adapter saves the trackers. Without the state file, a restarted adapter starts a fresh
   deck, and reconciliation of an applied transfer would read `RECONCILED_NOT_APPLIED`: a
   wrong answer, and OpenMHS cannot detect it.
5. **No fault injection into a hosted adapter in a running gateway.** The lost-answer and
   crash scenarios need an opt-in hook in the simulator adapter itself.
6. **The adapter is unsigned**, so the gateway must run with `--allow-unsigned-adapters`.
7. **The conformance suite and a "new id per request" parameter do not fit together.** The
   adapter reaches **L1** (59 checks pass, 5 fail; [conformance.json](results/conformance.json)).
   All five failures (`L2.malformed`, `L2.lost_ack`, `L2.recovery`, `L3.leases`, `L3.plan`)
   have one cause. (A sixth, `L2.timeout`, was the adapter's simulated hang holding a lock;
   found in experiment 03, fixed, and every scenario re-run: [final](results/final.json).) The suite sends every test command with the same
   generated `transfer_id` (`"openmhs-conformance"`), and the adapter refuses a reused id
   before the injected fault can act. The adapter's rule is needed (finding 3: a reused id
   would verify an earlier transfer), and the descriptor cannot declare that a parameter must
   be unique per request. So the suite cannot generate a valid second request. Before the
   fixture existed the suite refused to run at all ("offers no conformance fixture"), which is
   the right refusal. The adapter was not changed to get past the suite; L2 behaviour (lost
   answer, timeout, recovery) is shown instead by PLR-04 and PLR-07 through a real gateway.
   Fix options for the core: a way to declare a parameter unique (and have the suite generate
   one), or verification bound to the command id rather than a parameter.

## 16. What OpenMHS contributes

All shown by an execution record above:
- typed contracts enforced before dispatch (PLR-02, PLR-03);
- serialization of competing agents on one device (PLR-06);
- an explicit `UNKNOWN_OUTCOME` that holds the capability until the outcome is known
  (PLR-04);
- recovery after a crash that never re-sends (PLR-07);
- idempotency (PLR-05);
- per-agent authorization;
- a verifiable audit chain across crashes.

## 17. What OpenMHS does not contribute

- Any physical truth: everything is PyLabRobot's software state.
- Liquid bookkeeping or labware knowledge: that is all PyLabRobot.
- A per-well volume check: PyLabRobot did it (finding 2).
- Verification of the amount moved (finding 3).
- It also adds a process boundary, a gateway to run and an unsigned-adapter flag, plus the
  latency in §13.

## 18. Reproduction

See [README.md](README.md). Three runs reproduced every outcome.

## 19. Open questions

- Would a careful custom wrapper (column B) give the same guarantees with less machinery?
- Should parameter-bound preconditions and delta verification enter the descriptor, or is
  the composite-operation-with-adapter-checks pattern enough?
- Should the SDK warn when `accepted()` is called before the device has committed?
- Unique-per-request parameters, or verification bound to the command id (finding 7)?
- How does this behave on PyLabRobot's other backends (its Opentrons simulator), which keep
  state outside the trackers?

## 20. Recommended next experiment

Build column B for this same transfer first, so the comparison is not against a strawman.
Then move to Opentrons (experiment 02), whose simulator validates whole protocols. There the
boundary question is different: govern the submission of a protocol, not one step.
