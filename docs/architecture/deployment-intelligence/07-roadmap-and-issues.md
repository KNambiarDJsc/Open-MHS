# H. Implementation roadmap and I. Issue breakdown

Day 0 is the day this design is accepted. Durations assume one engineer with AI assistance and are estimates. Every
phase ends with a gate that can stop or narrow the work (see [09-critical-review.md](09-critical-review.md)).

## H.1 Phases

| Phase | Content | Depends on | Acceptance |
|---|---|---|---|
| **P−1 Repair** | Fix the four `cargo deny` advisories; refresh `IMPLEMENTATION_STATUS.md`; mark superseded strategy docs | — | CI green on `main`; status page matches `main` |
| **P0 Discovery** (done) | Audit, landscape, tests run | — | This folder |
| **P1 Architecture** (this review) | ADR-0026…0034 and RFC 0005 accepted, amended or rejected | P0 | Decisions recorded with status; open questions answered or assigned |
| **P2 Vertical slice** | `openmhs-events`, `openmhs-ops` (store, ingest, task assembly, incidents v1, timeline), Python importer extracted from research, CLI `ops ingest/rebuild/incidents/show`, Demo 1 | P1 | [08-vertical-slice-and-demos.md](08-vertical-slice-and-demos.md) §J acceptance list passes in CI |
| **P3 ROS 2 integration** | Importer rules for diagnostics, `ros2_medkit`, `/rosout`, task topics; live read-only observer (only if a partner needs live); Jazzy container tests | P2 | Gazebo and one partner recording import without code changes, only mapping files |
| **P4 Release intelligence** | Manifests and rollouts, `deployment.applied`, statistics (ADR-0032), `ops compare`, Demo 3, handoff packages | P2 | Simulated releases: interval coverage 93-97 % over 1,000 trials; Demo 3 reproduces committed output |
| **P5 Physical AI feedback** | Episodes, interventions, `policy.chunk`, LeRobot selection export, evaluation-run import, Demo 2 | P4 | Demo 2 runs from a clean environment; the selection loads with LeRobot's own tools |
| **P6 Service** | Ops HTTP API, static incident page, MCP read tools, tokens with scopes | P2 (can run beside P4) | API contract tests; cross-tenant refusal matrix |
| **P7 Collector** | Only if a partner needs live ingestion (ADR-0027) | P6 | Budgets of 06 §5.2 met on a Raspberry Pi 5 bench |

## H.2 30 / 60 / 90 days

| By | Engineering | Validation outside the code |
|---|---|---|
| **Day 30** | P−1 done. P2 done: Demo 1 runs on the four Gazebo recordings; idempotence and order-independence property tests pass | 10 conversations with robotics deployment/autonomy engineers about their last three field incidents (how long, which tools, what was missing). At least 2 teams agree to share one real recording plus deployment metadata |
| **Day 60** | P4 done: release comparison, Demo 3 (synthetic fleet with known rates + a real Gazebo A/B of a controller setting). Python SDK spool writer. Handoff packages | At least 1 partner's real recording imported; their incidents reviewed with them; time from "incident reported" to "evidence in hand" measured before and after |
| **Day 90** | P6 read-only service and MCP tools; P5 started with Demo 2 if approved; P3 mapping files for the partner | **Gate:** at least 1 partner uses it on their own data in 2 separate weeks, unprompted, or reports a concrete time saving. If not, narrow (critical review §3) |

## H.3 What is deliberately not on the roadmap

Hosted multi-tenant cloud, web application beyond one static page, gRPC, Kafka/ClickHouse, AI-generated summaries,
embedding search, automatic rollback, world-model types, a ROS fault manager, a visualizer.

## I. Issues (small, reviewable, testable)

Sizes: S ≤ 1 day, M ≤ 3 days, L ≤ 1 week.

