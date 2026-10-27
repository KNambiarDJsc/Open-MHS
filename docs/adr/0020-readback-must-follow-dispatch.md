# ADR-0020: Readback must be measured after the command was sent

- Status: Accepted
- Date: 2026-10-28
- Builds on: [ADR-0002](0002-command-lifecycle.md), [ADR-0010](0010-engine-execution-model.md)

## Context

Readback verification (`VERIFIED`) and reconciliation (`RECONCILED_APPLIED` /
`RECONCILED_NOT_APPLIED`) accepted any `GOOD` reading, however old: the engine called
`is_usable(now, None)`, and `None` means no age limit. An adapter that serves a cached or
retained value therefore caused two failures:

1. **False verification.** A value that already matched, measured before the command was
   sent, verified a command that may have done nothing.
2. **False non-application, the dangerous one.** A device applied the command and the
   answer was lost; the only reading on offer was the cached pre-command value. The command
   was reconciled `RECONCILED_NOT_APPLIED`, which tells the caller it may send again, and the
   device applies it twice. This is the duplicate physical effect the lifecycle exists to
   prevent. A third variant: an old value that contradicted the command ended
   `VERIFICATION_FAILED` and blamed a working device.

"A measurement taken before dispatch cannot establish that a later command took effect."

## Decision

A reading counts for verification or reconciliation only if it was **measured at or after
the command's `DISPATCHED` transition**, allowing `readback_skew_ms` (default 0) for the gap
between the device's clock and the gateway's.

- **Verification.** Readings older than that are ignored, whether they match or not. If none
  newer arrives within the verification timeout the command ends `UNVERIFIABLE`, with a
  reason that says every reading was older than the command. (It was reported applied, but
  nothing confirmed it.)
- **Reconciliation.** A reading older than dispatch refuses reconciliation
  (`NotReconcilable`), whatever it says. Because "not applied" is the conclusion that
  invites a resend, a reading that does *not* match must also postdate the moment the
  gateway gave up (`UNKNOWN_OUTCOME`), so one taken while a slow device was still working
  cannot be taken as proof that it never will. A matching reading only needs to postdate
  dispatch.
- **Operator escape.** `commands.resolve` is unchanged: a person who checked the device may
  settle an outcome the system cannot.

### Clock allowance

The reading's timestamp is the device's claim of when it measured. Devices with clocks
behind the gateway's would never verify at 0, so `serve --readback-skew-ms N` (and
`GatewayConfig::readback_skew_ms`) sets how far behind a stamp may be. It widens trust in a
device's clock by exactly N ms; it should be the measured offset, not a large number.
Adapters that stamp readings when they read (Modbus, HTTP, SCPI, the simulators) are
unaffected.

## Consequences

- Adapters that serve cached readings now end `UNVERIFIABLE` instead of `VERIFIED`. That is
  the intended finding; it should become a conformance check (`L2.stale_readback`) so such
  adapters fail the suite rather than only their users.
- A device with a skewed clock needs `--readback-skew-ms`; without it, commands stay
  `UNVERIFIABLE` and reconciliation refuses, which fails safe.
- Not addressed: a command that sets a value the device already had cannot be told apart
  from one that did nothing, whatever the timestamps. Readback cannot prove an effect that
  leaves no difference.

## Tests

`crates/openmhs-engine/tests/stale_readback.rs`: an old contradicting value does not fail
the command; an old matching value does not verify it; reconciliation refuses an old
reading and settles correctly once a current one exists; the clock allowance admits a
reading slightly older than dispatch.
