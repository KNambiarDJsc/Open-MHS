# ADR-0029: Storage: SQLite first, PostgreSQL and object storage on measured triggers

- Status: Accepted (reviewed 2026-11-15/16)
- Date: 2026-11-15
- Builds on: [ADR-0021](0021-journal-quota-and-reserve.md), [RFC 0002](../rfcs/0002-storage-engine.md),
  [DISTRIBUTED_MIGRATION_PLAN.md](../architecture/DISTRIBUTED_MIGRATION_PLAN.md)

## Decision

1. **Stage 1 (one organisation, up to tens of robots):** one SQLite file, `ops.sqlite`, separate from the gateway
   journal. WAL mode, `synchronous=NORMAL` (an event store may lose the last transaction on power loss; the journal may
   not), numbered migrations recorded in a `schema_migrations` table and `PRAGMA user_version` from the first release.
   Events are the source of truth; incidents, task attempts and release tables are **derived and rebuildable** by replay.
2. **Large artifacts never enter a database.** Recordings, video and point clouds stay where the team keeps them
   (disk, S3, Foxglove, Roboto); Open-MHS stores a reference (URI, SHA-256 when known, media type, time range, topics).
3. **Stage 2 triggers** (any one): sustained ingestion above 200 events/s; hot data above 200 GB; more than one site
   writing concurrently to a shared instance; a design partner requiring a hosted service. Then: PostgreSQL for
   metadata (deployments, incidents, task attempts, annotations, tenants) and the last 7 days of raw events in daily
   partitions; older raw events as Parquet in object storage, queried with DuckDB when needed. At 1,000 robots and
   0.5 events/s each, raw events would add ~69 GB/day to PostgreSQL, which is why they do not stay there.
4. **Stage 3 triggers** (any one): sustained above 5,000 events/s; analytical queries over 30 days exceed 10 s on
   tuned PostgreSQL; multi-tenant hosted operation. Then: a column store (ClickHouse or equivalent) for events, a
   durable queue (NATS JetStream, Redpanda or Kafka, chosen on measured needs) in front of ingestion.

## Alternatives considered

- **PostgreSQL from day one**: a server to run for a local investigation tool; slows design-partner trials. Rejected
  for Stage 1.
- **ClickHouse or DuckDB-only**: DuckDB is attractive for analysis over Parquet and may be added as a read path;
  neither is a good transactional store for incidents and annotations. Deferred.
- **Kafka/NATS now**: no workload to justify it (the 2026-11 migration plan already says so). Rejected for Stages 1-2.

## Consequences

- One binary and one file for local use; backup is a file copy (with the SQLite backup API).
- The store interface must keep SQL simple enough to port (no SQLite-only JSON tricks in core queries).
- Capacity numbers behind the triggers are in [06-scale-reliability-security.md](../architecture/deployment-intelligence/06-scale-reliability-security.md).
