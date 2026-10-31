# RFC 0002: Storage engine: restart cost bounded by live state

- **Status:** draft (milestones 1 and 2 and the streaming verifier implemented; everything after them is a proposal for review)
- **Author:** OpenMHS maintainers (drafted with Claude Code)
- **Date:** 2026-10-31
- **Touches:** journal format / stored journals / API / tooling / security model
- **Builds on:** [STORAGE_AND_SCALING_RFC.md](../strategy/STORAGE_AND_SCALING_RFC.md) (event
  classes, retention, telemetry store), [ADR-0021](../adr/0021-journal-quota-and-reserve.md)
  (quota and reserve), [PERSISTENCE_ADVERSARY.md](../architecture/PERSISTENCE_ADVERSARY.md)
  (failure drills), [benchmarks.md](../benchmarks.md) "Journal start-up"

## Summary

A gateway's start-up work today grows with everything it has ever done: every audit record
is re-hashed and every command receipt was parsed on each start. This RFC sets the goal
that **start-up time and memory depend on live execution state (unfinished commands, open
approvals, leases and plans), not on lifetime history**, without weakening dispatch
durability, idempotency, recovery or offline audit verification. It keeps SQLite and the
single-gateway design. It gives each kind of data the structure its access pattern needs:
indexed live tables, a permanent identity index, the hash-chained log split into sealed
segments, and a separate telemetry store. Milestone 1 (index-driven recovery, with the
unfinished set checked against the chain at open) is implemented and measured below; the
rest is staged so that **nothing is deleted from a journal until the crash drills for
archival pass**.

## Motivation

Measured on a desktop (i7-10700, NVMe, warm cache, release build; [benchmarks.md](../benchmarks.md)):

| Journal | Audit records | Open (all checks) | Recovery | Peak memory |
|---|---|---|---|---|
| 100k verified writes x 10 reads, 964 MB | 1.7 M | 6.3 s | 1.4 s before M1, 1.1 ms after | 16 MB |

- Peak memory is now bounded (streaming chain check, 6b780de). **Time is not**: the chain
  check costs about 2.4 us per record on this machine and grows with history.
- Allowed reads are 59% of the records in that journal (1.0 M of 1.7 M), and each is a
  synced write. Read rate, not command rate, sets growth.
- Until milestone 1, recovery parsed every stored receipt (1.4 s at 100k commands) to find
  the handful still unfinished.

