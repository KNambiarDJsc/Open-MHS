# Failure invariants and evidence

These are engineering invariants, not a claim of physical safety or production readiness.

| Invariant | Existing mechanism | Evidence / remaining gap |
|---|---|---|
| No dispatch without authorization | Gateway-authenticated caller; scope and grant checks in engine | Policy tests and gateway tests exist; expand agent-facing negative coverage |
| No blind retry after an ambiguous write | `UNKNOWN_OUTCOME`, unresolved hold, idempotency index | Research simulator evidence and engine recovery tests; not a universal device guarantee |
| No false verified result from acknowledgement | `ACCEPTED`/`APPLIED` are pending; post-dispatch readback | Lifecycle and stale-readback tests; physical sensor truth is not established |
| No two ordinary writes concurrently on one device | Per-device async mutex held across checks and verification | Engine tests; no distributed split-brain fencing exists |
| Admitted work survives process restart as evidence | SQLite command receipts and transitions | Recovery tests; disk loss/corruption and full-disk crash windows need expanded drills |
| Unknown devices are explicit errors | Gateway descriptor lookup returns `device.not_found`; insights now validates first | MCP regression test added for insights and access discovery |
| Permission discovery cannot grant authority | New query intersects credential scope and policy; each execution rechecks | MCP test added; output is advisory and expires after 10 seconds |

## Integrity limits

The SQLite audit uses a hash chain that detects edits and middle deletions under the tested
cases. It does not by itself prevent a privileged local actor from recomputing the chain or
detect tail truncation without an independently retained checkpoint. Receipts are stored in
the command row and transition audit records, but the research audit found that the mutable
receipt is not fully covered by the chain. An export cross-check is not equivalent to an
independent signature. Do not describe the journal as tamper-proof.

## Failure cases still requiring explicit tests

- Crash immediately before/after the durable dispatch transition and device send.
- Lost device acknowledgement, duplicated RPC submission and process restart in each state.
- Interrupted reconciliation and readings that predate dispatch.
- Full disk while admitting, transitioning, stopping or resolving work.
- Corrupted command receipt versus intact audit rows; audit tail truncation and missing sequence.
- Stale policy results after grant update; credential revocation between discovery and submit.
- Concurrent gateways attempting the same physical resource (currently no cross-process fence).
- LabThings server restart after accepting an action whose invocation record is volatile.

Simulator tests can show deterministic software behavior. They do not establish controller
fencing, device-side deduplication, physical completion, safety, or certification.
