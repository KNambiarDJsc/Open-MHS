# ADR-0002: Canonical command lifecycle with explicit uncertainty

- Status: Accepted
- Date: 2026-10-23

## Context

Commands to physical devices fail in ways software APIs usually don't model:
acknowledgements get lost, devices accept but don't apply, applied values drift,
and the gateway itself can crash between sending and recording. The community
MHS-style projects we inspected (see `docs/research/landscape.md`) model a command
as allowed/blocked/returned. That is not enough to avoid duplicate physical effects
or to tell an agent truthfully what happened.

## Decision

Every command on every device follows one state machine:

```
CREATED → VALIDATING → { DENIED | AWAITING_APPROVAL | APPROVED }
AWAITING_APPROVAL → { APPROVED | DENIED | EXPIRED }
APPROVED → { DISPATCHED | CANCELLED | EXPIRED }
DISPATCHED → { ACCEPTED | REJECTED | FAILED | TIMED_OUT }
ACCEPTED → { APPLIED | FAILED | TIMED_OUT }
APPLIED → { VERIFIED | VERIFICATION_FAILED | UNVERIFIABLE }
TIMED_OUT → UNKNOWN_OUTCOME
UNKNOWN_OUTCOME → { RECONCILED_APPLIED | RECONCILED_NOT_APPLIED }
```

Rules:

1. `ACCEPTED` = device acknowledged. `APPLIED` = device reports applied.
   `VERIFIED` = independent readback confirms. Acknowledgement never implies effect.
2. A timeout never becomes `FAILED`. It becomes `UNKNOWN_OUTCOME`, resolvable only
   by readback-based reconciliation. Non-idempotent writes are never auto-retried.
3. Before dispatch, an intent record is durably written. On restart, any command in
   `DISPATCHED`/`ACCEPTED` without a terminal record moves to `UNKNOWN_OUTCOME`.
4. (principal, target device, idempotency key) identifies a request. Duplicates
   return the existing command; they never dispatch again.
5. Every command — including denials — yields an immutable receipt with its full
   transition history, the policy decision (with rule IDs), adapter results and
   readback evidence.
6. The transition table is data, defined once, and tested exhaustively (every legal
   transition accepted, every illegal one rejected).

Terminal states: `DENIED`, `EXPIRED`, `CANCELLED`, `REJECTED`, `FAILED`,
`VERIFIED`, `VERIFICATION_FAILED`, `UNVERIFIABLE`, `RECONCILED_APPLIED`,
`RECONCILED_NOT_APPLIED`. `UNKNOWN_OUTCOME` is stable but reconcilable.

`UNVERIFIABLE` covers capabilities with no readback (e.g. a fire-and-forget
trigger); the receipt says so instead of pretending verification happened.

## Amendment (M1, 2026-10-23)

Writing the full specification (`spec/lifecycle.md`) surfaced three gaps, now part of
this decision:

- `AWAITING_APPROVAL → CANCELLED`: a requester can withdraw a command that is still
  waiting for approval.
- `DISPATCHED → UNKNOWN_OUTCOME` and `ACCEPTED → UNKNOWN_OUTCOME`: crash recovery
  (rule 3) needs these edges explicitly; previously they existed only in prose.
- Outcome classes are `pending`, `succeeded`, `unverified`, `failed`, `denied`,
  `not_executed`, `uncertain`. `unverified` keeps `UNVERIFIABLE` from being reported
  as success; `not_executed` tells an agent that `EXPIRED`/`CANCELLED` commands never
  reached the device.

## Alternatives considered

- **Simple success/failure.** Hides the exact cases that cause duplicate physical
  effects. Rejected.
- **Automatic retry on timeout.** Safe only for idempotent operations, and even then
  needs readback first. Rejected as a default; retries are an explicit, policy-gated
  reconciliation action.

## Consequences

- Agents get truthful, structured outcomes they can plan around.
- Adapters must implement readback for writable capabilities, or declare them
  unverifiable — which policy can restrict.
- More states means more client handling; mitigated by clear outcome classes in the
  spec (`succeeded`, `failed`, `denied`, `uncertain`).
