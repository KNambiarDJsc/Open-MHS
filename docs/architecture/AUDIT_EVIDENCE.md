# Receipt integrity and offline evidence verification

Status: implemented for the local SQLite execution kernel. Existing journals remain
readable. This work adds no database server, transport broker or cloud dependency.

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
`verify-file` path needs neither a running gateway nor database access. A full export
costs memory and disk proportional to journal contents; it is an explicit operator action.

Verification checks sequence continuity, previous hashes, each record hash, the bundle
head, receipt presence/uniqueness, creation records, latest audit pointers and current
receipt digests. It reports `valid` and `complete` separately. `verify-file` exits 0 only
when both are true, 3 for detected damage or incomplete coverage, and 1 for an unreadable
or malformed input. Existing `audit verify` retains its chain-only meaning for compatibility.

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