Extrapolation, **not a measurement**: at the same rate, 10 M records is about 24 s of chain
check and 100 M about 4 min. Cold caches and slower storage (a Raspberry Pi's SD card) will
be worse; those must be measured, not guessed.

## Phase 0: what the journal holds today

Everything is in one SQLite file, schema in [journal.rs `init`](../../crates/openmhs-engine/src/journal.rs).
Every lifecycle change is one transaction that updates the row and appends a hash-chained
audit record ([`transition_tx`](../../crates/openmhs-engine/src/journal.rs)).

### What must survive a restart, and why

| Data | Table / index | Needed for | How long |
|---|---|---|---|
| Unfinished commands (9 non-terminal states, derived from the lifecycle table by `CommandState::is_terminal`) | `commands`, `commands_state` | Recovery (lifecycle.md §4), capability holds for unknown outcomes ([`open_commands_on_devices`](../../crates/openmhs-engine/src/journal.rs), `unresolved_commands`) | While unfinished |
| Idempotency identity `(principal, device, key)`, request hash, **and the receipt** | `commands_idempotency` (unique, partial) | Rule 4 of [lifecycle.md §4](../../spec/lifecycle.md): a repeated key returns the existing command and receipt and MUST NOT dispatch again. **No time limit is specified** | Forever, as specified |
| A command's latest lifecycle audit record | `audit_subject (subject, seq)` | Every receipt read is checked against it ([`check_receipt`](../../crates/openmhs-engine/src/journal.rs)); the `DISPATCHED` record binds the readback target ([`expectation`](../../crates/openmhs-engine/src/journal.rs)) | As long as the receipt is served |
| Open approvals; spent approvals bound to a command | `approvals`, `approvals_command` (unique) | One approval spent once ([`consume_approval`](../../crates/openmhs-engine/src/journal.rs)) | Open: until final. Spent: an unknown id is refused anyway, so the row can go once its command is archived |
| Open leases | `leases`, `leases_open` | Exclusive use, expiry at recovery | Until ended |
| Open plans and their step links | `plans`, `plan_commands` | Plan recovery finds steps by derived idempotency key ([`command_by_key`](../../crates/openmhs-engine/src/journal/plans.rs)) | Until the plan ends |
| The audit chain | `audit` | Offline verification, evidence export, and the start-up refusal of an edited chain | Policy (strategy RFC §3); today forever |

### Findings

| # | Finding | Where | Status |
|---|---|---|---|
| F1 | Recovery parsed every receipt to find unfinished ones although `commands_state` exists | [`commands_in_states`](../../crates/openmhs-engine/src/journal.rs) | **Fixed in M1** |
| F2 | **A command's state edited in both the row and the receipt to a final state was believed**: the old scan only compared the two, so an `UNKNOWN_OUTCOME` stopped holding its capability and the next write was sent. Reproduced against 6b780de | `commands` row vs chain | **Fixed in M1**: refused at open |
| F3 | `audit.tail` loaded the whole audit log into memory to return the last N records | [server.rs `AUDIT_TAIL`](../../crates/openmhs-gateway/src/server.rs) via `audit_records` | **Fixed in M2**: 973 MB per call on a 1 GB journal, now 16 MB |
| F4 | `audit.export` built the whole log and all receipts in memory: twice in the gateway (the bundle, then a JSON value) and again in the CLI | [`export_evidence`](../../crates/openmhs-engine/src/journal.rs) | **Fixed in M2**: streamed on both sides |
| F5 | `commands.list` and `approvals.list` filtered in Rust after an ordered full scan; `open_plans` used `NOT IN` (no index) | [`list_commands`](../../crates/openmhs-engine/src/journal.rs), `approvals`, [`open_plans`](../../crates/openmhs-engine/src/journal/plans.rs) | **Fixed in M2** |
| F6 | Each allowed read is a synced audit write; 59% of records in the benchmark journal | [`Engine::read`](../../crates/openmhs-engine/src/engine.rs) | Strategy RFC §3 (classes); M6 |
| F7 | The journal file and the gateway home get default permissions (the token and policy stores use 0600) | [gateway `start`](../../crates/openmhs-gateway/src/lib.rs) `create_dir_all`; SQLite creates the file | Open: on a 022 umask other local users can read the journal. Proposed: home 0700 |

### Durability configuration (checked, unchanged)

- WAL with `synchronous=FULL`, set on every open ([`init`](../../crates/openmhs-engine/src/journal.rs)): a
  commit is synced to the log before it returns, so an intent record survives power loss
  on storage that honours syncs. Do not lower it; `NORMAL` can lose the last commits on
  power loss, and the last commit before a device call is the `DISPATCHED` record.
- Integrity at open: `quick_check`, `integrity_check` of `commands` and `approvals` with
  their unique indexes, the full chain, and now the unfinished set (M1).
- The readback target is written outside a transaction with an audit record but bound by
  its SHA-256 in the `DISPATCHED` record that follows (`set_expectation`).
- `--checkpoint-file` writes, syncs and renames the head checkpoint, and syncs the directory
  on Unix ([`write_checkpoint`](../../crates/openmhs-engine/src/engine.rs)).
- Not established: whether a given disk or VM honours syncs (needs a power-cut rig), and
  disk-level I/O errors (needs `dm-flakey`). Both are listed in the adversary report.

## Design

### Four structures, chosen by access pattern

| Data | Structure | Cost | Notes |
|---|---|---|---|
| Live command state | SQLite B-tree `commands` + the existing `commands_state` index | Lookup O(log N); unfinished set O(log N + K) | A partial index would only save space; the full index already gives this. The unfinished states come from the lifecycle table in code, never a hand-written list |
| Identity (idempotency, command id) | Permanent table `command_keys(principal, device, key) -> command_id, request_hash, location` with a unique index | O(log N) | Today this is `commands_idempotency`. After archival it is the only per-command row that stays, about 150 bytes each; a 1 M-command gateway keeps ~150 MB |
| Audit evidence | Append-only hash chain, unchanged byte for byte, cut into sealed segments | Append O(1) plus the sync | Old journals and exported evidence keep verifying |
| Routine telemetry | Separate store, per the strategy RFC §3–4 | O(1) insert | Opt-in per class; security and command evidence are never aggregated |

Ephemeral coordination stays in memory as it is (per-device and per-resource locks in the
engine; plan step DAG, ADR-0025).

### Sealed segments

A segment is a closed, contiguous range of the chain, written once:

```text
header   format version, gateway journal id, segment number, first_seq, last_seq,
         previous segment's last hash
records  every audit record of the range, in seq order, exactly as stored
receipts the final receipt of every command whose last lifecycle record is in the range
index    sparse seq index (every 256th seq -> byte offset); sorted (command_id -> offset)
footer   record count, last chain hash, SHA-256 of everything above
```

- Lookup by seq: binary search of the sparse index, then a scan of at most 256 records.
  By command: the identity index names the segment, the sorted id table the offset.
- The chain itself proves order and content inside a segment, and the `prev` link proves
  continuity between segments. **A Merkle tree is not proposed**: it does not make start-up
  faster, and it only pays off when per-record membership proofs are needed without
  shipping the segment. Revisit if evidence bundles need them.
- The manifest is a table in the journal (`segments`: number, seq range, last hash, file
  SHA-256, state), so recording a segment and deleting its rows can share one transaction.
  A copy is written next to the segments for offline verifiers.

### The seal record and the live-state snapshot

Sealing appends one chain record, `audit.segment_sealed`, holding the segment's last seq,
last hash, file SHA-256, and a **snapshot of live state at that seq**: the ids of unfinished
commands, open approvals, leases and plans, with the SHA-256 of that set.

Start-up then needs only the tail:

1. SQLite `quick_check` and the table integrity checks (now on a bounded file).
2. Verify the chain from the last seal record to the head.
3. Rebuild the unfinished set from the snapshot plus the tail's lifecycle records, and
   require it to equal the indexed set. This is milestone 1's check, starting at a seal
   instead of at genesis.
4. Check the seal record against the manifest row and the segment footer (no read of the
   segment body). Full archive verification runs separately (`openmhs audit verify --all`)
   and never makes an unverified archive count as evidence.
5. Recover through the index.

### Trust: what a faster start may and may not assume

| Checkpoint | Proves | Against whom |
|---|---|---|
| Seal record in the local chain | The segment followed from that point of this chain | Accidental damage; an edit that does not rewrite the chain |
| `--checkpoint-file` on separately failing storage (exists) | The journal has not been rolled back behind it | Rollback, lost writes |
| Signed seal (Ed25519, M8 key machinery) | The gateway's key endorsed this seal | A writer of the archive without the key |

A writer who controls the journal **and** the key can rewrite history today and after this
change; only an anchor kept elsewhere (the checkpoint file on another machine, or an
external log) detects that. **Skipping re-verification of sealed history is allowed only
when the last seal is at or behind an external checkpoint.** Otherwise the gateway verifies
the whole local chain at start, as it does now. If current-state integrity (steps 1–3)
cannot be established, the gateway refuses to start, as it does now.

### Archival state machine

```text
PLANNED -> WRITTEN -> VERIFIED -> SEALED -> COMPACTED
```

| State | Durable effect | After a crash here |
|---|---|---|
| PLANNED | Manifest row with the seq range | Discard the row and any partial file |
| WRITTEN | Segment file written, synced, renamed, directory synced | Re-verify, or discard and rewrite |
| VERIFIED | File re-read and checked against the live rows: every record byte for byte, chain continuity, receipts against their digests | Continue to seal |
| SEALED | Seal record appended to the chain and manifest row updated, in one transaction | Continue to compact; the rows still exist |
| COMPACTED | Archived rows deleted in one transaction | Done |

Rows may leave the active journal only if all of these hold:

1. Their seq is at or below a `SEALED` segment's last seq.
2. They are not needed by live state: every record of an unfinished command, of an open
   approval, lease or plan, and of any command a live plan links to, stays, wherever it
   sits in the chain.
3. Every archived keyed command has its `command_keys` row with the segment as location.
4. A repeated idempotency key for an archived command returns the archived receipt and
   sends nothing. If the segment cannot be read, the request is refused
   (`journal.archive_unavailable`), never treated as new.

Duplicate archiving (the same range twice) is refused by the manifest's range check.
A missing or damaged segment file never deletes anything and is reported by
`audit verify --all`.

## Safety and security

- Dispatch durability, the lock discipline and recovery semantics are unchanged by every
  milestone. Recovery never re-dispatches.
- M1 strengthens a start-up check (F2): the indexed unfinished set must equal the set the
  chain leaves unfinished.
- Archival changes what a local journal alone can prove: deleted rows are proved only by
  their segment. So deletion happens only after verification, and only behind an explicit
  operator setting.
- Idempotency stays permanent unless a separate spec change sets a retention window
  (open question 1). Until then the identity index is never compacted.

## Compatibility

- M1: no schema change. Old journals open as before. A journal whose stored state disagrees
  with its chain, which was previously accepted or failed later at recovery, is now refused
  at start with `journal.receipt_integrity`.
- Chain hashes are unchanged byte for byte (pinned by
  `audit_hash_is_unchanged_from_the_joined_string`). New audit kinds (`audit.segment_sealed`)
  verify with the existing algorithm, so old verifiers still check new chains.
- Schema additions (`segments`, `command_keys`) are additive. The first start on the new
  version fills `command_keys` from `commands` in one transaction, with nothing deleted.

## Migration plan

Each milestone ships alone, with its own tests, and leaves every journal readable.

| # | Milestone | Deletes anything? | Exit test |
|---|---|---|---|
| **M1** | **Index-driven recovery; unfinished set checked against the chain at open** | No | **Done**: below |
| **M2** | **Bounded API reads: `audit.tail` by `ORDER BY seq DESC LIMIT`, list filters in SQL, `audit.export` streamed to a file (F3–F5)** | No | **Done**: below |
| M3 | Segment writer and offline verifier: `openmhs audit segment` writes and verifies segments for a seq range; nothing is deleted. The streaming `verify-file` it needs is done (below) | No | Round trip: every record and receipt byte for byte; `verify-file` accepts it; bit flips in the file are detected |
| M4 | Seal records with snapshots; start-up verifies the tail when an external checkpoint covers the last seal | No | Same refusals as today for every adversary case; start-up flat in history |
| M5 | `command_keys`; compaction behind `--archive` | **Yes**, behind a flag | The archival drills below all pass, and the bit-flip and torn-write drills are rerun on compacted journals |
| M6 | Event classes and telemetry store (strategy RFC §3–5) | Raw telemetry, by policy | No loss of `CRITICAL` or `SECURITY` records under quota pressure |
| M7 | Evaluation: 10 M-record journals, cold cache, Raspberry Pi | No | Published with method and raw data |

Archival drills for M5: a crash before and after every state transition; full disk at each;
a lost segment file; a damaged manifest; duplicate archiving; partial compaction; a journal
restored from a snapshot older than its segments; receipt lookup after archiving; reuse of
an idempotency key after archiving; unfinished commands, approvals and plans that span a
segment boundary; offline verification of segments plus tail.

## Milestone 1, implemented

- [`commands_in_states`](../../crates/openmhs-engine/src/journal.rs) selects with
  `WHERE state IN (...)` through `commands_state`, then checks each returned row as before
  (stored state against receipt, receipt against its audit record). A unit test pins the
  query plan to the index.
- [`Journal::open`](../../crates/openmhs-engine/src/journal.rs) collects, in the chain pass it already
  makes, the commands whose last lifecycle record leaves them unfinished, and requires that
  set to equal the indexed one (`check_unfinished`). This keeps, and extends, the check the
  old full scan made: a state edited only in the row, edited in row and receipt together
  (F2), or a finished command made to look in flight are all refused at open
  ([persistence_storage.rs](../../crates/openmhs-engine/tests/persistence_storage.rs),
  `a_state_edited_to_look_finished_is_refused_at_start`). The existing
  `a_changed_state_index_cannot_hide_admitted_work_from_recovery` (evidence.rs) pinned the
  old mechanism on an in-memory journal; it now checks the same property through a reopen.
- Memory of the check: one id per unfinished command.

### Measured

Same desktop and method as [benchmarks.md](../benchmarks.md) "Journal start-up": the old
(6b780de) and new code built from the same harness, alternating on each journal, five opens
each in fresh processes, medians, warm cache. `L` is the number of unfinished commands
(writes waiting for approval, which recovery visits and leaves unfinished).

| Journal | Audit records | Recovery before | Recovery after | Open before | Open after | Peak memory after |
|---|---|---|---|---|---|---|
| 1k writes, L = 0 | 17 k | 15 ms | 1.1 ms | 76 ms | 71 ms | 14 MB |
| 10k, L = 0 | 170 k | 172 ms | 1.2 ms | 0.67 s | 0.68 s | 14 MB |
| 10k, L = 1,000 | 174 k | 195 ms | 50 ms | 0.78 s | 0.74 s | 16 MB |
| 100k, L = 0 | 1.70 M | 1.21 s | 1.1 ms | 6.6 s | 6.8 s | 14 MB |
| 100k, L = 100 | 1.70 M | 1.38 s | 6.9 ms | 7.8 s | 7.2 s | 14 MB |
| 100k, L = 1,000 | 1.70 M | 1.32 s | 48 ms | 6.8 s | 6.8 s | 16 MB |
| 100k, L = 10,000 | 1.74 M | 1.93 s | 0.52 s | 7.4 s | 7.4 s | 35 MB |

- Recovery now follows live state, about 50 us per unfinished command, and is the same on a
  10k and a 100k history (50 ms and 48 ms at L = 1,000).
- The open-time check costs nothing measurable: open times differ by -8% to +3%, within
  run-to-run spread. Open is still linear in history; that is M3–M4.
- **The old recovery also needed temporary disk.** Its `ORDER BY` over every receipt had no
  usable index, so SQLite sorted all receipts in a temporary file. In the first A/B run the
  temporary directory (on a drive with about 90 MB free) filled and the old code failed to
  recover every 100k journal with `SQLITE_FULL`; the new code, sorting only unfinished rows,
  did not. On a device whose `/tmp` is in RAM that sort would have been memory the peak
  working set does not show.
- Peak memory at L = 10,000 (35 MB) is the recovered receipts, held at once; bounded by live
  state, as intended.

## Milestone 2, implemented

- **`audit.tail`** reads `ORDER BY seq DESC LIMIT n`
  ([`Journal::audit_tail`](../../crates/openmhs-engine/src/journal.rs)).
- **`commands.list` and `approvals.list`** filter by owner and state in SQL and stop at the
  limit. Four indexes, `commands(created_ms, command_id)`,
  `commands(principal, created_ms, command_id)` and the same two on `approvals`, give the
  newest rows without sorting the table; a unit test pins each query plan to its index and
  rejects a temporary sort. Every returned receipt is still checked against its stored state
  and its audit record. Rows a filter skips are no longer checked on the way past: a state
  edited to hide a command from a filter is caught at open by the M1 check instead.
- **`open_plans`** selects `state IN (open states)` through `plans_state`.
- **`audit.export`** keeps its method and response, but the gateway writes the bundle into
  the response body as SQLite reads it
  ([`Journal::write_evidence`](../../crates/openmhs-engine/src/journal.rs)), in 64 KiB
  chunks, on a read-only connection of its own: the journal lock is not held, so commands
  keep running while a slow client reads. The bundle's `checkpoint` now comes first; JSON
  member order is not significant, and `verify-file` reads old and new files. An error
  before the first chunk is an ordinary error response; after it, the body breaks off. The
  CLI writes everything after `"result":` except the closing brace straight to the file
  (any other response shape, such as an error, is parsed whole), then re-reads the file
  with a streaming parser to check it is one whole bundle and to take its checkpoint; a
  failed export leaves no file. Clients that read the whole response, like the Python SDK
  or an older CLI, get the same bundle (gateway test
  `evidence_export_streams_the_same_bundle_to_every_client`).
- **Compatibility:** the new version builds the new indexes at its first open
  (`CREATE INDEX IF NOT EXISTS`), and older versions ignore them. Nothing is deleted or
  rewritten. The 1 GB journal grew by 11 MB.
- **Tests:** [journal_reads.rs](../../crates/openmhs-engine/tests/journal_reads.rs) checks
  that:
  - the filters return what a full read would, for every combination of owner, state set
    and limit;
  - an export is the whole journal;
  - an export neither holds the journal nor sees writes made while it runs;
  - a failed export leaves the journal usable.

  The query-plan unit test, the gateway test above, and the existing CLI export and
  `verify-file` test also cover this milestone.

### Streaming verifier, implemented

`openmhs audit verify-file` used to load the whole bundle, so verifying evidence cost
memory proportional to the journal; M3's verifier of segments needs the other behaviour, so
it is done first.

- [`EvidenceVerifier`](../../crates/openmhs-types/src/evidence.rs) is the verifier as a state
  machine: it is given audit records and receipts one at a time and keeps, per command, the
  latest record's hash and the receipt digest it commits to, and per receipt its id, pointer
  and computed digest. It never keeps a record. `verify_evidence` (a whole bundle in memory,
  as before) now runs on it, and `verify_evidence_reader` feeds it from a JSON reader with
  serde visitors. So there is one set of checks, not two.
- Receipts are compared with the chain only at the end, so the members of the file can come
  in any order, as JSON allows. The report is identical to the old one, including the order
  of errors. A test keeps the old implementation as a reference and requires equal results
  for 16 cases (good, edited, missing or duplicate receipts, forged receipt, wrong pointer,
  version, checkpoint, truncation with and without an anchor, a consistent rewrite, a legacy
  record, a record that is not JSON, an empty log) in three member orders.
- A file that is not exactly one bundle is refused: every missing member, an unknown member
  (also inside a record), a duplicate member, trailing data, a cut-off file.
- One change in the report: after 1,000 invalid records the rest are counted in one line.
  Otherwise a wholly corrupt log of 1.7 million records would produce 1.7 million lines.
- The CLI reads the file through a 64 KiB buffer. The old reader was unbuffered.

Measured with the release CLI of M2 (before) and of this change (after), three runs each,
on the files `audit export` made from the 100 MB and the 1 GB journals above:

| Export file | Before: time, peak memory | After: time, peak memory |
|---|---|---|
| 101 MB (174 k records, 11 k receipts) | 155 s, 130 MB | 1.6 s, 19 MB |
| 940 MB (1.70 M records, 101 k receipts) | not run | 15 s, 86 MB |

Both gave the same report on the 101 MB file. Most of the old time is the unbuffered file
reader (many tiny reads), not the checks, so the 155 s is not a fair measure of the checks
alone. The old verifier was not run on the 940 MB file: the machine had 3.6 GB free and its
page file is on a full drive. The after memory is 19 MB for 11 k commands and 86 MB for
101 k, about 0.7 KB per command; that is two measurements, not a fit.

### Measured

Each call was measured end to end through a real gateway and its own CLI, with the old
(21983b0) and new versions both built in release mode. For each journal, call and version,
the benchmark started a fresh gateway on a copy of the journal and ran the call three
times in fresh CLI processes. It then recorded the gateway's peak working set. Old and new
runs alternated. The machine is the same desktop as above, with a warm cache. Times include
starting the CLI process (about 30 ms).

| Journal | Call | Time before | Time after | Gateway peak before | Gateway peak after | CLI peak before | CLI peak after |
|---|---|---|---|---|---|---|---|
| 10k writes, 1,000 open (100 MB, 174 k records) | `audit tail` | 317 ms | 31 ms | 114 MB | 16 MB | 14 MB | 14 MB |
| | `commands list` | 112 ms | 52 ms | 19 MB | 17 MB | 15 MB | 15 MB |
| | `commands list --state AWAITING_APPROVAL` | 193 ms | 47 ms | 18 MB | 16 MB | 14 MB | 15 MB |
| | `audit export` | 2.5 s | 1.0 s | 519 MB | 19 MB | 477 MB | 14 MB |
| 100k writes, 1,000 open (1 GB, 1.70 M records) | `audit tail` | 2.33 s | 31 ms | 973 MB | 16 MB | 14 MB | 14 MB |
| | `commands list` | 513 ms | 47 ms | 18 MB | 17 MB | 15 MB | 15 MB |
| | `commands list --state AWAITING_APPROVAL` | 1.44 s | 46 ms | 18 MB | 17 MB | 15 MB | 15 MB |
| | `audit export` | not run | 8.5 s | not run | 21 MB | not run | 15 MB |

- **Memory:** every call's memory is now flat across journal sizes.
- **Time:** tail and list times are flat too. Export time stays linear, because it writes
  the whole history: 940 MB in 8.5 s.
- **The old export on the 1 GB journal was not run.** At 100 MB it peaked at 519 MB in the
  gateway and 477 MB in the CLI at the same time. The machine had 3.6 GB free, and its page
  file is on a full drive. How far the old export's memory would have grown at 1 GB was not
  measured.
- **Open time:** the new indexes leave it unchanged. On the 1 GB journal it was 6.35 s
  before and 6.34 s after (medians of five alternating opens, each version on its own copy).
  The first open, which builds the indexes, fell within the same spread.
- **First start:** the first gateway start on each fresh copy took 15–16 s for both
  versions. The copy had just been written, so this time mixes in cache effects and is not
  a cold-cache measurement.

## Alternatives considered

- **Do nothing.** Start-up stays linear in history: about 24 s at 10 M records on this
  desktop by extrapolation, more on a Pi.
- **A mutable "verified up to" marker in the journal.** Rejected: anyone who edits the
  journal edits the marker. A skip is allowed only against an external anchor.
- **PostgreSQL, Kafka or a custom LSM.** Rejected: the problem is which data is re-read at
  start, not the engine; SQLite already gives the B-trees and transactions the live state
  needs, and one file keeps the single-gateway failure model simple.
- **`VACUUM` or `mmap`.** Neither changes what has to be re-read or proved; `VACUUM` reclaims
  space after compaction (M5) and nothing more.
- **Merkle trees per segment from the start.** Deferred (see Sealed segments).

## How it will be tested

The persistence drills ([PERSISTENCE_ADVERSARY.md](../architecture/PERSISTENCE_ADVERSARY.md))
are the regression suite for every milestone. Each milestone adds its own cases (table
above). The start-up benchmark (`crates/openmhs-engine/examples/journal_startup.rs`) varies
history and live state independently (`generate PATH N R L`), and is run old against new in
the same session.

## Open questions

1. **Idempotency retention.** Keep keys forever (about 150 bytes per keyed command), or
   change the spec to a documented window after which a key may be reused? This is a
   contract with callers, not an implementation detail. *Standing decision until the
   owner chooses otherwise: forever, which is what the spec and the code do today.* Nothing
   in M1–M3 depends on the answer, and no milestone changes the contract. Only M5 does
   anything with keys: it keeps the permanent `command_keys` index, so a key is still
   honoured after its command is archived. A retention window would be a separate spec
   change, with its own RFC and a migration for callers that rely on a key never being
   reused.
2. Segment size: by record count, bytes or time? Proposed: bytes, so that a segment's
   verification time is bounded.
3. Must archived receipts be served by the live API (`commands.get`), or only by offline
   tools?
4. Signing keys for seals: reuse the M8 identity key or a dedicated one, and its rotation.
5. Which reads become `TELEMETRY` (strategy RFC §10, item 1, says none by default).