| # | Title | Size | Depends | Acceptance test |
|---|---|---|---|---|
| 1 | Move `rmcp` to `~3.7` and `rusqlite` to `0.41`; run full suite | S | — | `cargo deny check` and all CI jobs green |
| 2 | Update `IMPLEMENTATION_STATUS.md`; add "superseded by" notes to strategy docs | S | — | Doc review |
| 3 | `openmhs-events`: `Event`, `Manifest`, `ArtifactRef` types with serde, size limits | M | P1 | Round-trip tests on RFC 0005 examples; unknown fields preserved in `ext` |
| 4 | Canonical JSON and SHA-256 digests for events and manifests | S | 3 | Same digest for key-order permutations; golden vectors |
| 5 | JSON Schema export + `spec/schemas/event.v0.schema.json`; CI drift check | S | 3 | `tools/check_schemas.py` fails on divergence |
| 6 | `openmhs-ops` store: migrations framework (`schema_migrations`, `user_version`) + migration 0001 (RFC 0005 §6) | M | 3 | Open old/new fixture stores; refuses a store from a newer major |
| 7 | Ingest: validate, canonicalize, accept/duplicate/conflict/reject per event | M | 4, 6 | Proptest: any permutation and duplication of a batch yields identical `events` and `event_conflicts` |
| 8 | Task assembly: open/close rules, reboot closure as `unknown` | M | 7 | Fixtures: missing end, reboot, concurrent tasks |
| 9 | Correlation rules v1: triggers, id and window joins with basis, grouping, fingerprint dedup | L | 8 | Golden incidents for fixtures; rebuild = incremental (proptest) |
| 10 | Timeline ordering with clock domains and uncertainty (reuse `openmhs-outcome` time rules) | M | 9 | Skewed-clock fixtures show concurrency, not false order |
| 11 | CLI: `openmhs ops ingest`, `rebuild`, `incidents`, `show` | M | 9, 10 | CLI snapshot tests |
| 12 | Move `ros2_evidence.py` to `python/openmhs/importers/rosbag.py` with its 18 tests; `openmhs[ros2]` extra; CI job | M | — | Tests pass in CI; research scripts import from the new place |
| 13 | Importer emits RFC 0005 envelopes (action status → `action.*`, artifact registration) | M | 5, 12 | Output validates against the schema |
| 14 | Gazebo run adapter: `intent.jsonl` → tasks, `versions.txt` + run config → manifest, verified outcomes → `outcome.verified` | M | 13 | Demo 1 acceptance (08 §J) |
| 15 | Trimmed MCAP fixture (< 5 MB) from `bullet1` for CI | S | 12 | Import test runs in CI without the 70 MB bag |
| 16 | Manifests, rollouts, `deployment.applied`, interval attribution, overlap conflicts | M | 7 | Fixtures with gaps and overlaps |
| 17 | Statistics module: Wilson, Newcombe, Poisson (Byar), CMH, `insufficient_evidence` | M | — | Reference values; 1,000-trial coverage simulation |
| 18 | `openmhs ops compare` with strata, unknown bounds and representative incidents | M | 16, 17 | Demo 3 golden output |
| 19 | Synthetic fleet generator with known per-release rates and confounds | M | 3 | Generator's declared rates recovered within intervals |
| 20 | Handoff package (`incident.json`, `timeline.md`, manifests, links, README of observed/inferred/unknown) | M | 11 | Package validates; contains no artifact bytes |
| 21 | Python SDK `openmhs.ops`: Recorder, task/action/intervention contexts, spool writer with quota and drop counters | L | 5 | Non-blocking under a stalled disk; `contextvars` across asyncio; Python 3.10-3.13 |
| 22 | Gateway exporter: audit chain → envelopes | M | 7 | A demo journal imports; `UNKNOWN_OUTCOME` maps to `unknown` |
| 23 | Ops HTTP API read endpoints + ingest endpoint + token scopes | L | 11, 16 | Contract tests; cross-tenant refusal matrix |
| 24 | MCP read-only tools over the ops API | M | 23 | Parity test like the existing CLI/MCP/SDK one |
| 25 | LeRobot selection export + Demo 2 (needs approval for `lerobot`/`torch` install) | L | 21 | Selection loads in LeRobot; Demo 2 reproducible |

Issues 1-2 can start immediately. 3-15 are the vertical slice. 16-20 are P4. 21-25 are P5/P6.
