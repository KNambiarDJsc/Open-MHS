# Distributed reliability architecture: the maintainer's brief

This is the maintainer's architecture analysis and implementation prompt, preserved as given
(only the diagrams are stored as images). It is the **specification of the next phase**. Read
[HANDOFF.md](HANDOFF.md) first for the repository's real state and the constraints that apply.
Where this brief and the repository disagree, inspect the code (Phase 0) and say so.

Diagrams: [topology.png](images/topology.png) (central control plane + site-local execution:
API/Auth, Contracts+Policy, Workflow Coordinator, Command Ledger on PostgreSQL + outbox; durable
messaging/mTLS; per-site OpenMHS Edge Gateway with Policy, WAL, Fencing; device adapters) and
[command-lifecycle.png](images/command-lifecycle.png) (RECEIVED → ADMITTED → DISPATCHED → DEVICE
INVOKED → VERIFIED | UNCERTAIN → RECONCILIATION; a command may also be rejected before dispatch or
fail with a proven no-effect result; "verification must state what was verified: API completion,
observed state, or a physical outcome").

## The seven architectural responsibilities

| Component | Responsibility | Failure strategy |
|---|---|---|
| API/admission | Authenticate requests, validate schema, accept command intent | Stateless replicas |
| Contract registry | Versioned device capabilities, units, operating limits | Cache signed, expiring policies at edge |
| Durable ledger | Authoritative command status, idempotency and evidence references | PostgreSQL transactions and recovery |
| Workflow engine | Multi-step coordination, approvals and compensation | Durable state-machine replay |
| Messaging transport | Deliver work and status between control plane and sites | Persistent messages, acknowledgments |
| Edge gateway | Final policy enforcement, serialization and device execution | Local write-ahead journal and reconciliation |
| Observability | Traces, metrics, immutable evidence exports | Buffered telemetry, integrity checks |

The important principle is that the central plane can scale horizontally, but execution authority
must be constrained per physical resource. You can have hundreds of API replicas and thousands of
edge gateways. You should not have two independently acting controllers writing to the same
microscope stage.

## Technology choices

| Layer | Recommended stack | Rationale |
|---|---|---|
| Execution core | Existing Rust OpenMHS crates, Tokio | Preserve current investment; async I/O |
| Central database | PostgreSQL | Transactional commands, outbox, policy versions, workflow states |
| Edge journal | SQLite WAL, initially | Local persistence without a central network dependency |
| Site messaging | NATS JetStream, when needed | Lightweight durable transport and backpressure |
| API | Rust Axum + gRPC/Protobuf where useful | Versioned internal protocols; HTTP/MCP for clients |
| Workflow coordination | Rust state machine first; evaluate Temporal later | Avoid adding another source of truth prematurely |
| Device interfaces | LabThings/WoT, HTTP, Modbus, OPC UA, ROS 2, SCPI adapters | Protocol-specific capabilities and execution semantics |
| Authentication | OIDC for users, mTLS workload identities | Separate human and machine authority |
| Observability | OpenTelemetry, Prometheus, Grafana, Loki | Correlation across agents, gateways and adapters |
| Deployment | Single-node or site appliance first; Kubernetes for scaled central services | Edge autonomy without unnecessary orchestration dependency |

Temporal is a credible option for long-running coordination because it persists workflow progress
and provides activity retries. But its activities can retry after failures, so Temporal would not
replace OpenMHS's command idempotency and reconciliation layer. For database high availability,
replication configuration must explicitly balance durability against availability: PostgreSQL
synchronous replication can wait for a standby before acknowledging commits, but it introduces
latency and can block writes when a required standby is unavailable.

## Problems to fix before adding any distributed infrastructure

- **P0 Incorrect device selection.** Remove concrete example device IDs from agent tool
  descriptions. A language model copying `sim.axis-01` four out of four times is a tool-design
  defect, not just a model weakness. Introduce discovery-selected, validated device references.
- **P0 Silent false success.** Make `get_insights` return a typed `DEVICE_NOT_FOUND` when a
  requested device does not exist. Unknown devices must never become an empty successful
  observation.
- **P0 No permission-discovery API.** Introduce `get_allowed_operations` scoped to the
  authenticated principal, resource and current policy, with a revision and expiry. Treat the
  response as informative, not as an authorization token.
- **P1 Unauthenticated evidence gap.** Include receipt digests in an independently verifiable
  evidence record and externally anchor audit-chain checkpoints. A local hash chain alone does not
  protect against a privileged attacker rewriting history.
- **P1 Unsafe restart assumptions.** LabThings invocation IDs disappearing after restart means
  recovery cannot rely on polling alone. Introduce capability-specific recovery strategies,
  including `UNKNOWN_OUTCOME`.

These improvements would benefit every deployment, even one running entirely on a laptop.

## Scaling without destroying simplicity

- **Level 1, embedded single-machine.** One OpenMHS runtime, SQLite journal, native adapters,
  MCP/CLI/Python API. Remains a fully supported deployment mode, not a toy demo.
- **Level 2, one laboratory, multiple devices.** Multiple adapters and independently supervised
  processes, local registry, resource-scoped scheduling, resilient action tracking and local
  operator controls. No cloud requirement.
- **Level 3, multi-site distributed.** PostgreSQL-backed control plane, NATS JetStream, site
  gateways, mutual TLS, durable workflow orchestration, remote audit and fleet management. Sites
  continue safe local recovery during disconnection.
- **Level 4, industrial/research production.** Replicated infrastructure, certificate rotation,
  policy rollout, hardware watchdogs, secure upgrades, disaster recovery drills, signed artifact
  provenance and independently reviewed device-specific safety cases.

A Level 4 control plane must not be required to run a Level 1 microscope integration.

## Production-readiness gates

| Gate | Release requirement |
|---|---|
| Correctness | The three known tool defects fixed; lifecycle invariants tested |
| Persistence | Crash/restart recovery proven at every critical transition |
| Concurrency | Simulated split-brain and stale-owner tests pass |
| Security | Threat model, authorization tests and independent review |
| Evidence | Receipts verified, checkpoints externally anchored |
| Resilience | Disk-full, partitions, restarts and broker/database outage drills |
| Compatibility | LabThings integration works without unsupported hardware assumptions |
| Physical testing | Restricted, supervised testing before any real-hardware production claims |

The study's limitations (unavailable 9B model run, simulator-only execution, unverified newer
LabThings versions) must remain explicit in all release reports.

