# A. Repository audit (2026-11-15)

Scope: `main` at `d05c699`, read for the proposal in [README.md](README.md). Every claim below names the file it comes
from. "Implemented" means code exists and a test exercises it; "partial" means code exists for part of the capability;
"documented only" means a design or roadmap text with no code; "missing" means neither.

## 1. What the repository is today

OpenMHS is a **governed command gateway** for programmable physical devices: one trusted local process
([ADR-0001](../../adr/0001-single-trusted-gateway.md)) that authorizes, journals, dispatches and verifies commands from
agents and people, through adapters. It is in the actuation path of the commands it mediates. It is *not* an
observability system, and it holds no notion of a robot, site, task, deployment, release or model.

| Measure | Value |
|---|---|
| Rust workspace | 12 crates in `crates/`, 5 adapter crates, 1 template (`Cargo.toml` members), edition 2024, MSRV 1.89 |
| Largest crates (lines of Rust incl. tests) | `openmhs-engine` 21,042; `openmhs-cli` 7,255; `openmhs-gateway` 5,839; `openmhs-types` 5,799 |
| Tests (run 2026-11-15) | Rust workspace 623 passed, 0 failed; Python SDK 66 passed, 0 failed (see [README.md](README.md)) |
| Tracked files | 1,094 |
| Decisions | 25 ADRs (`docs/adr`), 4 RFCs (`docs/rfcs`), 12 strategy documents (`docs/strategy`) |
| CI | `.github/workflows/ci.yml`: Rust on three OSes, adapters in containers, ROS 2 Jazzy container, `cargo deny`, schemas |
| CI on `main` | **Red since 2026-11-10**: `cargo deny` advisories RUSTSEC-2026-0457/0461/0462 (`rmcp` 3.5.1) and RUSTSEC-2026-0459 (`libsqlite3-sys` 0.38.2). Every other job passes |

## 2. Subsystems, with a disposition for the new direction

KEEP: valuable and unchanged. EXTEND: reused and grown for the new product. EXTRACT: move a reusable part into a
shared place, only when a second user needs it. DEPRECATE: stop investing, keep working. REPLACE: rewrite or retire.

