# Distributed migration plan

Keep the embedded gateway as the reference execution mode. A distributed control plane is
not a prerequisite for running a local adapter or lab integration.

## Deployment levels

1. **Embedded:** one process, SQLite journal, local policy and adapters. Continue to support
   this mode independently.
2. **Single lab:** multiple local adapters with per-device serialization, operator recovery
   and offline evidence export. Add no central database requirement.
3. **Multi-site:** only after the command protocol and recovery tests are stable, add central
   admission, a transactional outbox, durable transport and site gateways. The edge remains
   final policy enforcement and owns locally observed device history.
4. **Industrial/research production:** require threat review, backup/restore drills, key
   rotation, signed policy distribution, external audit checkpoints and supervised physical
   validation for each device class.

## Staged work

### Necessary now

- Correct agent-facing discovery and error behavior.
- Principal-scoped permission introspection that does not bypass dispatch authorization.
- Deterministic command lifecycle, idempotency, uncertainty and restart recovery tests.
- Receipt/audit integrity scope and explicit evidence limitations.

### Necessary before multiple writers or sites

- Resource identity shared by all gateways that can control a device.
- Ownership epochs enforced at the final device boundary where supported; where not
  supported, single-writer operations and operator-controlled failover only.
- Stable command envelope and version/migration rules.
- Central command ledger/outbox plus edge inbox deduplication and replay-safe status sync.
- Network partition and stale-owner fault tests.

### Not justified yet

Installing PostgreSQL, NATS, Temporal and Kubernetes together; rewriting the current engine
into services; or promising exactly-once physical execution from at-least-once messaging.
Choose a transport only after workload, offline behavior and operational ownership are
measured. Native controllers remain responsible for timing-sensitive control loops.

## Migration gates

1. Land and verify the agent-facing correctness milestone.
2. Add crash-window, property-based and storage-exhaustion tests to the standalone engine.
3. Receipt binding and offline checkpoint verification are implemented in
   [AUDIT_EVIDENCE.md](AUDIT_EVIDENCE.md); automate independent checkpoint retention next.
4. Add resource ownership semantics and test stale owners; do not call a lease a fence unless
   the device/controller rejects stale epochs.
5. Prototype one edge-to-control-plane path with an outbox/inbox and simulator-only chaos.
6. Compare recovery behavior against standalone mode and preserve a local-only run path.
7. Add deployment automation and physical validation only with explicit device-specific
   review; publish no safety or compatibility claim based solely on simulator results.