## Positioning

*A reliable execution and evidence layer between AI agents and programmable physical systems.*
Not a ROS replacement, not just a hardware MCP server, not another device dashboard. The
contribution: an agent requested this operation; this identity was authorized; these constraints
were checked; this device invocation was recorded; this evidence was observed; and here is what
we know, or cannot know, about the result. **Build the execution kernel and recovery semantics
first, then distribute them**: a correct single-node state machine is the foundation of a
reliable fleet; a distributed platform on ambiguous command semantics merely distributes the
ambiguity.

---

# The implementation prompt

**OpenMHS: Distributed Reliability Architecture and Implementation**

You are the principal architect responsible for evolving OpenMHS from a simulator-validated
hardware gateway into a robust, production-oriented, distributed execution system.

You are working on the existing OpenMHS repository. Preserve its fundamental architecture,
contracts, interfaces and passing tests.

## Current evidence

The research branch `research/labthings-wot` (local commit `5539d19` when this was written;
see HANDOFF.md for the current head) contains a 12-phase LabThings/OpenFlexure interoperability
study. First read: `research/DECISION_MEMO.md`, all numbered research reports,
`research/results/`, `research/experiments/`, and the existing OpenMHS architecture and ADRs.

The study reports: 225 fault-injection runs; 444 passing Rust tests; 55 passing Python tests; 25
importer tests; correct denial of out-of-limit operations in the tested scenarios; significant
client-dependent recovery differences; audit chain limitations; loss of LabThings invocation
lookup after simulator restart; three unresolved agent-tool defects. **Do not assume these
results establish physical-hardware safety or production readiness.**

## Mission

Design and incrementally implement a distributed architecture where OpenMHS provides:

1. Durable command execution records
2. Safe idempotency and explicit uncertainty
3. Resource ownership and fencing
4. Reliable site-to-control-plane communication
5. Recovery after crashes and partitions
6. Hardened agent-tool interfaces
7. Independent local execution gateways
8. Scalable multi-device and multi-site coordination
9. Audit integrity and evidence export
10. Operational observability and recovery tooling

The primary objective is correctness under failure, not the number of technologies introduced.

## Phase 0: Architecture audit

Inspect current code before proposing new services. Produce: the actual code-level module
dependency graph; the existing command state machine; the persistence model; the concurrency and
locking model; failure/retry semantics; audit and receipt schema; authentication and
authorization boundaries; adapter lifecycle; test coverage and missing failure modes;
compatibility constraints. Classify each proposed change as **necessary now, necessary later, or
unnecessary**. Preserve the existing standalone mode. Never rewrite working components solely to
create microservices.

## Phase 1: Fix immediate correctness defects

Implement: removal of hardcoded example device IDs from agent instructions; strict unknown-device
errors across relevant tools; `get_allowed_operations` for effective principal-scoped
capabilities; contract and policy revision metadata in responses; expiring discovery results;
accurate tool errors and structured refusal reasons. Add unit and integration tests. Run all
relevant existing tests before proceeding.

## Phase 2: Formalize command execution

