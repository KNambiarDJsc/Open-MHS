# OpenMHS platform architecture: current, target, and what is missing

- Status: Living document. Section 2 is verified against the code at commit `3e82878`
  (branch `dx/mcp-sim`), with a code graph built by [Graphify](https://github.com/Graphify-Labs/graphify)
  0.9.97 (local tree-sitter pass only: 6 001 nodes, 14 112 edges, 273 communities; no LLM).
- Date: 2026-10-28
- Scope: the "Universal Physical AI Operating Platform" brief. That brief lists 17 target
  capabilities and 13 subsystems. This document records which exist, builds what was
  missing and can be built safely now, and designs the rest.

## 1. What the benchmark is, and is not

The brief uses Anthropic's publicly demonstrated hardware-agent capabilities as a
functional benchmark. Anthropic's implementation and its Model Hardware Standard are not
public. This document makes **no claim about how they work and no claim of compatibility**
(ADR-0007). The comparison below is against the 17 capabilities the brief names, because
those can be checked against code and tests.

## 2. Verified gap matrix

Legend:
- **Built**: implemented and tested.
- **Partial**: implemented with a stated gap.
- **New**: built in this change.
- **Missing**: not implemented. Designed in §5.

"Graph" means the Graphify query returned no implementing node beyond the ones named.

| # | Capability | State | Where (code, tests) | What is missing |
|---|---|---|---|---|
| 1 | Programmable hardware discovery | **Partial** | `openmhs discover scpi`, `modbus`, `http`, `mqtt`: read-only identification at named addresses; registers nothing (`crates/openmhs-cli/src/discover.rs`, four CLI tests) | mDNS, ROS 2 graph introspection; an operator-only `devices.discover` API (§5.4) |
| 2 | Device registration and lifecycle | **Partial** | `Engine::register`, `register_allow_offline` (`engine.rs`). Availability ADR-0019; `tests/availability.rs` | Hot add and remove without a restart |
| 3 | Unified state and capability abstraction | **Built** | `spec/descriptor.md`, `openmhs-types/src/descriptor.rs`, `validate.rs`, `spec/examples/invalid/*` | Semantic operation names (`power_supply.configure_voltage`) as an alias layer over capability names (§5.5) |
| 4 | Agent-accessible device tools | **Built** | 18 MCP tools (`openmhs-mcp/src/lib.rs`), CLI, Python SDK; `tests/mcp.rs` | — |
| 5 | Live cross-device state subscriptions | **New** | `events.poll`, `watches.*` (ADR-0023, `engine/events.rs`); `tests/events.rs`, MCP, CLI and Python tests | Push transport; adapter-native subscriptions for event-kind capabilities |
| 6 | Capability-based hardware selection | **New** | `find_capabilities`, `openmhs find`, `Client::find_capabilities` (`openmhs-api/src/selection.rs`) | Selection by relationship (via the capability graph), for example "the meter wired to this supply" |
| 7 | Multi-device workflow orchestration | **Built** | Plans with leases, compensation, twin preflight (ADR-0018); conditional steps (ADR-0024); opt-in parallel steps (ADR-0025); `tests/plans.rs`, `conditional_steps.rs`, `parallel_steps.rs` | Plans spanning gateways (RFC 0001) |
| 8 | Reusable deterministic procedures | **New** | `spec/procedures.md`, `openmhs-types/src/procedure.rs`, gateway `procedures.*`; agents propose, operators approve; `gateway/tests/procedures.rs`, MCP, CLI and Python tests | Procedure-level approval rules (for example, two approvers) |
| 9 | Adaptive observation and replanning | **Built** | Structured refusals (`guidance.rs`), insights (ADR-0022), the model-agnostic harness `openmhs.agent` (`docs/agent-harness.md`, `python/tests/test_agent.py`) | A run against a real model has not been recorded |
| 10 | Human approvals, deterministic policy | **Built** | ADR-0003, ADR-0017, `engine/approvals.rs`, `tests/approvals.rs` | — |
| 11 | Reliable physical command execution | **Built** | Lifecycle (ADR-0002, ADR-0010), journal, recovery; `tests/invariants.rs` | Gap-analysis items H5–H7 |
| 12 | Explicit uncertainty and reconciliation | **Built** | `UNKNOWN_OUTCOME`, `reconcile`, `resolve`, ADR-0020; `tests/stale_readback.rs` | A readback cannot prove a no-op (documented) |
| 13 | Cross-host laboratories and fleets | **Missing** | Single gateway | Federation, designed only (§5.6) |
| 14 | Robotics, instrument, industrial adapters | **Partial** | Modbus TCP, MQTT, SCPI (TCP and serial), HTTP, Python host; OPC UA read-only, ROS 2 observation-only, OctoPrint read-only (`adapters/`) | VISA, vendor SDKs, any write path for ROS 2 or OPC UA; **no physical hardware evidence** |
| 15 | Simulation and failure injection | **Built** | `openmhs-sim` (power supply, thermometer, linear stage), generic and per-device faults; profile simulators | A measurement instrument or electronic load simulator (§6) |
| 16 | Adapter registry and conformance | **Built** | `registry/`, L0–L3 conformance with negative controls, `tools/check_registry.py` | `L2.stale_readback` (deferred until the ESP32 run) |
| 17 | SDKs, examples, governance, docs | **Partial** | Python SDK, adapter template, GOVERNANCE.md, CONTRIBUTING.md, ADRs | Not published to PyPI or crates.io; no release yet |

## 3. Architecture

### 3.1 Current (after this change)

```text
 agents / people                 one host
 ───────────────     ┌──────────────────────────────────────────────────────────┐
 MCP host ──stdio──► │ openmhs mcp ─┐                                           │
 CLI ──────────────► │              ├─► gateway: JSON-RPC on loopback HTTP      │
 Python SDK ───────► │              │     auth (scoped tokens) · roles          │
                     │              │                                           │
                     │              ▼                                           │
                     │   engine (execution kernel)                              │
                     │    validate → scope → policy → limits → storage gate →   │
                     │    availability gate → lock → unresolved hold → live     │
                     │    checks → [approval] → journal intent → adapter →      │
                     │    verify (readback after dispatch) / UNKNOWN_OUTCOME    │
                     │    plans · leases · capability graph · twins             │
                     │    state fabric: event ring + watches (subscribe)  [new] │
                     │    journal.sqlite: commands, approvals, leases, plans,   │
                     │                    hash-chained audit, size limit        │
                     │              │                                           │
                     │   adapters: in-process (Modbus, MQTT, SCPI, HTTP, sims)  │
                     │             out-of-process (Python host, signed manifest)│
                     └──────────────┼───────────────────────────────────────────┘
                                    ▼
                          devices (their own controllers and
                          safety systems keep final authority)
```

Client-side, from the journal and descriptors, with no gateway change:
- `insights` (ADR-0022);
- `find_capabilities` (§2 row 6).

### 3.2 Target

A modular monolith per site, with adapters out of process where isolation matters, and
federation between sites:

```text
 site A gateway (authority for A's devices)        site B gateway (authority for B's)
 ┌────────────────────────────┐   signed, mTLS     ┌────────────────────────────┐
 │ A  registry + discovery    │◄──────────────────►│ ... same subsystems ...    │
 │ B  capability graph        │  descriptors,      └────────────────────────────┘
 │ C  state fabric            │  events (read),
 │ D  tool registry (MCP)     │  forwarded commands
 │ E  execution kernel        │  (B's policy decides
 │ F  procedures + DAG plans  │   B's devices)
 │ G  agent harness (client)  │
 │ H  scheduler, leases,      │
 │    fencing                 │
 │ I  identity, policy,       │
 │    approvals               │
 │ J  adapter runtime + SDK   │
 │ K  audit, evidence,        │
 │    observability           │
 │ L  simulation, conformance │
 │ M  ecosystem, registry     │
 └────────────────────────────┘
```

## 4. Subsystem contracts

Rust unless stated. "Persistent" means in the journal.

| | Subsystem | Responsibility | Contract | State | Failure behaviour | Security boundary | Tests |
|---|---|---|---|---|---|---|---|
| A | Registry and discovery | Which devices exist and how to reach them | Descriptor (`descriptor.schema.json`), `devices.list/describe/status` | Ephemeral registry; availability in memory; registrations audited | Unreachable device is registered `OFFLINE`; strict mode refuses start | Registration grants nothing | `availability.rs` |
| B | Capability graph | Relationships, shared resources, frames | `graph.get`, topology file | Topology from operator config | Unknown device in topology stops the start | Operator-only topology | `openmhs-types/src/graph.rs` (unit), `openmhs-gateway/tests/graph.rs` |
| C | State fabric | What changed, in order | `events.poll`, `watches.*`, `Event` (`openmhs-types/src/event.rs`) | Ephemeral ring and watches; watch create and end audited | Overflow reported as `missed`; restart is `reset` | `subscribe` scope and grant, per event | `events.rs` |
| D | Tool registry | What a model can call | MCP tools with output schemas and risk annotations | None | A refusal is a tool result, never a crash | Agent token only; operator token refused | `openmhs-mcp/tests/mcp.rs` |
| E | Execution kernel | The only path to `execute` | `commands.*`, lifecycle (`spec/lifecycle.md`) | Journal | Fail closed; `UNKNOWN_OUTCOME`, never a blind retry | Deny by default; approvals bound to fingerprints | `invariants.rs`, `stale_readback.rs` |
| F | Workflows | Ordered multi-device work | `plans.*` (`spec/plans.md`) | Journal | Halt, compensate or ask; never automatic physical rollback | Each step is the caller's ordinary command | `plans.rs` |
| G | Agent harness | Goal → plan → act → observe → replan | MCP or the Python SDK; Python (§5.3) | Client-side | Stops and reports instead of widening | Never holds operator credentials; no generated code | `examples/agent-loop` |
| H | Scheduler | Contention and fairness | Leases (`leases.*`), device lock, stop lane | Journal | Lease expiry; stop never waits | Lease within scopes and grants | `engine/tests/leases.rs`, `stop_lane.rs` |
| I | Identity and policy | Who may do what | Tokens, scopes, grants, approvals (ADR-0017) | Home directory and journal | Unknown means denied | The core boundary | `openmhs-policy/src/auth.rs`, `authz.rs` (unit), `engine/tests/approvals.rs` |
| J | Adapter runtime | Speak each protocol | `HardwareAdapter` trait; JSON-RPC host; signed manifests (ADR-0015) | Per adapter | Panics contained; timeouts become `UNKNOWN_OUTCOME` | Manifest permissions; adapters never see tokens | conformance L0–L3 |
| K | Audit and evidence | What happened, provably | Hash-chained audit, receipts, `audit.verify`, insights | Journal (size-limited, ADR-0021) | Refuses non-critical writes in the reserve | Append-only chain | `engine/tests/storage.rs`, `openmhs-engine/src/journal.rs` (unit) |
| L | Simulation and conformance | Test without hardware | `openmhs-sim`, fixtures, `openmhs conformance` | None | Faults on demand | Simulation only with `--sim` | `openmhs-conformance/tests/sims.rs`, `broken.rs` |
| M | Ecosystem | Others extend it | Adapter template, registry schema, Python SDK | Repository | — | Registry entries reviewed by a person | `tools/test_registry.py` |

## 5. Designs for what is missing

Each design is a proposal for review. Nothing in §5 is implemented.

### 5.1 Procedures (capability 8)

A procedure is a plan template stored by the operator. It has a name, a version, typed
parameters (a JSON Schema with units, from the descriptor value model), and a content hash.

- `procedures.add` (operator) stores it and audits it.
- `procedures.run {name, version, params}` (any caller) validates the parameters, expands
  them into an ordinary plan, and submits it as the caller. Every step stays the caller's
  command with every check.
- **A procedure grants nothing.** A caller without a grant for a step is refused at that
  step, as today.
- **Agents may propose procedures, never install them.** `procedures.propose` stores a
  draft that a human approves into a version.
- **Acceptance:**
  - parameter units are validated;
  - the same version and parameters expand to the same plan (hash recorded in the plan);
  - an agent cannot add or approve a procedure;
  - a revoked grant fails the run at the step, not before.

### 5.2 Parallel and conditional steps (capability 7)

Plans become a DAG.

- **Parallel.** A step runs when its dependencies have succeeded. Two steps run in
  parallel only if the capability graph shows no shared device or resource, all under the
  plan's one lease.
- **Conditional.** A condition is a precondition expression over fresh readings (the
  existing language, `spec/descriptor.md`), evaluated when the step would start.
- **Failure.**
  - An `UNKNOWN_OUTCOME` halts every step that depends on it, and lets independent
    branches finish their current step and then stop.
  - Compensation stays declared per step and is never inferred.
  - "Physical operations are not transactions": the plan reports exactly which effects
    happened.
- **Acceptance:**
  - parallel steps on a shared resource are rejected at preflight;
  - a halted branch leaves no queued command;
  - twin preflight covers the DAG;
  - no test passes by mocking a dispatch.

### 5.3 Model-agnostic harness (capability 9)

Python, `openmhs.agent`, over MCP or the SDK.

- **Providers.** A `ModelProvider` protocol (`complete(messages, tools) -> reply with
  tool calls`), with implementations for Anthropic, any OpenAI-compatible endpoint (which
  covers local llama.cpp, vLLM and Ollama servers), and a scripted provider for tests.
- **What the harness enforces:**
  - only tools the gateway lists;
  - a confirmation hook before tools annotated destructive;
  - a budget on steps and wall time;
  - a transcript of every tool call with its result;
  - plans proposed by the model go through `check_plan` before `submit_plan`;
  - nothing the model writes is executed as code.
- **Acceptance:**
  - the same scenario passes with the scripted provider and with one real provider;
  - swapping providers changes no OpenMHS code;
  - a model asking for an ungranted action is refused by the gateway, not by the harness.

### 5.4 Discovery (capability 1)

An optional adapter method, `discover(config) -> candidates`. Sources:
- mDNS for MQTT and HTTP devices;
- a Modbus unit-id scan on a configured gateway;
- SCPI `*IDN?` on configured ports;
- ROS 2 graph introspection.

**Candidates are proposals:**
- `devices.discover` (operator only) lists them, with the profile they would use.
- Registering one is a separate operator action, and grants nothing.

**Never:**
- auto-registration;
- probing outside the configured interfaces;
- any write during discovery.

**Acceptance:** discovery against each protocol simulator finds it; nothing is registered
without an operator action; a discovery run sends no write.

### 5.5 Semantic operations (capability 3)

An optional descriptor `semantic` field per capability, from a small, versioned
vocabulary, for example:
- `power_supply.configure_voltage`
- `instrument.capture_measurement`
- `sensor.observe_temperature`

`find_capabilities` and MCP descriptions use it. Authorization stays on the capability
name, so an alias never widens a grant. Dangerous raw writes are not exposed as tools at
all: every tool call is a descriptor-checked command.

### 5.6 Federation (capability 13)

**Design only.** The roadmap gates it on three independent single-gateway sites (M20).

- **Authority.** Each gateway owns its devices. Command authority never leaves the owning
  gateway.
- **Identity.** A gateway has an Ed25519 identity. Peers are configured by key, never
  discovered into trust.
- **What crosses between gateways:**
  - descriptors and availability, signed by the owner;
  - events, read-only, under the receiving principal's `subscribe` grants on the owner;
  - forwarded commands, which the owner authorizes with its own policy against a
    principal it knows.
- **Leases** across gateways carry a fencing token issued by the owner. The owner rejects
  a stale token.
- **Partition.** Remote devices are shown `UNREACHABLE`. No command is queued across a
  partition. A forwarded command whose answer is lost is `UNKNOWN_OUTCOME` on both sides,
  and is reconciled on the owner.
- **Audit.** Each gateway keeps its own chain. Peers exchange checkpoint hashes (storage
  RFC §6) so that a rewrite is detectable.
- **Local-first.** No gateway needs another, or a cloud, to stop or to operate its own
  devices.
- **Acceptance (when built):** a partition never produces a duplicate effect; a stale
  fencing token is refused; one gateway's policy can't authorize another's devices.

## 6. Milestones

These extend [NEXT_MILESTONE_ROADMAP.md](NEXT_MILESTONE_ROADMAP.md). The roadmap's ordering
stands: release (M11) and the ESP32 validation (M12) come first.

| Milestone | Delivers | Acceptance criteria |
|---|---|---|
| M17a: state fabric and selection (this change) | ADR-0023, `events.poll`, `watches.*`, `find_capabilities`, MCP, CLI and Python | The tests listed in ADR-0023 and in §2 rows 5–6, all passing |
| M17b: electronics lab demo | A simulated electronic load or DMM. A scripted agent that discovers, selects, plans, requests approval, configures, watches, detects an injected anomaly, recovers within its grants, reconciles an uncertain write, and writes an evidence report from receipts and the audit | Every command in the report has a receipt id that `commands.get` returns. The report states each fault injected. No dispatch is mocked |
| M18: procedures | §5.1 | **Done:** §5.1 acceptance met (units checked, deterministic expansion with the hash in the plan, agents cannot add or approve, a step outside the grants is refused) |
| M19a: harness | §5.3 | **Done** except the real-provider run: the same scenarios pass with scripted, OpenAI-compatible (fake server) and Anthropic (fake client) providers; the gateway, not the harness, refuses |
| M20a: DAG plans | §5.2 | **Done**: conditional steps (ADR-0024) and opt-in parallel steps (ADR-0025) |
| M20b: discovery | §5.4 | **SCPI, Modbus TCP, HTTP and MQTT done** (read-only, registers nothing); mDNS and the API remain |
| M20c: federation | §5.6, after an RFC and review | §5.6 acceptance |
| Benchmarks (**first slice done**, `docs/benchmarks.md`) | `openmhs bench` with wall-clock measurements: events per second through `events.poll`, watch sampling jitter, command latency with N watches running, startup with N offline devices | Numbers committed with the machine they ran on. Nothing inferred from virtual time |

## 7. The smallest complete system

**Question:** What is the smallest complete OpenMHS system we can ship that proves an
independent agent can discover, coordinate and operate several devices through one
reliable, model-agnostic runtime?

**Answer:** one gateway process, the three simulated devices, and the MCP server, driven by
a scripted agent and by any MCP-capable model. Concretely:

1. **Discover, select, understand.** The agent discovers and selects with `list_devices`,
   `find_capabilities` and `describe_device`.
2. **Act on several devices.** It acts through the one execution kernel, with policy,
   approvals, limits, journaling and readback. It coordinates more than one device in a
   plan under a lease.
3. **Observe.** It follows the result through `watch_state` and `poll_events`.
4. **Handle failure.** It meets an injected fault and handles it within its grants:
   reconcile an `UNKNOWN_OUTCOME`, or stop and ask.
5. **Learn and prove.** It learns from `get_insights` in a second session. The whole run
   verifies with `openmhs audit verify`.

Everything in that list is now implemented and tested, piece by piece. What is not yet
built is the single end-to-end scenario that runs it all at once and writes the evidence
report (M17b), and a measurement-instrument simulator to make it an electronics test
rather than a power-supply test. That scenario is the next thing to build. It needs no
new kernel features, which is the evidence that the runtime is complete enough to be its
foundation.

It proves nothing about physical hardware. M12 does that.
