# ADR-0021: Journal quota and critical reserve

- Status: Accepted (minimum protection only; segments, archives and retention remain a
  draft: [STORAGE_AND_SCALING_RFC.md](../strategy/STORAGE_AND_SCALING_RFC.md))
- Date: 2026-10-28
- Builds on: [ADR-0002](0002-command-lifecycle.md), [ADR-0010](0010-engine-execution-model.md),
  [ADR-0019](0019-device-availability.md)

## Context

The journal grows without bound (gap H8), so a long-running gateway eventually fills its
disk. What matters first is not tidiness but what happens then. The engine already journals
a command's intent and its `DISPATCHED` transition before it calls the adapter, and a
journal error stops the command, so a full journal already means nothing new is sent. But
that failure arrives as a raw storage error in the middle of a command, uses up the last
bytes with routine records, and leaves no room for the records that matter most: a stop,
the `UNKNOWN_OUTCOME` transitions of crash recovery, a person's resolution.

## Decision

An operator can set a **hard limit** on the journal's size and a **reserve** below it.

- The hard limit is enforced by SQLite (`max_page_count`): a write that would grow the file
  past it fails, whatever it is.
- When the journal is within `reserve` of the hard limit (the *soft limit*), the gateway
  refuses new work that is not critical:
  - writes and invokes that are not stops are denied before anything is sent, with
    `storage.exhausted`, at preflight, submit and approval;
  - audited reads and preflights are refused with the same code, so telemetry cannot eat the
    reserve.
- Only the following may use the reserve: stops, denials and cancellations of commands
  already held, reconciliation and resolution, lease release, and crash recovery. Approving
  a held command dispatches it, so it is refused like a new one.
- If even the reserve is gone, everything fails closed, including stops: a stop that cannot
  be journaled is not sent. The equipment's own emergency stop is authoritative; this is
  one reason a gateway stop is not a safety function.
- The default is no limit, as before. `openmhs serve --journal-max-mib N` sets the hard
  limit and `--journal-reserve-mib M` the reserve (default 256, the starting hypothesis in
  the RFC; it is rejected if not smaller than the limit). They are a floor to test against,
  not a sizing recommendation: the right numbers come from measured write rates and the
  offline period the deployment must survive.

## What this does not do

It does not delete, archive or compact anything, and it does not make the journal smaller.
It decides what happens at the limit. Reaching the soft limit needs an operator: archive
the journal by hand (stop the gateway, copy `journal.sqlite`), raise the limit, or wait for
the retention work in the RFC. It does not protect against a full volume the journal does
not control, only against growing past its own limit. The size is read from SQLite page
counts, so it includes free pages and is slightly above the logical data.

## Tests

`crates/openmhs-engine/tests/storage.rs`: at the soft limit a write is denied before the
device is touched, a preflight says so without writing an audit record, an audited read is
refused, a stop is still accepted, and raising the limit restores service. At the hard limit
every command that reached the device has its `DISPATCHED` transition in the journal (the
engine never sends what it could not record). Limits that cannot work (a reserve not smaller
than the limit, a limit below the journal's size) are refused.

Not tested here: restart after exhaustion (the journal and recovery are unchanged by this
ADR; recovery's own tests cover it) and exhaustion of a volume the journal does not control.
