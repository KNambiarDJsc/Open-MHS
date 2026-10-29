# Asynchronous actions and uncertain outcomes

Phase 5. LabThings-FastAPI already has a proper asynchronous action model: `POST` returns `201` and an
invocation resource that is polled to `completed`, `error` or `cancelled`. This study asks what
happens to that model when OpenMHS supervises it and the network, the adapter, or the gateway fails.
It does **not** claim LabThings lacks asynchronous actions.

All results are from simulators on one machine and can be reproduced:

```sh
labthings-server -c labbench/config.json --port 5100        # the independent example Thing
python research/experiments/async_reliability.py > research/results/async_reliability.json
```

Raw results: [`research/results/async_reliability.json`](results/async_reliability.json) (run at
commit `74bae6b`). Each scenario starts a fresh OpenMHS gateway with a fresh journal, serving the
`LabBench` Thing through the generated adapter and a fault-injecting proxy. **Every OpenMHS claim is
compared with ground truth read directly from the LabThings server**: the carriage's real position,
the number of jobs it actually started, and each invocation's own status.

## The vocabulary

Six different facts are easy to conflate. The adapter keeps them apart:

| Fact | Where it comes from | OpenMHS state |
|---|---|---|
| Request accepted | LabThings `201` with an invocation resource, `status: pending` | `ACCEPTED` |
| Invocation started | invocation `status: running` | still `ACCEPTED` |
| Invocation completed | invocation `status: completed` | `APPLIED` ("the device reports applied") |
| Result observed | a property read after the command | (used by verification) |
| Result verified | the descriptor's readback matches, with the reading taken after dispatch | `VERIFIED` |
| Outcome unresolved | contact lost, or the deadline passed, before a terminal status | `UNKNOWN_OUTCOME` |

`APPLIED` is not success. A `completed` invocation says the action *returned*, not that the world
changed as asked; only `VERIFIED` says that.

## Results

| # | Scenario | OpenMHS | Ground truth | Duplicate? |
|---|---|---|---|---|
| S01 | Accepted, not yet completed (3 s job) | `ACCEPTED` for 3.6 s while the invocation was `running`, then `APPLIED` → `VERIFIED`. **No early success** | position 40, one job | no |
| S02 | Completed | `VERIFIED` (readback 40 matches) | position 40 | no |
| S03 | Action raised an error part-way (stalled motor) | `FAILED`; message states how far the action got is *not* known | stuck at 14 of 60; invocation `error` | no |
| S04 | Cancelled by someone else while OpenMHS waits | `FAILED`, message says cancelled | stopped at 15 of 80; invocation `cancelled` | no |
| S04b | Cancel request on an action that ignores cancellation (direct) | (not OpenMHS) | `DELETE` → **200** "Cancel request sent", invocation still `running`, carriage kept moving 16 → 27 → … → 60 | n/a |
| S05 | Caller's deadline passes before completion | `TIMED_OUT` → `UNKNOWN_OUTCOME` while the carriage was **still moving** (position 20, bench busy). A second command is refused `command.outcome_unresolved`. After the job finished, `reconcile` → `RECONCILED_APPLIED` | position 70, one job | no |
| S06 | Connection to status polling reset after acceptance | `UNKNOWN_OUTCOME`; reconcile → `RECONCILED_APPLIED` | position 55, one job | no |
| S07 | Server answers 404 for the invocation (expired or restarted) | `UNKNOWN_OUTCOME`; reconcile → `RECONCILED_APPLIED` | position 45, one job | no |
| S08 | Status answers, but later than the adapter waits | `UNKNOWN_OUTCOME` although the job finished normally ("false unknown"); reconcile → `RECONCILED_APPLIED` | position 35, one job | no |
| S09 | Adapter process killed mid-action | `UNKNOWN_OUTCOME` ("adapter process lost during execute… not retried"); host restarted it, device back `ONLINE`; reconcile → `RECONCILED_APPLIED` | position 75, one job | no |
| S10 | Gateway killed mid-action, then restarted | client saw the connection drop; after restart the command is `UNKNOWN_OUTCOME` ("gateway restarted during execution; not retried"); reconcile → `RECONCILED_APPLIED` | one job started by that command; **nothing re-sent** | no |
| S11 | Duplicate requests | same idempotency key → one job; different key while unresolved → `DENIED command.outcome_unresolved`, one job in total. **Direct LabThings: two identical POSTs ran two jobs** | | only in the direct baseline |
| S12 | Readback served stale by an intermediary, `stamp=received` | `VERIFICATION_FAILED` ("readback 0 does not match 50") though the stage reached 50. **Blames a working device** | position 50 | no |
| S12-date | same, `stamp=date_header`, `readback_skew_ms=1000` | `UNVERIFIABLE` ("readings were older than the command"): correct | position 50 | no |
| S12b | Stale reading used to **reconcile**, `stamp=received` | **`RECONCILED_NOT_APPLIED` while the stage was at 50.** This tells the caller it may safely send again: a duplicate waiting to happen | position 50 | **risk** |
| S12b-date | same, `stamp=date_header`, `readback_skew_ms=1000` | reconcile **refused**: "the only reading… was measured at …, older than the command (given up on)" | position 50 | no |
| S12c | Normal uncached move with `stamp=date_header` | `VERIFIED` (the mitigation costs nothing in the normal case) | position 50 | no |

