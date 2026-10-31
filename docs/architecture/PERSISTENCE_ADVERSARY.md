# Persistence adversary: report (2026-10-31)

Step 1 of the roadmap: break the journal at every durable boundary of a command and check
that the gateway never repeats a physical action it may already have taken, and never turns
"maybe executed" into "not executed". Single gateway, simulated device, Windows 11 / NTFS,
SQLite (bundled, rusqlite 0.40) in WAL mode with `synchronous=FULL`.

Tests: [`crates/openmhs-engine/tests/persistence_adversary.rs`](../../crates/openmhs-engine/tests/persistence_adversary.rs)
(39 tests). Run: `cargo test -p openmhs-engine --test persistence_adversary`.

## Result

**One defect found and fixed.** Before the fix: 34 of 39 passed. After: 39 of 39, and the whole
workspace passes (498 tests in 81 binaries; clippy `-D warnings` clean).

The defect: when a journal write failed **after** `DISPATCHED` (recording `ACCEPTED`,
`APPLIED` or a final state), the engine returned an error at once. That released the
device lock while the adapter call was still running, left the command `DISPATCHED` or
`ACCEPTED` on record, and the unresolved hold, which only looked for `UNKNOWN_OUTCOME`, let
the **next write to the same capability reach the device**. Reproduced two ways:

- an injected I/O error at 4 boundaries (table below);
- SQLite's own `SQLITE_FULL`: with the journal's size limit swept a page at a time, 4 of 24
  sweeps ran out while a dispatched command was being recorded. After the operator raised
  the limit, without a restart, the next write was sent and `VERIFIED` while the earlier
  command's outcome was still open.

A restart was always safe: recovery already turned the stranded command into
`UNKNOWN_OUTCOME` without resending. The gap was the time between the failure and the next
restart.

The fix ([lifecycle.md §4 rule 7](../../spec/lifecycle.md)):

1. After `DISPATCHED` is durable, a failed journal write no longer ends the call. The device
   stays locked until the adapter call returns or times out.
2. The command is then settled on record if the journal still takes writes: `UNKNOWN_OUTCOME`
   if it may have reached the device ("the outcome could not be recorded; not retried"),
   `EXPIRED` if it was never sent.
3. If not even that can be written (the disk is still full), the command stays in flight on
   record, and the hold now treats `DISPATCHED`/`ACCEPTED`/`TIMED_OUT` (stops excepted) like
   `UNKNOWN_OUTCOME`: further writes on its capability and its shared resources are refused
   with `command.outcome_unresolved` until a restart's recovery settles it.

## The boundaries

Each successful command makes eight durable writes. A fault was injected at each, either
just before its commit (the transaction rolls back) or just after it (stored, but the caller
is told it failed), as either a single I/O error (`IoError`: the same gateway carries on) or
a crash (`Crash`: every later write fails, as a stopped process makes none; the file is then
reopened by a new engine on the same simulated device).

| Boundary | IoError before | IoError after | Crash before | Crash after |
|---|---|---|---|---|
| `command.created` (admission) | pass | pass | pass | pass |
| `VALIDATING` | pass | pass | pass | pass |
| `APPROVED` | pass | pass | pass | pass |
| expectation stored | pass | pass | pass | pass |
| `DISPATCHED` (intent, before the adapter call) | pass | **fail → pass** | pass | pass |
| `ACCEPTED` (device acknowledged, native reference) | **fail → pass** | **fail → pass** | pass | pass |
| `APPLIED` (device reports done) | **fail → pass** | pass | pass | pass |
| `VERIFIED` | pass | pass | pass | pass |

"fail → pass": failed before the fix, passes after.

Every case checks, on the same gateway (IoError) and after a restart (both):

- the request reached the device at most once; resubmitting it with the same idempotency
  key returns the stored receipt and sends nothing;
- no record, or a record saying "not executed" (`DENIED`, `EXPIRED`, …), only when the
  device was never called;
- while the command may have reached the device and is unsettled, a different write to the
  same capability is refused and not sent; on the same gateway it is no longer left in
  flight once the journal works again;
- recovery (run twice) sends nothing and leaves nothing in flight; `VERIFIED` only if the
  device ran it;
