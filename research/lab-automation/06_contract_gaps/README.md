# 06 · Contract gaps: ten properties, every ecosystem

The [final report](../FINAL_RESEARCH_REPORT.md) left dashes in its table: properties no scenario had
exercised in a given ecosystem. These scenarios fill them, each against that ecosystem's real
adapter and simulator, through a real `openmhs serve`. **Everything is simulated; nothing here says
anything about a physical effect.**

| Scenario | Property | How it is judged |
|---|---|---|
| GAP-01 | Unauthorized requests do not dispatch | Receipt `DENIED` with a policy code, no `DISPATCHED`, simulator unchanged |
| GAP-02 | Expired and revoked credentials authorize nothing new | Refused at the API, simulator unchanged, a valid credential still works |
| GAP-03 | A stale observation is not presented as fresh | An hour-old reading comes back `usable: false`, and a command is never `VERIFIED` on it |
| GAP-04 | A repeated request follows idempotency | Same key, same command, one execution; same key on a different request refused |
| GAP-05 | A gateway restart does not repeat an uncertain command | Lost answer, hard kill, restart: still `UNKNOWN_OUTCOME`, nothing sent, reconciled against the simulator |
| GAP-06 | A lease keeps conflicting operations off the device | Another principal refused before dispatch; the holder goes; after release, so can the other |
| GAP-07 | Evidence verifies independently | `audit verify-file` in a separate process passes, and fails on a one-word edit |
| GAP-08 | A superseded write does not reconcile misleadingly | Idempotent: reconciliation refused, stays unknown. Non-idempotent: the newer command is refused, so nothing can supersede |
| GAP-09 | An approval that runs out never runs late | `EXPIRED` past its deadline, a late approval refused, nothing ran |
| GAP-10 | An adapter lost mid-command | Adapter process killed: `UNKNOWN_OUTCOME`, not sent again; the device returns without being registered again, and reconciles |

## What each result is worth

- **GAP-03 ages readings with a test hook.** `stale_adapter.py` wraps the real adapter and backdates
  its timestamps, as a cached or failed feed would. It shows what the gateway does with an old reading
  from this adapter. It does not show that any of these simulators can really produce one.
- **Execution counts are only as good as the simulator's counter.** PyLabRobot and Opentrons have one,
  read from their saved state, outside OpenMHS. Micro-Manager's stage and SiLA's set-point do not, so
  for these two, "sent once" rests on the conformance suite's delivery count (`L2.no_retry`), which is
  counted in the adapter, not on an independent reading.
- **MADSci is not a separate column.** It governs through the PyLabRobot adapter. Its own behaviour
  around approvals and restarts is in MAD-05, 06, 08 and 09.
- **No scenario has a network adversary, hardware, or two gateways.**

## Run it

```sh
OPENMHS_BIN=... [MM_DIR=...] <venv python> run_gaps.py --ecosystem pylabrobot --out results/
```

Or everything at once, from [reproduce.py](../reproduce.py). Results:
[results/2026-11-02/](../results/2026-11-02/SUMMARY.md).
