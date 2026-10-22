# ADR-0010: Engine execution model

- Status: Accepted
- Date: 2026-10-23

## Context

M2 implements the command engine. Several runtime choices affect correctness under
concurrency and failure.

## Decision

1. **Per-device serialization.** Writes and invokes on one device run one at a time:
   the engine holds a per-device async lock from the stateful checks through
   verification. Rate limits, slew limits and preconditions are therefore never
   evaluated against a state another in-flight command is about to change. Different
   devices run in parallel. Approvals release the lock while waiting and re-acquire it
   (and re-run all checks) when approved.
2. **Fresh reads for decisions.** Stateful checks use readings taken during the check,
   never cached values. A device that cannot be read fails closed.
3. **Adapters run in their own task.** `execute` and `read` are spawned, so an adapter
   panic becomes a classified error (`adapter.internal` → `UNKNOWN_OUTCOME`), and a hung
   adapter is abandoned at the deadline rather than blocking the engine.
4. **Journal before effect.** Each transition, with its audit record, is committed to
   SQLite (`synchronous=FULL`, WAL) before the engine proceeds — in particular before
   calling the adapter (`DISPATCHED`).
5. **Reads are not commands.** They are authorized and audited, but create no receipt.
6. **Time comes from an injected clock.** The production clock follows Tokio's timer, so
   tests pause Tokio time and run timeout, slew and freshness scenarios deterministically
   and instantly.

## Alternatives considered

- **Optimistic concurrency per capability.** More parallelism, but preconditions often
  span capabilities (e.g. "output enabled" depends on "protection tripped"), so a
  per-capability lock is not enough. Rejected for v0.1.
- **Async SQLite driver.** The journal's critical sections are short; a blocking
  `rusqlite` call behind a mutex is simpler and keeps transactions explicit. Revisit if
  profiling shows contention.

## Consequences

- Throughput per device is one command at a time — appropriate for supervisory control,
  wrong for high-rate streaming (which is telemetry, not commands).
- A hung adapter task is abandoned, not killed; adapters must not hold resources that
  outlive a dropped future.
- Rate-limit history is in memory and resets on restart (documented in
  `spec/lifecycle.md` §4).
