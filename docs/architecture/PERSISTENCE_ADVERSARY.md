# Persistence adversary: report (2026-10-31)

Step 1 of the roadmap: break the journal at every durable boundary and check that the
gateway never repeats a physical action it may already have taken, and never turns "maybe
executed" into "not executed". Single gateway, simulated devices (plus one real Python
adapter process), Windows 11 / NTFS locally and Linux, Windows and macOS in CI; SQLite
(bundled, rusqlite 0.40) in WAL mode with `synchronous=FULL`.

| Tests | What they break |
|---|---|
| [`persistence_adversary.rs`](../../crates/openmhs-engine/tests/persistence_adversary.rs) (39) | Every command write, before and after its commit, as an I/O error or a crash; recovery itself; approval spending; real `SQLITE_FULL`; a real process `abort()`; journal rollback without protection |
| [`persistence_storage.rs`](../../crates/openmhs-engine/tests/persistence_storage.rs) (6) | Power lost mid-write (torn log), a bit flipped at 706 places in the file, a read-only journal, an edited readback target, an edited audit record, rollback caught by a checkpoint file |
| [`persistence_plans.rs`](../../crates/openmhs-engine/tests/persistence_plans.rs) (2) | Every plan and lease write (48 cases); crashes while taking or releasing a lease |
| [`openmhs-host/tests/process.rs`](../../crates/openmhs-host/tests/process.rs) (1 new) | A journal fault while recording a real child process's acknowledgement, then a restart |

Run: `cargo test -p openmhs-engine --test persistence_adversary --test persistence_storage --test persistence_plans`.

## Result

**Four defects found and fixed; one limit closed.** All 48 drills pass, and so does the whole
workspace (507 tests in 83 binaries; clippy `-D warnings` clean).

