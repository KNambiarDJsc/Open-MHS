# ADR-0034: Two new Rust crates and a Python importer; no services directory until Stage 2

- Status: Proposed
- Date: 2026-11-15
- Builds on: [ADR-0008](0008-languages.md)

## Decision

1. `crates/openmhs-events`: the envelope types, validation, canonical JSON, JSON Schema export, deployment manifests.
   Publishable (other projects may emit events), depends only on `serde`, `serde_json`, `sha2`, `uuid`, `time`.
2. `crates/openmhs-ops`: the SQLite store with numbered migrations, idempotent ingestion, task-attempt derivation,
   correlation rules, release statistics and handoff packages. Library first; the CLI (`openmhs ops ...`) and, later, a
   service binary use it.
3. The **ROS 2 recording importer stays in Python** (`python/openmhs/importers/`, optional extra `openmhs[ros2]`),
   because `mcap-ros2-support` decodes embedded message definitions today and the Rust equivalents are less mature.
   It emits envelope JSON Lines; Rust ingests them. The code moves from `research/outcome-verifier/ros2-evaluation`.
4. The **Python SDK** gains `openmhs.ops` (task context, events, spool) in the existing `openmhs` package.
5. No `services/`, `web/` or `integrations/` top-level directories until something lives in them; the collector
   (`crates/openmhs-collector`) is created when ADR-0027's condition is met.

## Alternatives considered

- **The full monorepo tree from the brief** (`openmhs-core`, `-edge`, `-ingest`, `-correlation`, `-storage`, `services/`,
  `web/`): six crates and two services before one user. Rejected for now; the split above can grow into it.
- **Rust MCAP/CDR decoding now**: possible (the `mcap` crate exists) but dynamic ROS 2 message decoding would be new,
  untested code on the critical import path. Revisit when import speed matters.

## Consequences

- The research bridge's 18 tests move with it and run in CI.
- Two languages on the import path; the envelope schema (JSON Schema exported from Rust) is the contract between them.