Across S05–S10 (six independent ways to lose contact after acceptance) the result was the same:
**exactly one job started, and every unresolved command reconciled correctly** by readback once the
work finished. No scenario resent an action.

## Findings

### 1. Acceptance is not completion, and OpenMHS kept them apart
The adapter reports `ACCEPTED` on the `201`, then follows the invocation. Because LabThings can
return `201` with the action already finished, the order is always the same, and no state implied
completion early (S01).

### 2. A bug the first run found in my own adapter
The first end-to-end command failed in 0.2 s, while the job was still running. LabThings returns the
invocation `href` as an absolute URL built from the request's `Host`; behind a proxy it pointed past
the proxy, my adapter's cooperative network-permission check refused to poll it, and the refusal was
raised as `not_sent`: **"nothing reached the device" for an action that had already started.** That
is the worst kind of misclassification. Two fixes, now covered by the scenarios above:
- the adapter polls only the *path* the server gave, under the origin it was configured with (a
  server-supplied URL is untrusted data);
- **after acceptance, no error is ever reported as `not_sent`**: any failure becomes an unknown
  outcome.

### 3. OpenMHS's ledger did not keep the invocation id: it does now
Before this study, acceptance was a bare event (`ProgressEvent::Accepted`); the journal could not
hold what the device called the work. The id survived only if an adapter happened to put it in an
error message, and not at all after an adapter crash or a gateway restart.

*Change made:* an optional, opaque `external_ref` on the acceptance report
(`progress.accepted(external_ref=…)` in the SDK; `adapter_result.external_ref` in the receipt;
validated ≤ 256 characters, no control characters, else dropped; never used to decide anything).
In S05, S09 and S10 the receipt now carries `labthings:<invocation id>`, **including the case where
the gateway itself was killed and recovered (S10)**, because the id was journaled at acceptance,
before the crash. Covered by a host test.

*What it does not do:* OpenMHS still reconciles by **readback**, never by asking LabThings for the
invocation's status. The id lets a person (or a later feature) ask. That later feature is
discussed in [UPSTREAM_OPPORTUNITIES.md](UPSTREAM_OPPORTUNITIES.md).

### 4. "Cancelled" is not "stopped"
S04b: `DELETE` on an action that never checks for cancellation returns **200 "Cancel request sent"**
while the invocation stays `running` and the carriage runs to its end. LabThings' own docs call this
"Cancel request sent"; the lesson is for integrators. The adapter therefore never cancels on its
own, not even at a deadline (S05: the job was left to finish, and the error message says
"it was NOT cancelled and may still be running"). OpenMHS also does not expose cancellation for
in-flight device commands.

When an invocation *is* cancelled by someone else (S04), the command ends `FAILED`, and the message
says how far the action got is not stated. The carriage was physically left at 15 of 80.

### 5. The cost of never assuming: false unknowns
S08 shows the price of the conservative policy. The job completed normally; only the status
endpoint was slow, so OpenMHS reported `UNKNOWN_OUTCOME` and required a reconcile. That is the
intended trade (never claim success you did not see), but a deployment with a slow status endpoint
will see operator work. Raising the adapter's timeout trades this against how long a lost network
stays unnoticed.

### 6. The important weakness: a timestamp-less read breaks the stale-reading rule
ADR-0020 says a reading counts for verification or reconciliation only if it was measured after the
command was sent. LabThings property reads return a bare value. The adapter can only stamp "when I
received it", so an intermediary that replays an old response *looks* fresh:

- verification fails with the wrong reason (S12): a working device is blamed;
- **reconciliation concludes `RECONCILED_NOT_APPLIED` (S12b): the unsafe conclusion ADR-0020 exists to prevent.**

*Mitigation tested:* `stamp: date_header` uses the server's HTTP `Date` header as the reading time.
A cache or proxy replays the original `Date`, so the reading is correctly seen as old (S12-date,
S12b-date). It needs `--readback-skew-ms 1000` (the header has one-second resolution) and adds no
cost when nothing is cached (S12c).

*Honest limits of the mitigation:*
- It cannot detect staleness shorter than about one second plus the allowance. A first attempt in
  this study took the cache snapshot less than a second before the command, inside that window, and
  the mitigation (correctly, by its own rule) did not trigger. The scenarios above use a snapshot
  2.5 s old.
- It trusts the server's clock, and a server without a `Date` header gets no protection.
- It protects against intermediaries replaying a *response*. It cannot detect a Thing that serves
  its own cached value.
- A property that carried a real measurement time (a field in the value, or a header) would be
  better. LabThings has no such convention today.

### 7. Duplicate protection is OpenMHS's, not LabThings'
Two identical POSTs run twice in LabThings (S11, direct): it has no notion of an idempotent
request, nor should it. Through OpenMHS the same idempotency key produced one job; a different key
while an outcome was unresolved was refused before anything was sent.

## What this does not show

- Only the paths exercised were tested. The 14 scenarios are not a complete failure model.
- Everything ran on loopback with a simulated carriage. Real networks have partial failures this
  harness does not produce (for example a half-open TCP connection).
- The "ground truth" is the LabThings server's own account. If a real device disagreed with its
  controller, none of this would detect it; only an independent measurement would.
- One machine, one OS, a debug build, small sample (one run per scenario; the behaviour was
  consistent across the three runs made while developing the harness).
- `idempotent: false` for `move_to` is a declaration chosen to exercise OpenMHS's refusal logic; an
  absolute move is harmless to repeat.
- No physical hardware was involved.
