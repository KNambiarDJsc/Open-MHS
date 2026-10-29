# Failure invariants and evidence

These are engineering invariants, not a claim of physical safety or production readiness.

| Invariant | Existing mechanism | Evidence / remaining gap |
|---|---|---|
| No dispatch without authorization | Gateway-authenticated caller; scope and grant checks in engine | Policy tests and gateway tests exist; expand agent-facing negative coverage |
| No blind retry after an ambiguous write | `UNKNOWN_OUTCOME`, unresolved hold, idempotency index | Research simulator evidence and engine recovery tests; not a universal device guarantee |
| No false verified result from acknowledgement | `ACCEPTED`/`APPLIED` are pending; post-dispatch readback | Lifecycle and stale-readback tests; physical sensor truth is not established |
| No two ordinary writes concurrently on one device | Per-device async mutex held across checks and verification | Engine tests; no distributed split-brain fencing exists |
| Admitted work survives process restart as evidence | SQLite command receipts and transitions | Recovery tests; disk loss/corruption and full-disk crash windows need expanded drills |
| Unknown devices are explicit errors | Gateway descriptor lookup returns `command.device.unknown`; insights now validates first | MCP regression test added for insights and access discovery |
| Permission discovery cannot grant authority | New query intersects credential scope and policy; each execution rechecks | MCP test added; output is advisory and expires after 10 seconds |

## Integrity limits

The SQLite audit uses a hash chain that detects edits and middle deletions under the tested
cases. It does not by itself prevent a privileged local actor from recomputing the chain or
detect tail truncation without an independently retained checkpoint. Receipts are stored in
the command row and transition audit records, but the research audit found that the mutable
receipt was not fully covered by the chain. New command events now include receipt digests;
legacy commands are explicitly reported as incomplete coverage by the independent verifier.
See [AUDIT_EVIDENCE.md](AUDIT_EVIDENCE.md). A digest and export cross-check are not equivalent
to an independent signature. Do not describe the journal as tamper-proof.

## Verification coverage and remaining drills

The evidence and recovery tests now cover receipt corruption, missing/duplicate receipts,
checkpoint mismatch, consistent chain rewrites, tail truncation, SQLite transaction
rollback, native reference retention and reopening eight persisted lifecycle states.
The lifecycle also has generated-history invariants in addition to its exhaustive matrix.
Broader drills remain:

- Crash immediately before/after the durable dispatch transition and device send.
- Lost device acknowledgement, duplicated RPC submission and process restart in each state.
- Interrupted reconciliation and readings that predate dispatch.
- Full disk while admitting, transitioning, stopping or resolving work.
- Power loss and filesystem failures beyond the SQLite fault/rollback tests.
- Stale policy results after grant update; credential revocation between discovery and submit.
- Concurrent gateways attempting the same physical resource (currently no cross-process fence).
- LabThings server restart after accepting an action whose invocation record is volatile.

Simulator tests can show deterministic software behavior. They do not establish controller
fencing, device-side deduplication, physical completion, safety, or certification.