Define a versioned command envelope containing: `command_id`; `tenant_id` and `site_id` where
applicable; `device_id` and `capability_id`; actor identity and authorization context;
`idempotency_key`; canonical request digest; `contract_digest` and `policy_revision`;
`owner_epoch`; expected observation version, if supplied; deadline and request expiry;
`trace_id`; immutable command parameters.

Define typed states including: `RECEIVED`, `REJECTED`, `ADMITTED`, `DISPATCHED`,
`INVOCATION_PENDING`, `COMPLETED_UNVERIFIED`, `VERIFIED`, `FAILED_NO_EFFECT`, `UNKNOWN_OUTCOME`,
`CANCEL_REQUESTED`, `CANCELLED_CONFIRMED`, and `RECONCILIATION_REQUIRED`. Use a formally reviewed
transition table. Distinguish API acceptance, execution, observation and physical verification.
Duplicate command IDs must return existing outcomes, not execute again. An idempotency key reused
with mismatched command parameters must be rejected. Handle the crash window between transmission
and receipt recording without pretending it can be eliminated for non-idempotent hardware.

## Phase 3: Durable command ledger

Build a storage abstraction with: the existing embedded SQLite backend; a PostgreSQL backend for
the distributed control plane; versioned migrations; transactional command admission;
transactional outbox; optimistic version checks; durable invocation references; receipt integrity
metadata; explicit retention policies; replay-safe recovery. PostgreSQL is authoritative for
centrally admitted commands; local edge persistence holds execution evidence. Define how command
status is reconciled when the edge and central database temporarily disagree. Do not use Redis or
a message queue as the authoritative command ledger. Test crash points surrounding every durable
transition.

## Phase 4: Single-writer resource ownership

Implement resource-scoped command serialization. Provide: logical ownership leases; monotonic
fencing epochs; lease expiration and reassignment; resource conflict detection; multi-resource
acquisition ordering; admission backpressure; explicit write refusal when ownership is uncertain.
A lease alone cannot stop a previously authorized writer already communicating with a device.
Where hardware supports it, enforce fencing at the final device-controlling endpoint. For hardware
without fencing support, provide a single-writer local process boundary and conservative failover
behaviour. Never allow two site agents to independently issue writes to the same device because
they both believe a lease is valid.

## Phase 5: Edge gateway

Implement an independently running OpenMHS edge gateway: local journal; locally enforceable
policy snapshot; cached capabilities with freshness metadata; workload identity; device adapter
process supervision; backpressure and bounded queues; controlled shutdown; offline evidence
buffering; reconnection and reconciliation; health and readiness probes; safe operation without a
persistent cloud connection. During cloud disconnection, no new remote authorization is
fabricated: allow only explicitly provisioned, unexpired local authority and operations permitted
by the relevant offline policy. If authorization or device state is uncertain, reject or hold
unsafe writes. Emergency functions must remain independent of the remote control plane and must
not depend on an LLM or central message broker.

## Phase 6: Distributed communication

Evaluate NATS JetStream against direct gRPC before adding the broker; use a broker only if it
materially improves multi-site delivery, resilience or backpressure. For the broker design
specify: subject naming; tenant/site isolation; durable consumers; acknowledgment strategy;
redelivery behaviour; message retention; delivery expiration; dead-letter handling; outbox
publishing; inbox deduplication; replay and poison-message handling. Do not equate at-least-once
message delivery with exactly-once hardware execution; broker redelivery must not blindly repeat
side effects.

## Phase 7: Durable workflows

Design a deterministic multi-device workflow coordinator supporting: workflow IDs and versioning;
approved execution plans; resource reservations; preconditions and postconditions; dependency
graphs; bounded parallelism; human approvals; timeouts; cancellation; compensating actions;
unknown-outcome suspension; operator-driven reconciliation. Evaluate an internal Rust workflow
state machine against Temporal, including operational burden. Do not introduce Temporal
automatically. An LLM may propose a plan but may not directly mutate authoritative execution state.

## Phase 8: Identity and authorization

Implement an explicit threat model covering: agent identity; human identity; service/workload
identity; tenant and site isolation; least-privilege scopes; short-lived credentials; mTLS
transport; policy signing and revision pinning; approval expiry; replay resistance; secret
rotation; adapter compromise; privilege escalation; audit access control. Require the gateway to
revalidate writes at the final execution boundary. Never trust an agent's statement that it has
permission.

## Phase 9: Audit and evidence integrity

Address the research findings: receipt contents are not fully covered by current audit integrity
checks; entire chains can be recomputed by a privileged attacker; tail truncation can go
undetected without an external anchor. Design: canonically serialized records; durable receipt
digests; signed checkpoint manifests; periodic external anchoring; independent verifier;
checkpoint retention; explicit evidence completeness flags; missing-sequence detection; storage
exhaustion handling; independent export and verification commands. Do not claim absolute
tamper-proofness. Ensure audit degradation never silently converts a refused or uncertain write
into success.

