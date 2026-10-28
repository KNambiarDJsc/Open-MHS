# ADR-0023: The state fabric: events, watches, and the `subscribe` operation

- Status: Accepted
- Date: 2026-10-28
- Builds on: [ADR-0010](0010-engine-execution-model.md), [ADR-0017](0017-identity-and-trust.md),
  [ADR-0019](0019-device-availability.md), [ADR-0020](0020-readback-must-follow-dispatch.md)

## Context

An agent coordinating several devices needs to see what changes without asking for every
value in turn: a measurement drifting during a test, a device going offline, its own
command reaching `VERIFIED`. The gateway had no way to say any of that. A client could only
poll `state.read` (one audited record per call) and `commands.get`.

The specification already reserved the means. Descriptors declare `subscribe` on properties
that can be streamed (`power.output.voltage.measured`, `temperature.ambient`), and scopes,
grants and adapter manifests accept `subscribe`. Nothing implemented it.

## Decision

### Events

The engine publishes an event at the three points everything already passes through:

- `observation`: every successful adapter read (`read_device`), with the reading exactly as
  the adapter returned it (value, measurement timestamp, quality, source), why it was taken
  (`read`, `check`, `verify`, `reconcile`, `preflight`, `watch`), and the command it was
  taken for, for verification and reconciliation;
- `availability`: every availability change (ADR-0019);
- `command`: every command state transition.

Events go into one bounded in-memory ring (8 192 by default, `EngineOptions::event_capacity`)
with a sequence number per process and a stream id. `events.poll` returns events after a
cursor, filtered by device pattern and kind, and waits up to 30 s for one if there are none.

**Backpressure is pull plus explicit loss.** The gateway never blocks a publisher, never
buffers without bound, and never drops silently: a consumer whose cursor fell out of the ring
gets `missed`, the number of events it can no longer receive. A cursor from another process
gets `reset`.

### Who sees what

Decided per event at delivery, with the same functions as everything else:

- An observation needs the capability to be a property whose descriptor declares
  `subscribe`, the credential's scope to cover `subscribe` on it, and a grant to allow it.
  A `read` grant is not enough. Streaming is a separate, explicit permission the operator
  gives, as the specification intended. A setpoint's readback (read and write only) is never
  streamed. Its result is in the command's receipt and command events.
- An availability event needs the credential's scope to reach the device.
- A command event goes to the command's principal, and to the operator.

### Watches

`watches.create` samples one capability for one principal every `interval_ms` (at least
200) for `ttl_ms` (default 5 min, at most 1 h). Samples go through `read_device` like any
read and appear as `observation` events with `cause: watch`.

- **Bounds.** At most 16 watches per principal and 256 in all. A second watch on the same
  capability by the same principal replaces the first.
- **Authorization.** Checked at creation and again before every sample, so a revoked grant
  ends the watch at its next tick. Revoking or rotating the credential ends its watches at
  once.
- **Audit.** The creation and the end are audited (`watch.created`, `watch.ended` with the
  reason). Samples are not audited one by one.
- **Lifetime.** Watches are not persisted. A restart ends them, and clients create them
  again.

### Why samples are not audited individually

A one-second watch would otherwise write 86 400 audit records a day per principal. That is
exactly the unbounded growth the storage RFC is about. What the audit needs to answer is
who was allowed to stream what, from when to when, and with which credential. The
`watch.created` and `watch.ended` records answer that. Every other read path is unchanged:
`state.read` is still audited per call, and commands still decide on fresh reads of their
own. Whether routine telemetry should also be recorded is the RFC's `TELEMETRY` class,
still under review. **This is a deliberate change in what is audited, and it is recorded
here for review.**

### Events are not truth

An observation is a notification of what the adapter said, when it said it. It is never
used by the engine to decide anything: live checks, verification and reconciliation take
their own fresh readings, under ADR-0020's rule that a reading must postdate the command.
Model-facing guidance says the same. Check `measured_at` and `quality`, `read_state`
before deciding, and treat `missed > 0` as "you no longer know the current value".

## Alternatives considered

- **Server push (WebSocket or SSE).** Rejected for now. The API is JSON-RPC over loopback
  HTTP, and long polling keeps it so with one more method. Push can come later over the
  same events, with the same rules.
- **Adapter-level subscriptions** (an MQTT topic, an OPC UA monitored item). These are the
  right source for event-kind capabilities and for fast telemetry, but they need an adapter
  trait change. They are deferred. Watches sample through `read`, so they work with every
  adapter today. Event-kind capabilities (`subscribe` only, no `read`) cannot be watched yet.
- **Persisting events.** Rejected. The journal already holds what must survive, and
  persisting telemetry is the RFC's question.

## Consequences

- Operators grant `subscribe` explicitly: `grants add --ops subscribe`, plus a credential
  scope with `subscribe`.
- Memory is bounded by the ring and the watch limits. Device traffic grows by one read per
  watch per interval. There is no deduplication across principals watching the same
  capability.
- `missed` reveals how many events passed while a consumer was behind, including events it
  could not have seen. It reveals volume, not content.

## Tests

- **Engine** (`crates/openmhs-engine/tests/events.rs`):
  - order and command transitions;
  - setpoint readbacks withheld;
  - subscribe needed for observations;
  - commands visible only to their principal (all to the operator);
  - `missed` counted exactly under overflow, with paging that neither repeats nor skips;
  - a poll woken by a publish;
  - reset after a restart;
  - availability streamed;
  - a watch sampling five times in 5.5 s, then expiring with exactly two audit records;
  - watch refusals: no grant, not streamable, too fast, too long;
  - a revoked grant ending a watch;
  - replacement and owner-only cancel.
- **MCP** (`a_model_watches_a_measurement_and_follows_its_command`).
- **CLI** (`find_and_watch_a_streamable_measurement`).
- **Python** (`StateFabric`).
