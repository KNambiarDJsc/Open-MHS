# G. Scalability, reliability and security

The three stages are design scenarios, not forecasts. All figures below are estimates from stated assumptions; none
has been measured on this design. Cost figures are order-of-magnitude list-price ballparks, not quotes.

## 1. Workload assumptions

| Assumption | Value | Basis |
|---|---|---|
| What counts as an event | Structured task/action/status/fault/intervention/deployment records. **Not** telemetry, images or joint streams | ADR-0028 |
| Event size | 800 B compact canonical JSON (the RFC 0005 example is ~1.1 KB pretty-printed) | RFC 0005 §1 |
| Events per robot per second | **0.5 nominal** (a task a minute with ~20 action, status and fault events, plus markers); range 0.1 (AMR, a task every few minutes) to 1.0 (manipulation, several picks a minute) | Estimate |
| Peak | 5× average (incident bursts, shift starts); reconnect backfill counted separately | Estimate |
| SQLite and row-store footprint | ~2× raw (indexes, page overhead) | Rule of thumb, to measure |
| Columnar / Parquet footprint | ~1/8 of raw JSON with zstd | Rule of thumb for repetitive JSON, to measure |

## 2. Capacity by stage

| | Stage 1: 10 robots | Stage 2: 1,000 robots | Stage 3: 10,000 robots |
|---|---|---|---|
| Average / peak ingest | 5 / 25 events/s | 500 / 2,500 events/s | 5,000 / 25,000 events/s |
| Events per day | 0.43 M | 43 M | 432 M |
| Raw volume per day | 0.35 GB | 35 GB | 346 GB |
| Stored per day | SQLite ~0.7 GB | PostgreSQL ~69 GB if all raw events stay in it; Parquet ~4.3 GB | Column store / Parquet ~43 GB |
| Retention plan | 180 days in SQLite ≈ 125 GB (90 days ≈ 62 GB) | 7 days raw in PostgreSQL ≈ 480 GB; derived tables and incidents for 1 year; raw Parquet 1 year ≈ 1.6 TB | 90 days hot in a column store ≈ 3.9 TB; Parquet 1 year ≈ 16 TB |
| Backfill example | 1 robot offline 24 h: 43,200 events, ~35 MB, seconds | 50 robots × 8 h: 0.72 M events, ~5 min at 2,500/s headroom | 500 robots × 8 h: 7.2 M events, ~6 min at 20,000/s |
| Query patterns | Per robot/time, per task, per incident; release comparisons over ≤ 10⁵ attempts | Same, plus cross-site comparisons over ~10⁶–10⁷ attempts | Fleet-wide aggregation over 10⁸+ events |
| Infrastructure | One process, one file, a laptop or site server | PostgreSQL (one primary + replica), object storage, ops service ×2, DuckDB over Parquet for long ranges | Column store cluster, PostgreSQL HA, durable queue, ingest tier ×N, multi-region object storage |
| Approximate monthly cost | $0 (customer hardware) | $1–3k ($1–3 per robot) | $10–25k ($1–2.5 per robot) |
| First bottleneck | Disk size; single-writer SQLite under rebuild + ingest | PostgreSQL storage growth and vacuum on raw events | Ingest fan-in and cross-shard aggregation |

At 0.1 events/robot/s every figure is five times smaller; at 1.0 it doubles. Stage 2 sits near the point where raw
events stop fitting a single relational database comfortably. That is why raw events leave PostgreSQL after 7 days.

**Artifacts are not in these numbers.** They stay in customer storage, but they dominate cost: continuous MCAP
recording at about 1 GB per robot-hour would be ~240 TB/day at 10,000 robots. The design assumes **triggered or
ring-buffer recording** (rosbag2 has a snapshot mode) and references only the windows that matter.

### What stays the same across stages

The envelope, the manifest, ingestion semantics (idempotent per event id, conflicts kept), the correlation rules and
their versioning, the statistics, the SDK and spool format, the CLI. What changes: the store implementation behind one
interface, authentication strength, and the presence of a queue.

### Migration triggers (refines ADR-0029)

| From → to | Trigger (any one) |
|---|---|
| SQLite → PostgreSQL + Parquet | > 200 events/s sustained; > 200 GB store; > 1 site writing to one instance; a partner needs a hosted service |
| + column store and queue | > 5,000 events/s sustained; interactive queries routinely over > 7 days of raw events; multi-tenant hosted operation |

## 3. Reliability

### 3.1 Failure modes

| Failure | Effect | Handling |
|---|---|---|
| Ops service or network down | Nothing on the robot changes | SDK writes the spool; collector buffers; upload resumes oldest-first |
| Spool or buffer full | Data loss at the edge | Drop `info` first, keep a critical reserve (task end, intervention, deployment, fault ≥ error), count every drop and ingest the counters so reports show the gap |
| Crash between upload and marking sent | Resend | Duplicates are `duplicate` at ingest; exactly-once **storage** per event id |
| Same id, different content (bug, clock rewrite, replay of edited file) | Possible corruption | `conflict` kept beside the original; incidents show it |
| Clock jump or skew | Wrong order | Order within a boot by `seq`; across sources only beyond combined uncertainty; skew recorded per source at ingest |
| Robot reboot mid-task | Open tasks | Closed `unknown` with reason `source_restarted`, never `failed` |
| Partial spool file (power loss) | Truncated last line | The reader skips an incomplete final line and records it; a line is valid JSON or ignored |
| Schema version newer than the store | Unknown fields | Stored verbatim; rules ignore them; a newer major version is rejected per event with `event.schema_version` |
| Rebuild while ingesting | Inconsistent derived tables | Rebuild writes a new generation and swaps atomically; ingestion keeps appending events |
| Correlation rule change | Different incidents | `rules_version` on every derived row; human state kept by stable incident id, orphans shown |
| Artifact URI no longer resolves | Broken evidence | Marked `unreachable` on access, never deleted; digest still identifies it |
| Ops store corruption | Lost analysis | `PRAGMA quick_check` at open (as the journal does); restore from backup and re-ingest spools still on disk |