## Phase 10: Observability

Instrument critical execution paths using OpenTelemetry. Correlate: `request_id`, `command_id`,
`workflow_id`, `site_id`, `device_id`, native invocation ID, `trace_id`, policy decision, outcome
classification. Measure: command admission latency; queue delay; device invocation latency;
verification latency; unknown-outcome rate; recovery duration; duplicate-dispatch attempts; policy
refusal rate; edge disconnect duration; journal backlog; audit verification failures; storage
pressure. Avoid exposing credentials and sensitive payloads in telemetry.

## Phase 11: Chaos and distributed testing

Use deterministic simulation and fault injection. Test at minimum: gateway process death before
device invocation; gateway death immediately after invocation; control plane restart; database
failover; message redelivery; duplicate request; stale owner; split-brain attempts; network
partition; out-of-order events; lost acknowledgment; lost status response; device restart; expired
approval; stale policy; missing Thing Description units; corrupted journal; disk full; audit-chain
truncation; clock skew; slow consumers; cancellation during execution; partially completed
multi-device plans. Define temporal invariants and use state-machine/property-based testing
wherever practical.

Critical invariants:

- No unauthorized physical dispatch
- No duplicate dispatch for the same command unless the operation is explicitly safe and idempotent
- No stale controller allowed to write when effective fencing is available
- No unsupported claim of verified completion
- No silent loss of an admitted command
- No automatic non-idempotent retry after ambiguous failure
- No execution under an invalid or expired policy
- No LLM-controlled emergency stop bypass or weakening of hard safety limits

For hardware lacking native fencing or idempotency, explicitly document the weaker attainable
guarantees and the required operator intervention. **Do not infer these invariants passed from the
old 225-run research suite.** Build stronger tests and report the actual results.

## Phase 12: Performance and scale

Benchmark in standalone and distributed modes. Measure separately: read-only throughput;
concurrent device status collection; command admission rate; per-device write throughput; number
of simulated gateways; number of simulated devices; number of concurrent workflows; message
backlog recovery; persistence overhead; memory and CPU consumption; tail latency; recovery time.
Do not optimize for high-throughput actuator writes at the expense of serialization or safety.
Propose numerical SLOs only after establishing baselines.

## Phase 13: Deployment and operations

Create reproducible configurations for (1) embedded standalone, (2) single-lab multi-device, (3)
multi-site distributed. Provide: Docker Compose development setup; secrets handling; database
migrations; backup and restore; startup ordering; safe rollback; upgrade compatibility;
certificate rotation; offline recovery instructions; disaster recovery exercises; operator
runbooks. Kubernetes is optional and only for centrally hosted services where warranted. Do not
make edge gateways depend on Kubernetes.

## Phase 14: Architecture decision records

Produce: `docs/architecture/DISTRIBUTED_SYSTEM.md`, `COMMAND_LIFECYCLE.md`,
`RESOURCE_OWNERSHIP.md`, `EDGE_RECOVERY.md`, `SECURITY_MODEL.md`, `FAILURE_MATRIX.md`,
`DEPLOYMENT_MODES.md`, `SCALE_BENCHMARKS.md`. Each must state: problem; existing solution;
proposed change; alternatives considered; why it is needed; failure guarantees; guarantees it
cannot provide; complexity and operational costs; verification evidence.

## Implementation order and working rules

First inspect the actual repository and the local research branch. Do not start by installing
Kubernetes, NATS, Temporal and PostgreSQL all at once. Implement incrementally:

1. Correctness fixes
2. Command protocol and state machine
3. Durable ledger and recovery
4. Resource ownership
5. Edge gateway
6. Distributed communication
7. Workflow coordination
8. Audit strengthening
9. Security and observability
10. Chaos tests and production deployment

Make small, independently testable commits. Keep the embedded deployment working throughout. Run
Rust formatting, clippy, unit tests, integration tests and relevant research regressions after
each major change. **Never push, publish, merge, delete unrelated files, modify external
repositories or contact researchers without explicit approval.** If a proposed guarantee is
impossible without stronger hardware support, explain that and implement the safest available
behaviour rather than claiming it is solved.

## Final deliverable

An architecture grounded in the actual codebase; working implementations for the highest-priority
phases; passing tests with real outputs; distributed failure-injection results; a migration
strategy from the existing OpenMHS architecture; clear remaining risks; a concrete
release-readiness assessment. Do not declare OpenMHS production-ready merely because tests pass.
The target is a system that fails predictably, preserves evidence, and keeps hardware control
constrained even when distributed infrastructure behaves badly.
