# RFC: Bounded storage, audit lifecycle and fleet-scale scheduling

- Status: **Draft for review.** Nothing here is implemented except where §9 says so.
  Destructive audit rotation must not be built until this RFC and its migration are
  reviewed.
- Date: 2026-10-28
- Related: [ARCHITECTURE_GAP_ANALYSIS.md](ARCHITECTURE_GAP_ANALYSIS.md) H8 and H9,
  [ADR-0019](../adr/0019-device-availability.md), [M8 threat model](../security/M8_THREAT_MODEL.md)

## 1. Problem

The gateway keeps everything in one SQLite file (`journal.sqlite`): commands with their
receipts, approvals, leases, plans, and one hash-chained audit table. The audit chain is
verified from its first record (`Journal::verify_audit`) and nothing is ever deleted.

Every allowed or refused read writes an audit record (`Engine::read`), as does every
command preflight. So storage grows with *observation rate*, not with *how much happened*:

| Load | Records per day | At 500 bytes each (illustrative, not measured) |
|---|---|---|
| 1 agent reading 1 value per second | 86,400 | ≈ 43 MB |
| 100 such readers | 8.64 M | ≈ 4.3 GB |
| 1,000 | 86.4 M | ≈ 43 GB |

Indexes and SQLite overhead come on top. Record sizes will be measured (§8) before any
quota is set; these figures only show the shape of the problem.

What does not have this problem: commands (bounded by how often anything is changed),
approvals, leases, plans, and availability changes (only transitions are recorded,
ADR-0019).

## 2. Principles

1. **Physically consequential records are never sampled, aggregated or dropped.**
2. **Security evidence stays individual** where the threat model needs it: a refused
   attempt is an event, not a count.
3. **Unknown event kinds are treated conservatively:** retained in full until classified.
4. **If intent cannot be recorded, nothing is dispatched.** This is already true (§5).
5. **A stop must still be recordable when storage is otherwise exhausted.**
6. **Retention never weakens what verification can prove about what is kept.** A
   checkpoint proves continuity from a trusted anchor; it cannot prove the content of
   deleted records. Deleting means archiving first.
7. Local-first: one gateway must not depend on any remote service to authorize or
   execute a command.

## 3. Event classes

Every audit kind gets exactly one class, declared in code next to where it is written, and
checked by a test that fails on an unclassified kind.

| Class | Kinds today | Integrity | Retention | Aggregation |
|---|---|---|---|---|
| `CRITICAL` | `command.created`, `command.transition`, `approval.*`, `plan.*`, `lease.*`, `gateway.recovered`, `sim.fault` | Hash chain, segment checkpoints | Configured minimum (default: forever locally until archived) | Never |
| `SECURITY` | `read.denied`, `request.rejected`, `credential.rejected`, `grant.*`, `policy.updated`, `token.*`, `topology.set` | Hash chain, segment checkpoints | Configured minimum; legal hold possible | Never |
| `OPERATIONAL` | `gateway.started`, `gateway.stopped`, `device.registered`, `device.availability`, `command.preflight`, `plan.preflight` | Hash chain | Shorter default (e.g. 90 days) | Never; availability is already change-only |
| `TELEMETRY` | `read` (an allowed, successful read) | Counted and summarized in the chain; raw values in a separate store | Raw: short (e.g. 7 days); summaries: as `OPERATIONAL` | Yes, explicitly configured |

`read` is the only kind proposed for aggregation, and only allowed reads. A read whose
value was the basis for a command is already captured in that command's receipt (live
checks and readback), so the receipt, not the read log, is the evidence for the action.

The unclassified default is `CRITICAL`.

## 4. Storage layout

Three logical stores in one gateway home, each with its own quota:

```text
journal.sqlite        commands, approvals, leases, plans; the CRITICAL/SECURITY/OPERATIONAL chain
telemetry.sqlite      raw allowed reads: (device, capability, principal, at, value, quality)
audit/segments/       closed audit segments, append-only, one file per segment
audit/manifest.json   segment index: seq range, class counts, first/last hash, checkpoint, archive state
```

Telemetry in its own file means its quota can fill and roll over without touching the
journal, and its write rate cannot slow command journaling (separate SQLite write lock).

### Read summaries

For each (principal, device, capability) per interval (default 60 s), one `read.summary`
record goes into the audit chain: count, first and last timestamp, first and last value,
min and max for quantities, and how many were dropped from the raw store (if any). So the
chain still says who read what, when and how often; the raw values live in telemetry
storage for their retention window.

## 5. Failure behaviour

| Condition | Behaviour |
|---|---|
| Normal | Everything persisted as classified |
| Telemetry store reaches its soft quota | Alert (`gateway.info`, log), drop oldest raw telemetry first |
| Telemetry store reaches its hard quota | Stop writing raw reads; summaries continue and record `dropped` counts. Loss is explicit, never silent |
| Audit archive target unavailable | Keep closed segments locally within the reserved budget; alert; never delete an unarchived segment |
| Journal cannot write (disk full, I/O error) | **Refuse new writes and invokes** before dispatch. Already the case: `dispatch` journals the expectation and the `DISPATCHED` transition before it spawns the adapter call, and errors propagate (`engine.rs`, `dispatch`) |
| Journal cannot write, and a stop is requested | Must still work: see reserve below |
| Restart after exhaustion | Normal recovery: in-flight commands become `UNKNOWN_OUTCOME`, nothing is re-sent |