| Subsystem | Where | State | Disposition | Why |
|---|---|---|---|---|
| Command lifecycle, 19 states, transitions as data | `crates/openmhs-types/src/lifecycle.rs:54` (`TRANSITIONS`), `spec/lifecycle.md` | Implemented, property-tested | **KEEP** | The best asset in the repository: uncertainty is a state (`UnknownOutcome`), not a log line. The new event model borrows its vocabulary (reported vs verified outcome) |
| `UNKNOWN_OUTCOME` and reconciliation | `engine.rs:1700-1775` (crash and timeout paths), `engine.rs:2186` `reconcile`, `engine.rs:2274` `resolve`, `engine.rs:2333` `recover` | Implemented. Only two exits (`ReconciledApplied`, `ReconciledNotApplied`); "still unknown after looking" is proposed in RFC 0003 §1, not built (`docs/limitations.md`) | **KEEP** | Same reason. The limitation matters for incidents: an operator must pick applied/not-applied even when nobody knows |
| Readback verification | `engine.rs:1817-1945`, ADR-0020 | Implemented; `VERIFIED` is only as strong as the readback, often a simulator id (`docs/limitations.md`) | **KEEP** | |
| Command journal and hash-chained audit (SQLite) | `crates/openmhs-engine/src/journal.rs:273-387` | Implemented. WAL (`:173`), `synchronous=FULL` (`:275`), quick/integrity checks at open (`:172-183`), single `Mutex<Connection>` | **KEEP** | Correct and tested; wrong shape for high-rate events (one global lock, FULL sync per record) |
| Schema migrations | same, `CREATE TABLE IF NOT EXISTS` plus an ad-hoc backfill (`journal.rs:388-401`) | **Partial**: no stored schema version, no migration table | **REPLACE for new stores** | Any new store must start with numbered migrations and `PRAGMA user_version`. Retrofitting the journal is optional |
| Segments, sealing, archive, compaction | `journal/segments.rs`, `journal/archive.rs`, `journal/compact.rs`, RFC 0002 M3-M5 | Implemented, drilled (crash at each step, bit flips) | **KEEP, EXTRACT later** | The quota, reserve and archive logic is what an edge buffer needs. Extract into a storage crate only when the collector is built |
| Journal quota and critical reserve | ADR-0021, `engine.rs:304` `set_storage_limits`, `engine.rs:502` | Implemented | **EXTRACT later** | Same pattern for bounded edge storage |
| Audit event classes | `crates/openmhs-types/src/audit_class.rs` (`Critical`, `Security`, `Operational`, `Telemetry`), enforced by `tests/event_classes.rs` | Implemented | **EXTEND** | Directly reusable as drop priorities for an edge buffer under pressure |
| Routine telemetry store | `crates/openmhs-engine/src/telemetry.rs` (separate `telemetry.sqlite`, explicit loss counters, 7-day raw retention) | Implemented; throughput not measured (`docs/IMPLEMENTATION_STATUS.md`) | **KEEP** | Pattern to copy: loss is counted, never silent |
| State fabric (event stream) | `crates/openmhs-engine/src/engine/events.rs` (8,192 in memory, cursor polling, `missed` counts) | Implemented, **ephemeral** (ends with the process) | **KEEP** | Not a durable source. An exporter must read the audit chain, not this |
| Insights | `crates/openmhs-api/src/insights.rs:143` `summarize` | Implemented | **KEEP (pattern)** | A pure function over receipts that names the commands behind every figure: the rule incident summaries must follow |
| Outcome verifier (RFC 0004) | `crates/openmhs-outcome/src/lib.rs:770` `verify`, `audit.rs:23` | Implemented, deterministic; evaluated on a real recording and in Gazebo (`research/outcome-verifier`) | **EXTEND** | Becomes the *verified outcome* of a task event. Its time rules (clock domain, uncertainty, windows) are the clock model the event schema needs |
| Policy and authorization | `crates/openmhs-policy/src/authz.rs`, `auth.rs` (tokens hashed, constant-time) | Implemented for devices and capabilities | **EXTEND** | Scopes are device/capability globs. Incident data needs tenant, site and robot scopes |
| Credentials store | `crates/openmhs-gateway/src/store.rs` (JSON files, atomic rename, SHA-256 token hashes) | Implemented | **KEEP** | Fine for one host; not for many robots authenticating to a collector |
| Gateway API | `crates/openmhs-gateway/src/server.rs:177` (JSON-RPC 2.0 over HTTP, one route), ADR-0005/0011 | Implemented | **KEEP** | Not the ingestion API for the new product (see ADR-0031) |
| MCP server | `crates/openmhs-mcp/src/lib.rs`, `rmcp ~3.5.1` with `server, macros, schemars, transport-io` only (`crates/openmhs-mcp/Cargo.toml:14`) | Implemented, stdio only | **KEEP, fix advisories** | The flagged `rmcp` code paths (OAuth client, Streamable HTTP) are not enabled, but CI is red until the pin moves |
| Adapter host and signed manifests | `crates/openmhs-host` (`process.rs`, `trust.rs`, Ed25519) | Implemented | **KEEP, freeze** | Gateway domain, not the wedge |
| Protocol adapters (SCPI, Modbus, MQTT, HTTP) and conformance suite | `adapters/openmhs-*`, `crates/openmhs-conformance` | Implemented, simulator-verified | **DEPRECATE (investment), KEEP (code)** | Valuable for the gateway; none of it serves incident intelligence |
| Plans, leases, approvals, procedures, twin preflight | `engine/plans.rs`, `engine/leases.rs`, `engine/approvals.rs`, `engine/twin.rs` | Implemented | **KEEP, freeze** | Same |
| Device descriptors | `crates/openmhs-types/src/descriptor.rs` | Implemented. `DeviceInfo.firmware: Option<String>` (`:43`), `AdapterBinding.version` (`:52`) | **EXTEND** | The only deployment metadata in the repository: a free-text firmware string and an adapter version. No software SHA, image digest, model, policy, hardware revision, calibration, site or deployment |
| Python SDK | `python/openmhs/client.py` (gateway JSON-RPC client, ~70 methods), `adapter.py`, `runtime.py`, `agent/` | Implemented | **EXTEND** | Add an `ops` module (task context, events, offline spool) beside the client, same package |
| ROS 2 adapter | `adapters/python/ros2/ros2_adapter.py` | Implemented, **read-only by design** (subscribes to topics, no actions, no publishing) | **EXTEND** | The right posture for observation. Lacks action status, goal ids and deployment metadata |
| ROS 2 recording bridge | `research/outcome-verifier/ros2-evaluation/ros2_evidence.py` (MCAP via `mcap-ros2-support`; action status per goal UUID; `/clock` placement; empty-schema fallback) | Research code, 18 tests | **EXTRACT** | Already decodes what an importer needs. Move into a supported importer |
| Gazebo experiment harness | `research/outcome-verifier/ros2-evaluation/gazebo/` | Research code, four recorded runs | **EXTEND (demo)** | Seed for Demo 1 and the release-comparison demo |
| Simulators | `crates/openmhs-sim` (PSU, axis, thermo, faults) | Implemented, no unit test files | **KEEP** | Test fixtures |
| ESP32 firmware in QEMU | `firmware/esp32-mqtt-node` | Implemented, **emulated only** (README says so) | **KEEP** | Fixture; no hardware claim |
| OpenTelemetry | mentioned in `docs/roadmap.md:21` (M11+), `docs/architecture/SYSTEM_DESIGN.md:353` | **Documented only** | (new) | No exporter, no trace context anywhere in code |
| Distributed and federated operation | `docs/rfcs/0001-federation.md`, `docs/architecture/DISTRIBUTED_MIGRATION_PLAN.md` | **Documented only** | **KEEP (as guidance)** | The migration plan's rule ("Not justified yet: PostgreSQL, NATS, Temporal and Kubernetes together") still holds |
| Implementation status page | `docs/IMPLEMENTATION_STATUS.md` | **Stale**: says PR #10 is "not merged"; it was merged as `a5d8d94` on 2026-11-02 | **REPLACE** | |
| Strategy documents | `docs/strategy/*.md` (12), plus RFC 0004's evidence-plane framing | Three successive directions: physical tool runtime, evidence plane, now deployment intelligence | **DEPRECATE** the superseded ones after review | A reader cannot tell which direction is current |