- an `UNKNOWN_OUTCOME` holds the capability, and reconciling it by readback gives
  `RECONCILED_APPLIED` exactly when the device ran it;
- service resumes after settling; the exported audit evidence verifies valid and complete.

## Other cases

| Case | What happened | Result |
|---|---|---|
| Crash while recovery itself settles an in-flight command (before and after the commit of its `UNKNOWN_OUTCOME`), then another start | Second recovery settles it; nothing sent | pass |
| Crash while spending an approval, which moves its command in the same transaction | Before commit: still waiting, one approval runs it once. After commit: spent, never sent, recovery expires it, it cannot be approved again | pass |
| Journal full (real `SQLITE_FULL`), limit swept over 24 page counts | Every device call has its dispatch on record; a stranded command holds its capability after the limit is raised; recovery settles it and sends nothing | fail → pass |
| **Real process death**: a child process runs the gateway and calls `abort()` (no unwinding, no orderly close) at 6 points: admission before commit, `DISPATCHED` before/after, `ACCEPTED` before/after, `APPLIED` before. Device calls are counted in a file that outlives the child | Recovered state as expected at each point (`EXPIRED` before `DISPATCHED`, otherwise `UNKNOWN_OUTCOME`); recovery sends nothing; the same key does not resend | pass |
| Journal rolled back: a consistent backup taken before the command, and the main file copied without its write-ahead log | Opened without complaint; the command is gone though the device ran it; the stale journal's own evidence verifies; **against a checkpoint kept elsewhere it fails** | known limit, pinned |

## Known limit: the gateway trusts its journal

Recovery is only as good as the journal it reads. A journal rolled back to an earlier
state (a restored backup, a lost or truncated write-ahead log, a disk or virtual machine
that acknowledged a sync it did not perform) is not detected at start, and recovery then
believes the older record. In the worst case a command that ran is missing, and nothing
holds its capability. `synchronous=FULL` makes SQLite sync the log at every commit; whether
that survives power loss depends on the storage honouring the sync.

What detects it today: comparing an export against an audit checkpoint retained
somewhere else (`openmhs audit export --checkpoint-out`, later `openmhs audit verify-file
--checkpoint`). Signed checkpoints sent to an independent store (roadmap step "signed
checkpoints") would make that routine. A gateway could also refuse to start when its
journal is behind the last checkpoint it sent out; not built.

## Not tested

- **Hard power loss** or a virtual machine reset, which is the only way to test whether the
  storage honours syncs. It needs a VM or a power-cut rig; the process-abort case covers the
  process side only.
- **Real I/O errors from the filesystem.** Injected errors are `SQLITE_IOERR` raised at the
  journal's commit points, not produced by a faulty disk or a custom SQLite VFS. Disk full
  *is* SQLite's own error.
- **Corruption**: flipped bytes in the database or log file, a read-only filesystem, a
  deleted directory, lost permissions. (Edited receipts and audit records inside a sound file
  are covered elsewhere: `journal.receipt_integrity`, `tests/evidence.rs`.) The journal does
  not run `PRAGMA quick_check` at start.
- **Plans, leases and procedures** at their own durable boundaries (a plan step's
  completion, a lease's expiry). Plan steps are ordinary commands, so the command boundaries
  above apply to each step, but the plan's own records were not faulted.
- **Out-of-process adapters**: a fault between the gateway and an adapter child process
  during the JSON exchange. Adapter crashes are covered by existing host tests, not combined
  with journal faults.
- **Linux and macOS**: run locally on Windows only; CI runs the suite on all three.

## Checklist (roadmap step 1)

- [x] Identify the durable transitions around admission, `DISPATCHED`, adapter execution,
  receipt persistence and verification: the eight above, plus approval spending and
  recovery.
- [x] Add deterministic failpoints at those transitions: `Journal::inject` (test-only,
  `#[doc(hidden)]`), before or after each commit, as an I/O error, a crash, or a real
  process abort.
- [x] Crash/restart tests proving that uncertain actions are never resent automatically.
- [x] Disk-full (real `SQLITE_FULL`) and I/O-error scenarios.
- [x] This report: passed cases, the failure found and fixed, what remains untested.