**Reserve.** Today a stop is a journaled command, so a full disk refuses stops too. The
gateway should keep a reserve that only `CRITICAL` writes may use: a preallocated file in
the home (e.g. 64 MB) released, with an alert, when the journal reports `SQLITE_FULL`, so
that stops, their transitions and recovery records still commit. Separately, the hardware
emergency stop remains authoritative and independent of the gateway.

## 6. Audit segments and checkpoints

- The audit chain is split into segments by size or time. A segment closes with a
  `audit.checkpoint` record holding the segment's last hash and record count; the next
  segment's first record chains from that checkpoint.
- A checkpoint can be signed (Ed25519, the key machinery of M8) or anchored externally
  (written to a second machine). Unsigned checkpoints still detect accidental gaps;
  signed ones detect a writer of the home rewriting history.
- `manifest.json` records each segment's sequence range, class counts, first and last hash,
  checkpoint, file hash, and archive state (`local`, `archived(at, location, hash)`).
- **Deletion only after archive acknowledgement**, and never inside a retention minimum or
  a legal hold.
- `openmhs audit verify` verifies the live segment from the last retained checkpoint;
  `--all` verifies every local and reachable archived segment; both report gaps, replaced
  records, and manifest entries whose archives are missing.

What survives deletion of an archived-then-deleted segment locally: its manifest entry and
checkpoint. Verification can then prove the retained records continue from that point, not
what the deleted ones said; the archive must be kept to prove that.

**Migration.** Existing journals are one unbounded chain. The first start on the new
version closes it as segment 0 with a checkpoint, in place. Nothing is deleted by the
migration.

## 7. Fleet-scale health scheduling (extends ADR-0019)

What exists after ADR-0019: periodic sweeps, devices not checked recently and not busy,
at most 16 probes at once, a sweep finishes before the next sleep, one probe per device
per sweep.

Next, in order, each behind measurements from §8:

1. **Per-device schedule instead of sweeps.** A min-heap of next-probe times. Online and
   active devices: base interval. Offline: exponential backoff with full jitter, capped
   (e.g. 5 s → 10 → 20 → … → 5 min). Reached by a command or read: rescheduled, since
   passive observation already counts as a check.
2. **Budgets.** A global concurrency limit plus a per-adapter limit (a Modbus gateway or a
   serial bus is one link for many devices) and a per-adapter rate limit.
3. **Hysteresis.** `ONLINE → OFFLINE` after N consecutive failures (a command's disconnect
   counts immediately); `OFFLINE → RECOVERING → ONLINE` after M consecutive successes. A
   flapping device then writes a bounded number of availability records.
4. **Priority.** A probe never waits on a device lock and never delays a command; commands
   to an `OFFLINE` device still probe on demand.
5. **Adapter supervision.** The out-of-process host already restarts adapters; add a
   restart budget and a `QUARANTINED` state after repeated crashes.

## 8. Measurements to take before choosing numbers

A benchmark harness (`openmhs bench`, see [HARDWAREBENCH_RESEARCH_PLAN.md](HARDWAREBENCH_RESEARCH_PLAN.md))
with simulated fleets of 10, 100, 1,000 and 10,000 devices, mostly-online and
mostly-offline, measuring: bytes per audit record by kind, journal growth per hour under a
fixed read and command mix, CPU and resident memory, probe scheduling delay, sweep lag,
command latency p50/p95/p99 with and without probes running, and recovery time after a
restart with N in-flight commands. Quotas, intervals and limits are set from those numbers.

## 9. What is implemented now, and what is not

| Item | State |
|---|---|
| Fail-closed dispatch when the journal cannot write | Existing behaviour, confirmed by reading `dispatch` |
| Availability changes recorded only on transition | Implemented (ADR-0019) |
| Bounded probe concurrency, no overlapping sweeps | Implemented (ADR-0019), tested at 1,000 devices (`crates/openmhs-engine/tests/availability.rs`) |
| Event classes, telemetry store, read summaries | Not implemented |
| Segments, checkpoints, manifest, archive, retention | Not implemented; **no destructive rotation before review** |
| Reserve for critical writes | Not implemented |
| Per-device scheduler, backoff, budgets, hysteresis | Not implemented |
| Benchmarks | Not implemented |

## 10. Provisional decisions (2026-10-28)

The four review questions were answered provisionally by the maintainer. They are inputs
to the implementation, not final policy.

1. **Reads as evidence.** Preserve security-sensitive, privileged and experiment-critical
   reads individually, in addition to every physical-command record. Only explicitly
   classified routine telemetry may be aggregated. `TELEMETRY` aggregation is therefore
   per-class opt-in, and the default class of a read is not `TELEMETRY` until configured.
2. **Retention.** Configurable per event class. No silent deletion of critical command
   records. Proposed defaults to validate against real deployments: 7 days of raw telemetry
   and a longer retention for its summaries.
3. **Checkpoints.** Verifiable segment chains first, with independent anchoring of
   checkpoints (for example written to a second machine), and signatures added with
   documented key management. Explicit archival verification.
4. **Critical-storage reserve.** Configurable; **256 MiB per gateway as a starting
   hypothesis for controlled testing**, to be sized from measured write rates and the
   required offline period. It is not a safety guarantee. It must be protected from
   telemetry and ordinary logs, and exhaustion behaviour must be tested.

### Minimum protection implemented ahead of the rest

See [ADR-0021](../adr/0021-journal-quota-and-reserve.md): a hard journal size limit, a
reserve below it that only stops and recovery may use, and refusal of new physical commands
once only the reserve is left. Segments, archives, retention and telemetry aggregation are
still unimplemented and still need review.
