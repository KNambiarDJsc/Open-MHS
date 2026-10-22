# Command lifecycle

Decision record: [ADR-0002](../docs/adr/0002-command-lifecycle.md). Reference
implementation: `crates/openmhs-types/src/lifecycle.rs` (the transition table is
defined there once, as data, and tested exhaustively).

## 1. States and transitions

```
CREATED ─► VALIDATING ─┬─► DENIED
                       ├─► AWAITING_APPROVAL ─┬─► APPROVED
                       │                      ├─► DENIED
                       │                      ├─► EXPIRED
                       │                      └─► CANCELLED
                       └─► APPROVED ─┬─► DISPATCHED
                                     ├─► CANCELLED
                                     └─► EXPIRED

DISPATCHED ─┬─► ACCEPTED ─┬─► APPLIED ─┬─► VERIFIED
            │             │            ├─► VERIFICATION_FAILED
            │             │            └─► UNVERIFIABLE
            │             ├─► FAILED
            │             ├─► TIMED_OUT
            │             └─► UNKNOWN_OUTCOME   (crash recovery)
            ├─► REJECTED
            ├─► FAILED
            ├─► TIMED_OUT ─► UNKNOWN_OUTCOME ─┬─► RECONCILED_APPLIED
            └─► UNKNOWN_OUTCOME (crash recovery)└─► RECONCILED_NOT_APPLIED
```

Any transition not shown is illegal and MUST be rejected.

| State | Meaning |
|---|---|
| `CREATED` | Request received and journaled |
| `VALIDATING` | Schema, authorization, constraints and preconditions being checked |
| `DENIED` | Refused by policy or validation. Never sent to the device. |
| `AWAITING_APPROVAL` | Allowed only with human approval, which hasn't happened yet |
| `APPROVED` | Cleared to dispatch |
| `EXPIRED` | Deadline passed before dispatch. Never sent. |
| `CANCELLED` | Withdrawn before dispatch. Never sent. |
| `DISPATCHED` | Intent journaled, handed to the adapter |
| `ACCEPTED` | Device acknowledged receipt |
| `REJECTED` | Device refused the command |
| `FAILED` | Adapter or device reported failure |
| `TIMED_OUT` | No acknowledgement/report within the deadline |
| `APPLIED` | Device reports the change applied |
| `VERIFIED` | Readback confirms the expected state |
| `VERIFICATION_FAILED` | Readback contradicts the expected state |
| `UNVERIFIABLE` | Applied, but no readback is possible (declared `verification: none`, or the readback source was unavailable); reason recorded |
| `UNKNOWN_OUTCOME` | May or may not have taken effect |
| `RECONCILED_APPLIED` / `RECONCILED_NOT_APPLIED` | Resolved by readback after `UNKNOWN_OUTCOME` |

## 2. Outcome classes

Agents usually only need the coarse class:

| Outcome | States | Physical effect |
|---|---|---|
| `pending` | `CREATED` … `APPLIED` (non-terminal, except `UNKNOWN_OUTCOME`) | In progress |
| `succeeded` | `VERIFIED`, `RECONCILED_APPLIED` | Confirmed by readback |
| `unverified` | `UNVERIFIABLE` | Device says yes; not independently confirmed |
| `failed` | `REJECTED`, `FAILED`, `VERIFICATION_FAILED`, `RECONCILED_NOT_APPLIED` | Not as requested (may be partial for `FAILED` after `ACCEPTED`; see receipt) |
| `denied` | `DENIED` | None — never sent |
| `not_executed` | `EXPIRED`, `CANCELLED` | None — never sent |
| `uncertain` | `TIMED_OUT`, `UNKNOWN_OUTCOME` | Unknown |

## 3. Rules

1. **Acknowledgement is not effect.** `ACCEPTED` and `APPLIED` are not success; only
   readback is (`VERIFIED`), or an explicit `UNVERIFIABLE`.
2. **Timeouts are uncertain, not failed.** `TIMED_OUT` MUST move to
   `UNKNOWN_OUTCOME`, never to `FAILED`.
3. **No blind retries.** A command in `UNKNOWN_OUTCOME` MUST NOT be re-dispatched
   automatically. Resolution is by readback only. A client may submit a *new* command
   after reconciliation.
4. **Idempotency.** `(principal, device, idempotency_key)` identifies a request. A
   repeated submission with the same key returns the existing command and receipt and
   MUST NOT dispatch again. Reusing a key with a different request body is an error
   (`command.idempotency.conflict`).
5. **Deadlines.** `deadline_ms` bounds the whole command. If it passes before dispatch,
   the command `EXPIRED`. If it passes after dispatch, `TIMED_OUT`.
6. **Receipts.** Every command, including denials, produces an immutable receipt
   (`schemas/receipt.schema.json`) with the full transition history, the policy decision
   and rule IDs, adapter results and readback evidence.

## 4. Durability and crash recovery

1. `CREATED` and every transition are written to the journal before they take effect.
2. Before calling the adapter, the gateway journals the `DISPATCHED` intent.
3. On restart, any command whose last journaled state is `DISPATCHED` or `ACCEPTED`
   moves to `UNKNOWN_OUTCOME` with detail `gateway restarted during execution`.
4. Commands in `APPLIED` resume verification. Commands in `CREATED`, `VALIDATING`,
   `APPROVED` or `AWAITING_APPROVAL` are re-evaluated against current policy, or
   `EXPIRED` if their deadline passed.

## 5. Reconciliation

For a command in `UNKNOWN_OUTCOME` with readback verification, the gateway (or an
operator, via an explicit request) reads the readback source:

- matches the expectation → `RECONCILED_APPLIED`
- matches the pre-command value, or otherwise contradicts it → `RECONCILED_NOT_APPLIED`
- cannot be read → stays `UNKNOWN_OUTCOME`

Commands with `verification: none` cannot be reconciled automatically; they stay
`UNKNOWN_OUTCOME` until an operator resolves them with a recorded reason.