## 3. What the new thesis needs, against what exists

| Capability the thesis needs | State | Evidence |
|---|---|---|
| Task / episode identity | **Missing** | No `task_id` in any type. Plans have step ids (`spec/plans.md`), which are gateway-internal |
| ROS action goal ids as correlation keys | **Partial (research)** | `ros2_evidence.py` maps `GoalStatusArray` to per-goal records keyed by UUID |
| Robot / site / tenant identity | **Missing** | Principals and devices only (`crates/openmhs-policy`) |
| Deployment and release metadata | **Missing** except a firmware string | `descriptor.rs:43` |
| Model / policy lineage | **Missing** | |
| Human intervention events | **Partial (gateway only)** | Approvals and operator `resolve` are recorded (`engine.rs:1951`, `:2274`); nothing for teleoperation or takeover |
| Clock domains and uncertainty | **Implemented (outcome crate)** | RFC 0004 rules 4-6; the research bridge places unstamped messages with a stated uncertainty |
| Explicit uncertain outcome | **Implemented** | `Outcome::Uncertain`, `CommandState::UnknownOutcome`; verdict `UNKNOWN` with reasons in the outcome crate |
| Durable, bounded local storage with explicit loss | **Implemented (gateway)** | Journal quota and reserve, telemetry loss counters |
| Offline buffering and resumable upload | **Missing** | Single host only |
| Idempotent ingestion / dedup | **Partial (commands)** | `commands_idempotency` unique index (`journal.rs:289`); nothing for events |
| Artifact references (MCAP, video) with time ranges | **Missing** | Evidence bundles reference journal records only |
| Incident grouping, timelines, release comparison | **Missing** | |
| OpenTelemetry trace context | **Missing** (documented only) | |
| Multi-tenant isolation | **Missing** | One home directory, one operator |

## 4. Technical debt and risks found

1. **CI is red on `main`** from four dependency advisories, none introduced by recent work. A pin move (`rmcp` to 3.7,
   `rusqlite` to 0.41) and a full test run fixes it. Until then every PR looks red.
2. **No schema versioning** in the journal or telemetry store (§2). Fine while one binary owns the file; a liability once
   stores outlive releases.
3. **One `Mutex<Connection>` and `synchronous=FULL`** for every audit record: right for command intent, too slow for
   event ingestion. The telemetry store already shows the separation pattern; a new ops store should follow it.
4. **Documentation drift**: `IMPLEMENTATION_STATUS.md` is out of date; strategy documents describe three directions.
5. **The gateway sits in the actuation path.** Any product framed as non-invasive observability must not require it.
   It can be one optional event source.
6. **Simulation-only evidence everywhere.** Every verified claim in the repository is from simulators, emulators or a
   replayed public recording. No physical robot, no external user.
7. **Python research code carries the only ROS 2 recording decoder.** It has tests but no packaging, versioning or CI job.
8. **Scale untested**: RFC 0002 M7 (Raspberry Pi, 10 M records) not done; telemetry throughput not measured.

## 5. Reusable assets, in order of value to the new product

1. The uncertainty vocabulary and state discipline (`lifecycle.rs`, `spec/lifecycle.md`, RFC 0003).
2. The outcome crate's evidence and time model (`openmhs-outcome`): clock domains, stated uncertainty, frames, verdicts
   with reasons, deterministic canonical reports.
3. Local durability patterns: WAL, quota with a critical reserve, sealed segments, event classes, explicit loss counters.
4. The MCAP/ROS 2 decoding and action-status extraction in `ros2_evidence.py`, and the Gazebo harness that produces
   real action statuses with physics ground truth.
5. The read-only ROS 2 adapter posture and its container CI job.
6. Evidence-naming summaries (`insights.rs`) and the "no text from devices or agents copied in" rule (ADR-0022).
7. Python SDK packaging, the CLI/MCP/SDK parity test discipline (CI step "invariant 10").

## 6. What the audit does not cover

Hardware behaviour (none tested), the website, the book, and the `openflexure/` directory in the working tree, which is
untracked and not part of `main`.

## 7. Running the tests

See "Tests run with this audit" in [README.md](README.md).
