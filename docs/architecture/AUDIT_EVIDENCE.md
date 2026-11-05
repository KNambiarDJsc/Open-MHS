# Receipt integrity and offline evidence verification

Status: implemented for the local SQLite execution kernel. Existing journals remain
readable. This work adds no database server, transport broker or cloud dependency.

Requests refused before a command exists have their own record: see [REFUSAL_AUDIT.md](REFUSAL_AUDIT.md).

## Evidence written with a command

Every new `command.created` and `command.transition` record includes:

```json
{"receipt_digest": {"version": 1, "sha256": "<64 lowercase hexadecimal characters>"}}
```

Version 1 hashes canonical receipt JSON: sorted object keys, compact encoding, and
numbers serialized by serde_json. Only `audit_hash` is removed because it points at the
audit record containing the digest and would otherwise make the hash circular. All
other receipt fields are covered, including request, principal, policy, native reference,
transition history, adapter result and observed verification values.

The digest is included in the existing hash chain in the same SQLite transaction as
the receipt update. A failed receipt write rolls back its audit record too. Before a
transition, status lookup, listing or duplicate-key replay, the journal compares the existing receipt with its last recorded digest and
refuses a mismatch (`journal.receipt_integrity`). This prevents a changed stored receipt
from being silently incorporated into a later valid digest.

Recovery also checks the stored state index against the receipt, so a changed index cannot
silently hide pending work. A corrupt held command now returns `journal.receipt_integrity`
instead of generating a fresh denial receipt; preserve the evidence and investigate the
storage before attempting further recovery of that command.

## Portable snapshot and independent verifier

The operator RPC `audit.export` returns version 1 `EvidenceBundle`: the full ordered
audit chain, every current receipt and the head sequence/hash. All rows come from a
single SQLite read transaction. Agents cannot export other principals' command evidence.

```sh
openmhs audit export --out evidence.json --checkpoint-out checkpoint.json
openmhs --json audit verify-file evidence.json --checkpoint checkpoint.json
```

Export creates new files and refuses to overwrite existing ones. Exported evidence
contains command payloads and identities and should receive the same access controls as
the journal. The verifier is a pure function in `openmhs-types::evidence` and the CLI
`verify-file` path needs neither a running gateway nor database access.

A full export costs disk proportional to journal contents; it is an explicit operator
action. Memory stays flat on both sides (RFC 0002, M2): the gateway streams the bundle
from its own read-only connection as SQLite reads it, and the CLI writes the stream to the
file and then re-reads it to check it is one whole bundle. Commands keep running during an
export; SQLite cannot checkpoint its write-ahead log past the export's snapshot until it
ends, so a very slow export lets the log grow.

`verify-file` reads the file as a stream and never holds the bundle. What it keeps is one
small entry per command and per receipt (about 0.7 KB each, measured), so its memory grows
with the number of commands, not with the length of the audit log: 19 MB for a 100 MB
export of 11,000 commands, 86 MB for a 940 MB export of 101,000. The members of the file
may come in any order, and a file that is not exactly one bundle (an unknown, duplicate or
missing member, trailing data, a cut-off file) is refused as malformed. If a log is
corrupt in a great many places, the first 1,000 invalid records are listed and the rest
are counted in one line; `valid` is false either way.

Verification checks sequence continuity, previous hashes, each record hash, the bundle
head, receipt presence/uniqueness, creation records, latest audit pointers and current
receipt digests. It reports `valid` and `complete` separately. `verify-file` exits 0 only
when both are true, 3 for detected damage or incomplete coverage, and 1 for an unreadable
or malformed input. Existing `audit verify` retains its chain-only meaning for compatibility.

## Segments and compaction

A long journal can be cut into **sealed segments** ([RFC 0002](../rfcs/0002-storage-engine.md),
milestones 3 to 5). A segment is a file holding a closed range of the chain exactly as stored,
the final receipts of the commands whose last record lies in it, an index and a footer with
the file's SHA-256. `openmhs audit verify-segments DIR` checks a directory of them with no
gateway and no journal: every record's hash and link, the receipts against the records that
bind them, the index, the footer, and that the files follow on from one another. Any changed
bit, a truncation or trailing data is detected. `--checkpoint FILE` also says whether a
separately retained checkpoint lies in them. `audit verify-file` accepts a single segment.

`audit seal` appends an `audit.segment_sealed` record that commits the chain to a segment's
file and to a snapshot of live state, so a seal in the chain proves the segment followed from
that point of this chain. It does not prove more than that against someone who controls the
journal and can rewrite the chain; only a checkpoint kept elsewhere does, as above.

`audit compact`, only on a gateway started with `--archive`, then deletes from the journal the
rows a sealed segment replaces, keeping everything live. After that **the journal alone no
longer proves the deleted history: the segment does**, so the segment files are evidence and
must be kept and backed up with the journal. A lost or damaged segment is reported
(`audit verify --all`, `journal.archive_unavailable`), never treated as an empty history.
The evidence export of a compacted journal is the same bundle an uncompacted one gives: it
is read from the segments, then the journal, and verifies with `verify-file`. A compacted
journal cannot be opened by older versions of the gateway.

## Routine reads

With `openmhs serve --telemetry DEVICE:CAPABILITY`, an allowed, successful read of a matching
device is not one audit record: the audit log gets a `read.summary` record per window with the
number of reads, the first and last reading, the minimum and maximum of a quantity, the
quality counts, and how many raw readings the telemetry store could not keep. The raw
readings are in `telemetry.sqlite` for a short time. **A summary is not evidence of any one
reading.** Denied reads, failed reads and every read of a device nobody opted in are still
individual records, and so is everything about commands, approvals, plans, leases and
credentials: only a routine successful read can be aggregated, and nothing is aggregated by
default. See the event classes in
[audit_class.rs](../../crates/openmhs-types/src/audit_class.rs).

## Legacy journals

Older audit events contain no receipt digest. Their command ids appear in
`unbound_commands`; the report is incomplete even when the chain is valid. An old journal
is not rewritten and no retroactive proof is fabricated. A later legitimate transition
binds the resulting receipt prospectively; it does not authenticate the original legacy
history. Legacy receipt content was not protected by a digest before that transition.

## Checkpoint trust and limits

The bundle's own checkpoint detects internal inconsistency. To detect an attacker
rewriting or truncating the chain, retain a checkpoint separately *before* the suspicious
change, under access controls unavailable to the journal writer. A later export may extend
that checkpoint and still verify, as long as its sequence/hash remains in the chain.
Keeping a checkpoint beside the database under the same writer's control is not an
independent anchor. The CLI does not arrange remote storage or authenticate checkpoint
provenance; signed checkpoints and external retention remain future work.

A privileged actor who can rewrite the journal and all retained checkpoints can still
forge a consistent history. These checks establish consistency and evidence binding, not
physical truth, clock accuracy, device safety or resistance to every local compromise.

## Validation scope

`openmhs-engine/tests/evidence.rs` exercises full receipt-field corruption, missing or
duplicate receipts, modified audit records, audit pointer changes, tail truncation,
consistent rewrites, legacy coverage, refusal to rebind a corrupt receipt, and rollback
after an injected SQLite update failure. `recovery_windows.rs` reopens each persisted
command boundary, recovers twice, and checks zero device execution calls and native
reference retention. These are deterministic persisted-state tests; the existing CLI
process-kill test additionally covers a gateway dying during dispatch.
