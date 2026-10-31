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
| `UNVERIFIABLE` | Applied, but no readback is possible (declared `verification: none`, the readback source was unavailable, or every reading was older than the command, [ADR-0020](../docs/adr/0020-readback-must-follow-dispatch.md)); reason recorded |
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
   automatically. It is resolved by readback, or by an operator (§5). Until it is, a new
   write or invoke to the same capability is `DENIED` with
   `command.outcome_unresolved`, unless the capability is declared `idempotent` (doing
   it twice is the same as once) or `stop`. Any write or invoke on another device that
   shares a declared resource with the unresolved command is also denied, even if that
   other capability is idempotent. Checks run under the device and resource locks before
   device traffic.
4. **Idempotency.** `(principal, device, idempotency_key)` identifies a request. A
   repeated submission with the same key returns the existing command and receipt and
   MUST NOT dispatch again. Reusing a key with a different request body is an error
   (`command.idempotency.conflict`).
5. **Deadlines.** `deadline_ms` (default 60 000 ms) bounds the command up to the
   device's report. If it passes before dispatch, the command `EXPIRED`. If it passes
   after dispatch, `TIMED_OUT`. A capability's `timing.timeout_ms`, if shorter, bounds
   execution instead. Verification after `APPLIED` has its own `timeout_ms`
   (default 2 000 ms) from the descriptor.
6. **Receipts.** Every command, including denials, produces an immutable receipt
   (`schemas/receipt.schema.json`) with the full transition history, the policy decision
   and rule IDs, adapter results and readback evidence.
7. **Uncertain adapter failures.** A disconnect, malformed response or adapter crash
   after dispatch moves the command straight to `UNKNOWN_OUTCOME` (the
   `DISPATCHED`/`ACCEPTED → UNKNOWN_OUTCOME` edges). Only a timeout passes through
   `TIMED_OUT`. See the adapter codes in [errors.md](errors.md).
8. **Reads are not commands.** A read cannot change the device, so it does not create a
   command or a receipt. It is still authorized (`read` operation) and recorded in the
   audit log, including denied reads.
9. **One state-changing command per device at a time.** The gateway serializes writes and
   invokes per device, from the stateful checks through verification, so rate limits,
   slew limits and preconditions are evaluated against settled state
   ([ADR-0010](../docs/adr/0010-engine-execution-model.md)). The one exception is a
   capability declared `stop` ([descriptor.md](descriptor.md) §2): it does not wait for
   the command running on the device, so it can interrupt it, and it is not blocked by
   another holder's lease ([ADR-0018](../docs/adr/0018-orchestration.md)). The command
   it interrupts ends `FAILED` if the device reports it did not complete.

## 3a. Validation order

1. Command schema. Failure → error to the caller; no command is created (audited as
   `request.rejected`).
2. Device and capability exist.
3. Authorization. Denied requests cause **no device traffic at all**.
4. Stateless checks: operation, types, units, ranges, parameters. Failures cause no
   device traffic.
5. Stateful checks, under the device lock, against readings taken **now**: rate limit,
   slew limit, preconditions and freshness. Readings that fail, are stale, or are not
   `GOOD` count as missing, and missing state denies.

## 4. Durability and crash recovery

1. `CREATED` and every transition are written to the journal before they take effect.
2. Before calling the adapter, the gateway journals the `DISPATCHED` intent.
3. On restart, any command whose last journaled state is `DISPATCHED`, `ACCEPTED` or
   `TIMED_OUT` moves to `UNKNOWN_OUTCOME` with detail `gateway restarted during
   execution`.
4. Commands in `APPLIED` resume verification once their device is registered again.
5. Commands that never reached the device are closed, never resumed: `CREATED` and
   `VALIDATING` become `DENIED` (`gateway.restarted`; the client may resubmit), and
   `APPROVED` becomes `EXPIRED`. Commands in `AWAITING_APPROVAL` keep waiting until
   approved, cancelled, or past their deadline.
6. Rate- and slew-limit history is kept in memory and starts empty after a restart.
7. If the journal refuses a write while a command is being dispatched or after (an I/O
   error, a full disk), the gateway does not end the call early: the device stays locked
   until the adapter call ends. It then settles the command on record if it still can:
   one that was never sent becomes `EXPIRED`; one that may have reached the device
   becomes `UNKNOWN_OUTCOME` (detail `the outcome could not be recorded`), never resent.
   If not even that can be written, the command stays `DISPATCHED`, `ACCEPTED` or
   `TIMED_OUT` on record, and is held exactly like `UNKNOWN_OUTCOME` (rule 3) until a
   restart's recovery settles it. Tested by `crates/openmhs-engine/tests/persistence_adversary.rs`
   ([PERSISTENCE_ADVERSARY.md](../docs/architecture/PERSISTENCE_ADVERSARY.md)).
8. The gateway trusts the journal it is given. A journal rolled back to an earlier copy
   (a restored backup, a lost write-ahead log, a disk that acknowledged writes it did not
   make) is not detected at start; recovery then believes the older record. Only an audit
   checkpoint kept elsewhere reveals it (`openmhs audit verify-file --checkpoint`).

## 5. Reconciliation

For a command in `UNKNOWN_OUTCOME` with readback verification, the gateway (or an
operator, via an explicit request) reads the readback source:

- matches the expectation → `RECONCILED_APPLIED`
- matches the pre-command value, or otherwise contradicts it → `RECONCILED_NOT_APPLIED`
- cannot be read → stays `UNKNOWN_OUTCOME`

Only a reading **measured after the command was sent** counts, here and for `VERIFIED` /
`VERIFICATION_FAILED` ([ADR-0020](../docs/adr/0020-readback-must-follow-dispatch.md)); a
cached or retained value from before cannot show that a later command took effect. The
reading's timestamp is compared with the `DISPATCHED` transition, allowing the operator's
configured clock allowance (`readback_skew_ms`, default 0). A reading that does not match
must also postdate the moment the gateway gave up, because "not applied" invites a resend
and a slow device may not have finished. A reading older than that leaves the command in
`UNKNOWN_OUTCOME` (reconciliation is refused), never concludes it.

Commands with `verification: none` cannot be reconciled automatically; they stay
`UNKNOWN_OUTCOME` until an operator resolves them with a recorded reason
(`commands.resolve`): `RECONCILED_APPLIED` or `RECONCILED_NOT_APPLIED`, with the
transition detail `resolved by <principal>: <reason>`. An operator may also resolve a
command whose readback cannot be taken. Agent credentials cannot resolve, and resolving
never contacts the device.

### What the gateway reports after a timeout or cancellation

| Situation | Reported | Sent again? |
|---|---|---|
| Cancelled before dispatch | `CANCELLED`, never sent | — |
| Never left the gateway (link down, instrument still busy with an earlier request) | `FAILED` with `adapter.not_sent` | No |
| Sent, no answer within the deadline | `TIMED_OUT` → `UNKNOWN_OUTCOME` (`adapter.timeout`) | No; a retry is refused until resolved (rule 3) |
| Sent, link lost or answer unreadable | `UNKNOWN_OUTCOME` | No |
| Device refused it | `REJECTED`, with the device's error | — |
| Gateway restarted mid-command | `UNKNOWN_OUTCOME` (§4) | No |
| Journal write failed mid-command (I/O error, disk full) | `UNKNOWN_OUTCOME`, or left in flight on record and held (§4 rule 7) | No |

A caller that stops waiting (a client disconnect, a cancelled request) does not cut the
command short: it runs to one of these states, and its receipt is fetched later.