| # | Found by | Defect | Fix |
|---|---|---|---|
| 1 | Command-boundary I/O errors (4 cases) and real `SQLITE_FULL` | A journal write that failed after `DISPATCHED` ended the call at once: the device lock was released while the adapter still ran, the command stayed `DISPATCHED`/`ACCEPTED` on record, and the **next write to the same capability was sent** although the first one's outcome was open | The device stays locked until the adapter call ends; the command is settled as `UNKNOWN_OUTCOME`; one that cannot be settled is held like an unknown outcome until recovery ([lifecycle.md §4 rule 7](../../spec/lifecycle.md)) |
| 2 | Bit flips (9 cases) | The receipt lookup by idempotency key (the Python SDK's lost-response recovery, `commands.get_by_key`, and plan recovery) **returned a damaged receipt without the integrity check** every other read runs: one flip changed the capability name, unnoticed | Checked like every other read (`journal.receipt_integrity`) |
| 3 | Bit flips (1 case) | A flip in the idempotency index made the lookup **find no command where one exists**. SQLite's `quick_check` does not compare indexes with tables, so the gateway started; a resubmission under the same key would have created and sent a second command | At start, the full `integrity_check` of the `commands` and `approvals` tables and their unique indexes (`journal.corrupt`) |
| 4 | Reading the code for the bit-flip drill | The readback target a command is verified and reconciled against was stored outside any digest: **an edited target would be used**, and could turn "applied" into "not applied", which invites a resend | Its SHA-256 is written into the command's `DISPATCHED` audit record and checked on every read; a mismatch is refused |
| — | (limit, not a defect) | A journal rolled back to an older copy is believed at start | `serve --checkpoint-file`: the audit head is written outside the journal after every dispatch, before the device is called; a journal behind it is refused (`journal.behind_checkpoint`) |

Also new: the gateway refuses to start on a journal whose audit chain does not verify
(`journal.audit_invalid`), rather than extending a broken chain; and a command whose
dispatch was recorded but whose adapter was never called ends `FAILED` with
`adapter.not_sent`, not `UNKNOWN_OUTCOME`.

## Command boundaries

Each command makes eight durable writes. A fault was injected at each, just before its
commit (the transaction rolls back) or just after it (stored, but the caller is told it
failed), as a single I/O error (`IoError`: the same gateway carries on) or as a crash
(`Crash`: every later write fails, as a stopped process makes none; the file is then reopened
by a new engine on the same simulated device).

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

"fail → pass": failed before the fixes, passes after. Each case checks, on the same gateway
and after a restart:

- the request reached the device at most once, and resubmitting it under the same
  idempotency key returns the stored receipt and sends nothing;
- no record, or a record saying "not executed", only when the device was never called;
- while the command may have reached the device and is unsettled, a different write to its
  capability is refused and not sent; once the journal works again it is not left in flight;
- recovery (run twice) sends nothing and leaves nothing in flight; `VERIFIED` only if the
  device ran it;
- an `UNKNOWN_OUTCOME` holds the capability, and reconciling it by readback gives
  `RECONCILED_APPLIED` exactly when the device ran it;
- service resumes once settled; the exported audit evidence verifies valid and complete.

## Process, recovery, approvals, disk full

| Case | Result |
|---|---|
| **Real process death**: a child process runs the gateway and calls `abort()` (no unwinding, no orderly close) at 6 points from admission to `APPLIED`; device calls are counted in a file that outlives it | Recovered as expected at each point (`EXPIRED` before `DISPATCHED`, `UNKNOWN_OUTCOME` after); nothing resent |
| Crash while recovery itself settles an in-flight command (before and after its commit), then another start | Settled by the second start; nothing sent |
| Crash while spending an approval (one transaction with its command) | Before commit: still waiting, one approval runs it once. After: spent, never sent, expired at recovery, cannot be approved again |
| Real `SQLITE_FULL`: the journal limit swept over 64 page counts, with records of varying size so the journal runs out at different writes | Every device call has its dispatch on record. Where a dispatched command was stranded (1 sweep after the fixes; 4 of 24 before, all of which sent the next write), it holds its capability after the limit is raised and is settled by recovery |
| **Out-of-process adapter** (the Python test adapter as a real child process): journal fault while recording its acknowledgement with a native reference, once and as a crash, before and after the commit, then a restart on the same running process | Not cut off mid-exchange; `UNKNOWN_OUTCOME`, with the reference kept exactly when its record committed; delivered once; held until a person resolves it |

## The files themselves

| Case | Result |
|---|---|
| **Power lost mid-write** (storage that honours its syncs): after each of the 8 commits of a command, the files were copied, and the write-ahead log of the next commit cut at 5 points, from its first byte to one short of complete (40 torn writes) | SQLite came back at the last complete commit every time; recovery was safe from each, and a command whose device call had happened was never recorded as not executed |
| **A bit flipped** at 706 places spread over the whole database file (two commands: one verified, one unknown) | 80 refused at start, 25 caught when read, 601 without effect (free space, padding), **0 silently used** after fixes 2 and 3; never resent |
| Journal file made read-only | Refused at start, or every command refused; nothing sent |
| Readback target edited in the database | Reconciliation refused (`journal.receipt_integrity`); the command stays `UNKNOWN_OUTCOME` |
| Audit record edited | Refused at the next start (`journal.audit_invalid`) |
| **Journal rolled back** (a consistent backup from before a command, or the main file without its log) | Without protection: opened and believed, the command is gone though the device ran it; only an outside checkpoint shows it. With `--checkpoint-file`: refused at start (`journal.behind_checkpoint`); if the checkpoint cannot be written, the command is not sent (`FAILED`, `adapter.not_sent`) |

## Plans and leases

A two-step plan (set the voltage, switch the output on) makes 12 plan and lease writes:
taking the lease, recording the plan, renewing the lease and starting each step, linking each
step to its command, each step's result, releasing the lease, the plan's end. A fault at each
(before and after commit, I/O error or crash; 48 cases), then a restart, then an operator
settling the plan as documented (reconcile an uncertain step, resume; retry a step that never
ran). Every case: **each step reached the device exactly once**, recovery sent nothing, the
plan finished `SUCCEEDED`, the evidence verified. Plans already derive each step's
idempotency key from the plan, step and attempt, and recovery finds the command by that key,
so a lost link or plan record is recovered, not resubmitted.

Leases, crashing while taking or releasing one: never two holders; the holder can still use
the device and another principal cannot; the lease ends at its deadline whatever was recorded.

## Still not tested

- **Hard power loss on real storage**: whether a disk, controller or virtual machine honours
  the syncs SQLite asks for. The torn-write drill assumes it does; a device that does not is
  the rollback case, which `--checkpoint-file` catches when the checkpoint lives on storage
  that fails separately. Testing the storage itself needs a VM or a power-cut rig.
- **I/O errors produced by a faulty disk**: injected errors are `SQLITE_IOERR` at the
  journal's commit points, not from the filesystem (that needs a fault-injecting SQLite VFS,
  which the workspace's `unsafe_code = "forbid"` rules out, or a Linux `dm-flakey` device in
  CI). Disk full *is* SQLite's own error.
- **The directory deleted under a running gateway** (on Linux the open files live on,
  unlinked, and writes are lost at restart; Windows refuses the deletion).
- **Damage to the write-ahead log**: frames carry checksums, so damage ends the log there,
  which is the torn-write or rollback case; not flipped bit by bit.
- **Procedures** were not faulted separately: a procedure runs as a plan.
- Start-up cost of the checks on a Raspberry Pi, and from a genuinely cold cache. On a
  desktop it is measured ([benchmarks.md](../benchmarks.md), "Journal start-up"): a 1 GB
  journal with 1.7 million audit records opens in 6.3 s with every check, at 16 MB of peak
  memory. The first measurement found the chain check holding the whole log in memory (969 MB
  for that journal); it now streams.

## Checklist (roadmap step 1)

- [x] Identify the durable transitions: 8 per command, plus approval spending, recovery,
  12 per two-step plan, and the lease writes.
- [x] Deterministic failpoints: `Journal::inject` and `Journal::observe_commits`
  (test-only, `#[doc(hidden)]`), before or after each commit, as an I/O error, a crash, or a
  real process abort.
- [x] Crash/restart tests proving that uncertain actions are never resent automatically.
- [x] Disk full (real `SQLITE_FULL`), I/O errors, torn writes, damaged bytes, read-only
  files, rollback.
- [x] This report.