### 3.2 Backpressure

- SDK: bounded in-memory queue; non-blocking enqueue; drops counted.
- Collector: bounded disk with reserve; upload concurrency 1 per robot with exponential backoff and jitter on 429/5xx.
- Ops service: per-token rate limits; `429 Retry-After`; batch limits (1,000 events, 4 MiB).
- Stage 3: the queue absorbs peaks and backfills; consumers scale on lag.

### 3.3 Consistency guarantees

At-least-once delivery; idempotent storage; per-source order when `seq` is present; no global order; derived tables
deterministic for a given event set and rules version; comparisons labelled with their data cut-off.

## 4. Security

| Concern | Stage 1 | Stage 2+ |
|---|---|---|
| Robot-to-service authentication | Hashed bearer tokens per robot (the gateway's `store.rs` model), TLS verified, plaintext refused beyond loopback (ADR-0016) | Per-robot client certificates or short-lived tokens from an identity provider; rotation without robot downtime |
| Tenant isolation | `tenant` in every primary key; one query layer adds the tenant filter; tests that a token for tenant A cannot read or write B | PostgreSQL row-level security as a second barrier; dedicated databases for tenants who require it |
| Site permissions | Token scopes (`sites[]`, `robots[]`) | Same, from the identity provider's groups |
| A compromised robot | Can write false events for itself | Scope limits it to its own robot id; events are `provenance: observed` from that source, never more trusted than the robot; anomalies (future-dated events, conflict storms) flagged |
| Artifact access | Never proxied or fetched; links use the viewer's own credentials | Signed URLs generated by the customer's storage |
| Secrets | SDK never records exception messages or environment; `ext` is size-limited; docs warn against putting credentials in events | Secret scanning on ingest (pattern list) with redaction, recorded |
| PII and video | Operator ids are pseudonymous strings; video is referenced, never stored; annotations may contain names, covered by retention | Per-tenant retention and deletion by subject; data-processing terms |
| Retention | Configurable, deletes whole days, records what it deleted | Legal hold |
| Audit of Open-MHS itself | Append-only table of handoffs, annotations, token use, retention runs | Hash-chained like the gateway's audit if partners require tamper evidence |
| Resource budgets on robots | SDK < 20 MiB RSS and non-blocking; collector < 30 MiB RSS, < 1 % CPU at 50 events/s, disk quota 256 MiB | Same, measured on a Raspberry Pi 5 and a Jetson class board before any claim |
| Failure isolation | No call path from Open-MHS into robot control | Same |
| Observability of Open-MHS | Logs, `ops status` (counts, drops, conflicts, lag) | OTLP metrics and traces |
| Supply chain | `cargo deny` in CI (currently failing: fix first), locked Python deps for importers | SBOM per release, signed artifacts (the repository already uses `dist`) |

## 5. Engineering quality

### 5.1 Tests

| Kind | What it proves |
|---|---|
| Unit | Envelope validation, canonical JSON and digests, manifest diff, Wilson/Newcombe/CMH against published reference values |
| Property-based (proptest, already a dependency) | Any permutation and duplication of a batch gives identical derived tables; incremental derivation equals full rebuild; conflict rows never overwrite |
| Integration | Import the four Gazebo runs and the Málaga recording; incidents and timelines match committed expectations |
| Fault injection | Truncated spool lines, full disk, crash between upload and mark, conflicting resends, clock jumps, reboots mid-task |
| Restart and recovery | Kill the ops service during ingest and rebuild; reopen passes `quick_check`; nothing accepted is lost |
| Deduplication and out-of-order | Shuffled multi-robot streams with injected duplicates |
| Clock skew | Sources with offset clocks and stated or missing uncertainty; ordering shows concurrency rather than a false order |
| Load | Sustained 500 events/s and 5,000-event bursts into SQLite; 1 M-event rebuild |
| Schema compatibility | Old fixtures against new code; JSON Schema ↔ Rust ↔ Python round trip; unknown fields preserved |
| Multi-tenant security | Cross-tenant and cross-site reads and writes refused for every endpoint (generated test matrix) |
| ROS 2 | Importer and live observer in the existing Jazzy container CI job |
| Python SDK | Python 3.10-3.13; non-blocking enqueue under a stalled disk; `contextvars` across threads and asyncio |
| Statistics | Simulated releases with known failure rates: interval coverage ≈ 95 % over 1,000 simulated comparisons; false-regression rate at the declared margin |

### 5.2 Performance targets (to be measured, not claimed)

| Target | Value | Why this number |
|---|---|---|
| Batched ingest into SQLite on a laptop | ≥ 5,000 events/s | 200× Stage 1 peak; makes a 24 h backfill of a 10-robot site (0.43 M events) take under 2 minutes |
| Full rebuild | 1 M events in < 60 s | Rules changes must be cheap enough to iterate on |
| Timeline query | p95 < 200 ms for an incident with ≤ 1,000 members | Interactive investigation |
| Release comparison | < 2 s over 10⁵ attempts in SQLite | Stage 1 data sizes |
| SDK enqueue | p99 < 50 µs, no allocation-heavy work on the caller thread | Robot application threads must not notice it |
| Collector | < 30 MiB RSS, < 1 % of one core at 50 events/s | Embedded computers already run perception and planning |

**Simulation and laptop measurements are not production readiness.** A claim about robots needs a design partner's
robots, or at least a Jetson/Raspberry Pi bench run with the partner's event rates.
