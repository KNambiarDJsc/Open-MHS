# Graph Report - Open-MHS  (2026-10-28)

## Corpus Check
- 434 files · ~347,277 words
- Verdict: corpus is large enough that graph structure adds value.
- Unclassified: 19 file(s) not represented in the graph (top: (none) 8, .toml 4, .css 2)

## Summary
- 6153 nodes · 14564 edges · 272 communities (255 shown, 17 thin omitted)
- Extraction: 96% EXTRACTED · 4% INFERRED · 0% AMBIGUOUS · INFERRED: 639 edges (avg confidence: 0.83)
- Token cost: 0 input · 0 output

## Graph Freshness
- Built from commit: `e14485cf`
- Run `git rev-parse HEAD` and compare to check if the graph is stale.
- Run `graphify update .` after code changes (no API cost).

## Community Hubs (Navigation)
- pathlib
- Client
- __init__.py
- cli.rs
- ClientError
- openmhs-api/src/lib.rs
- server.rs
- ADR-0004: UCUM unit codes, exact match or explicit conversion, never inference
- openmhs-mqtt/src/lib.rs
- Gateway
- json
- src/tls.rs
- axis.rs
- runner.rs
- output.rs
- openmhs-mqtt/src/sim.rs
- validate.rs
- openmhs-http/src/lib.rs
- store.rs
- Runner
- psu.rs
- openmhs-scpi/src/link.rs
- ValueSpec
- CommandState
- EngineError
- README.md
- registry_lib.py
- MqttSim
- setup
- openmhs-modbus/src/lib.rs
- profiles.rs
- Ros2Adapter
- adr/README.md
- openmhs-mcp/src/lib.rs
- thermo.rs
- connect-a-device.md
- Caller
- Operation
- Engine
- JournalError
- deploy
- protocol.rs
- Timestamp
- constraints.rs
- with
- engine.rs
- openmhs-scpi/src/lib.rs
- openmhs-gateway/tests/availability.rs
- Journal
- Clock
- openmhs-cli/src/lib.rs
- Counter
- PsuSim
- guidance.rs
- adapter.py
- btreemap
- mcp.rs
- Engine
- openmhs-engine/tests/common/mod.rs
- Policy
- src/trust.rs
- manifest.rs
- Value
- openmhs-engine/tests/availability.rs
- plan.rs
- openmhs-gateway/src/lib.rs
- Source
- _Server
- openmhs-mqtt/tests/tls.rs
- octoprint/manifest.json
- NEXT_MILESTONE_ROADMAP.md
- Fault
- Permissions
- temperature
- OpcUaTests
- insights.rs
- Cmd
- broken.rs
- openmhs-modbus/src/sim.rs
- main
- Capability
- Engine
- credentials.rs
- ThroughTheGateway
- src/process.rs
- gateway.rs
- tests/plans.rs
- Status
- OpenMhsServer
- openmhs-scpi/src/sim.rs
- tests/approvals.rs
- Schema
- descriptor.rs
- ros2_adapter.py
- AdapterCommand
- HardwareAdapter
- tests/trust.rs
- lab
- plans_http.rs
- src/plans.rs
- src/events.rs
- openmhs-http/tests/tls.rs
- OpcUaAdapter
- demo.rs
- tests/process.rs
- ok
- RpcError
- scaffold.rs
- stale_readback.rs
- Node
- LlmAgentWithoutKey
- SimControl
- selection.rs
- units.rs
- 1. Hardware execution
- adapter/src/lib.rs
- HttpSim
- OpenMHS platform architecture: current, target, and what is missing
- Effect
- approvals_http.rs
- workcell.rs
- 2. Milestones
- Lease
- template.rs
- LeaseRequest
- StepVerdict
- journal.rs
- super
- Adoption, sustainability and strategic futures
- Broker
- with
- check_comparison
- leases_http.rs
- path
- client_command
- Role
- fault.rs
- openmhs-types
- Observer
- ConformanceLevel
- untrusted.rs
- Threats
- 3. Methods
- run
- stop_lane.rs
- ProcessAdapter
- .transition_tx
- Writing an OpenMHS adapter
- attributes
- Code registry
- psu-sim/manifest.json
- storage.rs
- spec_fixtures.rs
- ROS 2 (observation only)
- M8: identity and trust
- Landscape: AI agents ↔ physical hardware (as of 2026-10-23)
- Troubleshooting
- FirmwareTests
- body
- ADR-0023: The state fabric: events, watches, and the `subscribe` operation
- RFC: Bounded storage, audit lifecycle and fleet-scale scheduling
- openmhs-http/src/sim.rs
- Script
- Scripted
- PlanState
- Independent hardware validation plan
- body
- host
- .from_home
- Health
- Connect your own device
- HardwareBench: research plan
- jobs
- steps
- PlanDecision
- start_hosted
- ADR-0013: Conformance suite and adapter scaffolding
- HTTP profiles
- .initialize
- Decision
- Decision
- M10 release audit
- M12 runbook: bounded, independently observed ESP32 test
- OpenMHS Playground
- AdapterError
- PlansCmd
- ErrorObject
- Connecting an AI agent over MCP
- Releasing OpenMHS
- RFC NNNN: Title
- steps
- GH_TOKEN
- build-local-artifacts
- .__init__
- ApprovalsCmd
- ADR-0001: Single trusted gateway process; all interfaces are clients
- principal.rs
- ADR-0014: Protocol adapters driven by profiles
- Core concepts
- M10_TEST_REPORT.md
- ADR-0003: Small typed authorization engine now; Cedar re-evaluated in M8
- Transport security
- Current state audit
- jobs
- steps
- with
- Authorization
- Command lifecycle
- Plans (v0.1)
- Stager
- CommandsCmd
- openmhs-cli/src/values.rs
- ADR-0005: Local API is JSON-RPC 2.0 over HTTP on loopback, token-authenticated
- openmhs-host/tests/conformance.rs
- ADR-0008: Rust for the trusted core, Python for clients and out-of-process adapters
- Protocol profiles
- SCPI profiles
- ADR-0010: Engine execution model
- ADR-0022: Insights: learning from the journal, never from a back door
- SUMMARY.md
- `openmhs` command line
- Configuration
- ADR-0011: Gateway API, roles and home directory
- ADR-0015: Out-of-process adapters and the adapter host
- Installation
- GitHub presentation and recommended settings
- main.c
- with
- rust
- plan
- Conformance
- jsonschema
- LeasesCmd
- TokensCmd
- Lookup
- ADR-0017: Identity and trust
- ADR-0002: Canonical command lifecycle with explicit uncertainty
- ADR-0006: MCP via the official Rust SDK (`rmcp`), as a gateway client
- ADR-0020: Readback must be measured after the command was sent
- RoundTrip
- contact_links
- Governance
- Line
- AdapterCmd
- GrantsCmd
- TrustCmd
- Modbus TCP profiles
- ADR-0021: Journal quota and critical reserve
- The adapter host
- LeaseStatus
- ADR-0007: Independent of Anthropic MHS; compatibility via a future façade
- uses
- ros2
- env
- Progress
- Adapter permission manifest (v0.1)
- Capability graph and topology (v0.1)
- OpenMHS website
- vercel.json
- DevicesCmd
- ProfileCmd
- SimCmd
- PULL_REQUEST_TEMPLATE.md
- steps
- on
- main
- AuditCmd
- main
- OpenMHS Specification v0.1 (draft)
- adapter/README.md
- openmhs

## God Nodes (most connected - your core abstractions)
1. `Value` - 158 edges
2. `EngineError` - 129 edges
3. `Timestamp` - 110 edges
4. `Engine` - 85 edges
5. `Client` - 84 edges
6. `Capability` - 81 edges
7. `Caller` - 80 edges
8. `ClientError` - 79 edges
9. `Operation` - 79 edges
10. `JournalError` - 70 edges

## Surprising Connections (you probably didn't know these)
- `What each answer means` --references--> `ok()`  [INFERRED]
  docs/adapters/http.md → firmware/esp32-mqtt-node/test_node.py
- `No core changes` --references--> `serve()`  [INFERRED]
  docs/adr/0014-protocol-adapters.md → python/openmhs/adapter.py
- `2. Agent usability` --references--> `serve()`  [INFERRED]
  docs/strategy/ARCHITECTURE_GAP_ANALYSIS.md → python/openmhs/adapter.py
- `Table (v0.1)` --references--> `temperature()`  [INFERRED]
  spec/units.md → adapters/python/octoprint/octoprint_adapter.py
- `Consequences` --references--> `number()`  [INFERRED]
  docs/adr/0009-descriptor-model.md → adapters/python/psu-sim/psu_sim.py

## Import Cycles
- 1-file cycle: `crates/openmhs-policy/src/glob.rs -> crates/openmhs-policy/src/glob.rs`
- 1-file cycle: `crates/openmhs-types/src/principal.rs -> crates/openmhs-types/src/principal.rs`
- 2-file cycle: `crates/openmhs-mcp/src/lib.rs -> crates/openmhs-mcp/src/plans.rs -> crates/openmhs-mcp/src/lib.rs`
- 2-file cycle: `crates/openmhs-engine/src/engine.rs -> crates/openmhs-engine/src/engine/availability.rs -> crates/openmhs-engine/src/engine.rs`
- 2-file cycle: `crates/openmhs-engine/src/engine.rs -> crates/openmhs-engine/src/engine/events.rs -> crates/openmhs-engine/src/engine.rs`

## Communities (272 total, 17 thin omitted)

### Community 0 - "pathlib"
Cohesion: 0.06
Nodes (21): free_port(), main(), main(), cli(), node_log(), say(), step(), available() (+13 more)

### Community 1 - "Client"
Cohesion: 0.04
Nodes (9): Client, _request(), _with_reason(), Approval, Lease, Plan, Receipt, openmhs (Python client) (+1 more)

### Community 2 - "__init__.py"
Cohesion: 0.05
Nodes (43): 7. What was implemented on this branch, GatewayError, GatewayUnreachableError, NotRunningError, OpenMHSError, ProtocolError, UnauthorizedError, AdapterResult (+35 more)

### Community 3 - "cli.rs"
Cohesion: 0.13
Nodes (32): an_operator_resolves_an_unknown_outcome_with_a_reason(), approved_write_is_verified(), BIN, check_reports_what_a_submission_would_get_and_runs_nothing(), DENIED, denied_write_on_stale_telemetry(), denied_write_out_of_range(), denied_write_without_grant() (+24 more)

### Community 4 - "ClientError"
Cohesion: 0.06
Nodes (7): Client, ClientError, Connect, NotRunning, Protocol, Rpc, Unauthorized

### Community 5 - "openmhs-api/src/lib.rs"
Cohesion: 0.03
Nodes (76): ApprovalActionParams, ApprovalRequestParams, APPROVALS_APPROVE, APPROVALS_DENY, APPROVALS_GET, APPROVALS_LIST, APPROVALS_REQUEST, APPROVALS_REVOKE (+68 more)

### Community 6 - "server.rs"
Cohesion: 0.04
Nodes (18): id(), CommandIdParams, EventsPollParams, SimFaultResult, WatchParams, bearer(), DEFAULT_LIST_LIMIT, engine_error() (+10 more)

### Community 7 - "ADR-0004: UCUM unit codes, exact match or explicit conversion, never inference"
Cohesion: 0.33
Nodes (5): ADR-0004: UCUM unit codes, exact match or explicit conversion, never inference, Alternatives considered, Consequences, Context, Decision

### Community 8 - "openmhs-mqtt/src/lib.rs"
Cohesion: 0.07
Nodes (28): Ack, Done, Refused, Unreadable, AckRule, check_pointer(), check_topic(), CommandRule (+20 more)

### Community 9 - "Gateway"
Cohesion: 0.05
Nodes (25): Tests, Quantity, binary(), default_grants(), Gateway, grant(), Approval, Concurrency (+17 more)

### Community 10 - "json"
Cohesion: 0.06
Nodes (20): binary(), default_grants(), grant(), Lab, main(), run(), say(), session() (+12 more)

### Community 11 - "src/tls.rs"
Cohesion: 0.07
Nodes (19): a_ca_file_and_client_certificate_load(), bad_files_are_named(), ca(), check_plaintext(), create(), ExpectedName, EXPIRED, GOOD (+11 more)

### Community 12 - "axis.rs"
Cohesion: 0.08
Nodes (23): ADAPTER_ID, axis(), AxisHandle, AxisTruth, BASE_LATENCY_MS, DESCRIPTOR, descriptor_is_valid_and_matches_adapter(), DEVICE_ID (+15 more)

### Community 13 - "runner.rs"
Cohesion: 0.07
Nodes (28): run(), render(), achieved_level(), check(), CheckResult, Environment, FixtureInfo, Report (+20 more)

### Community 14 - "output.rs"
Cohesion: 0.06
Nodes (32): approval(), approvals(), audit_entries(), audit_verify(), clock_time(), clock_time_keeps_three_digits(), created_token(), device_statuses() (+24 more)

### Community 15 - "openmhs-mqtt/src/sim.rs"
Cohesion: 0.09
Nodes (5): LINK_CHECK, MAX_PACKET, Stop, TELEMETRY, matches()

### Community 16 - "validate.rs"
Cohesion: 0.08
Nodes (31): Relationship, RelationshipKind, Contains, DependsOn, Frame, SharesResource, a_topology_must_name_registered_devices_and_follow_the_rules(), check_topology() (+23 more)

### Community 17 - "openmhs-http/src/lib.rs"
Cohesion: 0.08
Nodes (20): at(), check_path(), check_pointer(), Connection, Endpoint, HttpAdapter, judge(), measured_at() (+12 more)

### Community 18 - "store.rs"
Cohesion: 0.12
Nodes (28): sha256_hex(), credentials_expire_exactly_at_their_time(), ids_are_not_derived_from_the_secret(), issuing_refuses_impersonation_and_privilege_mixing(), new(), new_id(), new_token(), NewToken (+20 more)

### Community 19 - "Runner"
Cohesion: 0.22
Nodes (7): applied(), command_finding(), evidence(), Finding, Runner, verification_finding(), wire()

### Community 20 - "psu.rs"
Cohesion: 0.09
Nodes (21): Inner, Faults, ADAPTER_ID, amps(), BASE_LATENCY_MS, boolean(), cmd(), DESCRIPTOR (+13 more)

### Community 21 - "openmhs-scpi/src/link.rs"
Cohesion: 0.10
Nodes (24): a_busy_instrument_is_sent_nothing_new_until_it_catches_up(), a_caller_that_gives_up_does_not_cut_the_exchange_in_half(), a_late_answer_to_the_sync_query_does_not_end_the_resync_early(), a_late_reply_is_never_taken_for_the_next_answer(), Conn, exchange(), IDN, instrument() (+16 more)

### Community 22 - "ValueSpec"
Cohesion: 0.14
Nodes (28): bounds(), check_reading(), generated_values_differ_from_the_current_one(), midpoint(), neutral_value(), next_value(), number(), number_in() (+20 more)

### Community 23 - "CommandState"
Cohesion: 0.05
Nodes (34): CommandState, Accepted, .ALL, Applied, Approved, AwaitingApproval, Cancelled, Created (+26 more)

### Community 24 - "EngineError"
Cohesion: 0.08
Nodes (29): Engine, parse_request(), plan_devices(), PlanRuntime, refused(), requests_are_checked_and_keys_are_the_plans(), settle(), Settled (+21 more)

### Community 25 - "README.md"
Cohesion: 0.04
Nodes (43): Code of Conduct, Enforcement, Expected behaviour, Scope, Unacceptable behaviour, Before you open a pull request, Commits and pull requests, Contributing to OpenMHS (+35 more)

### Community 26 - "registry_lib.py"
Cohesion: 0.06
Nodes (28): main(), adapters_table(), _checks(), document(), history_table(), _levels(), _link(), main() (+20 more)

### Community 27 - "MqttSim"
Cohesion: 0.18
Nodes (4): Broker, Client, matches(), MqttSim

### Community 28 - "setup"
Cohesion: 0.13
Nodes (45): allow_all(), assert_schema_valid(), grant(), setup(), setup_not_idempotent(), volts(), with(), inv01_discovered_device_is_not_writable() (+37 more)

### Community 29 - "openmhs-modbus/src/lib.rs"
Cohesion: 0.08
Nodes (15): Connection, echoes(), exception(), Failure, Down, Exception, Garbled, Lost (+7 more)

### Community 30 - "profiles.rs"
Cohesion: 0.10
Nodes (24): FORMAT_VERSION, load(), Loaded, Loaded<C, M>, Mapping, ProfileError, protocol_of(), Raw (+16 more)

### Community 31 - "Ros2Adapter"
Cohesion: 0.13
Nodes (7): convert(), bad(), field(), Ros2Adapter, stamp(), ValueTests, _extra()

### Community 32 - "adr/README.md"
Cohesion: 0.12
Nodes (9): ADR-0016: Transport security for networked adapters, Alternatives considered, Consequences, Context, Decision, Architecture Decision Records, Documentation site, How the documentation is versioned (+1 more)

### Community 33 - "openmhs-mcp/src/lib.rs"
Cohesion: 0.06
Nodes (19): CapabilitySummary, CheckArgs, CommandList, DescribeOutput, DeviceArgs, DeviceList, DeviceSummary, GetCommandArgs (+11 more)

### Community 34 - "thermo.rs"
Cohesion: 0.09
Nodes (16): Rng, ADAPTER_ID, BASE_C, celsius(), DESCRIPTOR, descriptor_is_valid_and_matches_adapter(), DEVICE_ID, Inner (+8 more)

### Community 35 - "connect-a-device.md"
Cohesion: 0.15
Nodes (4): Recorded run of the adapter guide, What the run changed, Guides, Real hardware

### Community 36 - "Caller"
Cohesion: 0.08
Nodes (20): default_kind(), describe_value(), Engine, origin(), a_scope_limits_device_capability_and_operation(), AuthMethod, Credential, InProcess (+12 more)

### Community 37 - "Operation"
Cohesion: 0.10
Nodes (24): GrantParams, grant(), grants(), everything(), grant_all(), WireCommand, allow_requires_matching_operation(), AuthzDecision (+16 more)

### Community 38 - "Engine"
Cohesion: 0.13
Nodes (4): Engine, Denied, lock(), scope_denied()

### Community 39 - "JournalError"
Cohesion: 0.15
Nodes (7): JournalError, IllegalTransition, Json, Refused, Sqlite, decode(), Journal

### Community 40 - "deploy"
Cohesion: 0.05
Nodes (39): runs-on, steps, default, description, environment, if, needs, permissions (+31 more)

### Community 41 - "protocol.rs"
Cohesion: 0.06
Nodes (36): AdapterErrorKind, DeviceFailure, Disconnected, Internal, MalformedResponse, NotSent, Timeout, Unsupported (+28 more)

### Community 42 - "Timestamp"
Cohesion: 0.08
Nodes (13): TokioClock, Record, Availability, Degraded, Offline, Online, DeviceStatus, LeaseHold (+5 more)

### Community 43 - "constraints.rs"
Cohesion: 0.16
Nodes (28): bad_quality_is_not_usable(), check_command(), check_read(), codes(), coerce(), coerce_unchecked(), convert(), converts_units_and_checks_range() (+20 more)

### Community 44 - "with"
Cohesion: 0.05
Nodes (38): runs-on, steps, name, url, publish, jobs, build, publish (+30 more)

### Community 45 - "engine.rs"
Cohesion: 0.06
Nodes (29): DEFAULT_APPROVAL_TTL_MS, MAX_APPROVAL_TTL_MS, MAX_CONCURRENT_PROBES, MAX_PROBE_BACKOFF_MS, probe_backoff_ms(), Device, EngineOptions, Evaluation (+21 more)

### Community 46 - "openmhs-scpi/src/lib.rs"
Cohesion: 0.10
Nodes (12): Command, Connection, decode(), encode(), parse_error(), plain(), PROTOCOL, ScpiAdapter (+4 more)

### Community 47 - "openmhs-gateway/tests/availability.rs"
Cohesion: 0.11
Nodes (15): a_device_that_goes_away_and_comes_back_is_followed(), a_switched_off_device_does_not_stop_the_gateway_or_the_others(), agent(), availability(), code(), config(), many_wedged_devices_do_not_make_the_gateway_start_one_timeout_at_a_time(), OFF (+7 more)

### Community 49 - "Clock"
Cohesion: 0.08
Nodes (18): greenhouse_node_reaches_its_declared_level(), load(), Profile, the_example_profile_loads_and_bad_mappings_are_named(), bench_psu_over_a_serial_line_reaches_its_declared_level(), bench_psu_over_tcp_reaches_its_declared_level(), PROFILE, save() (+10 more)

### Community 50 - "openmhs-cli/src/lib.rs"
Cohesion: 0.07
Nodes (30): ABOUT, adapter_keygen(), adapter_new(), adapter_sign(), Cli, Daemon, DENIED, ERROR (+22 more)

### Community 51 - "Counter"
Cohesion: 0.14
Nodes (4): Broken, Counter, CounterState, Retrier

### Community 52 - "PsuSim"
Cohesion: 0.07
Nodes (20): boolean(), number(), PsuSim, Configure a real server, Install, OPC UA (read-only), Readings, Security (+12 more)

### Community 53 - "guidance.rs"
Cohesion: 0.15
Nodes (23): an_unresolved_earlier_command_is_named(), check_view(), CheckView, command_view(), command_view_for(), CommandView, dedup(), detail() (+15 more)

### Community 54 - "adapter.py"
Cohesion: 0.04
Nodes (27): is_loopback(), _json(), check_server_certificate(), _is_ca(), _issued_by(), load_cert(), cert(), key() (+19 more)

### Community 55 - "btreemap"
Cohesion: 0.10
Nodes (16): CommandRequest, DEFAULT_DEADLINE_MS, Target, AdapterResult, Decision, Allow, Deny, RequireApproval (+8 more)

### Community 56 - "mcp.rs"
Cohesion: 0.17
Nodes (28): a_bad_token_is_a_tool_error_not_a_crash(), a_model_finds_the_device_that_can_do_what_it_needs(), a_model_watches_a_measurement_and_follows_its_command(), a_verified_change_is_confirmed_and_idempotent(), an_offline_device_is_visible_and_refused_with_a_next_step(), an_unconfirmed_stop_is_not_reported_as_stopped(), approval_flow_waits_for_a_human(), call() (+20 more)

### Community 57 - "Engine"
Cohesion: 0.18
Nodes (3): classify(), Engine, InFlight

### Community 58 - "openmhs-engine/tests/common/mod.rs"
Cohesion: 0.10
Nodes (9): AGENT, ENABLED, NotIdempotent, OPERATOR, PSU, SETPOINT, states(), TrapAdapter (+1 more)

### Community 59 - "Policy"
Cohesion: 0.27
Nodes (15): a_command_arrives_in_order_and_only_streamable_readings_are_delivered(), a_consumer_that_falls_behind_is_told_how_many_it_missed(), a_poll_waits_for_the_next_event_and_a_restart_resets_the_cursor(), a_watch_needs_subscribe_and_a_streamable_capability_and_stops_when_revoked(), a_watch_samples_until_it_expires_and_both_ends_are_audited(), agent(), availability_changes_are_streamed(), engine_with() (+7 more)

### Community 60 - "src/trust.rs"
Cohesion: 0.08
Nodes (28): MANIFEST_SCHEMA, OS_ENV, ALGORITHM, collect(), err(), generate_keypair(), hex(), key_id() (+20 more)

### Community 61 - "manifest.rs"
Cohesion: 0.10
Nodes (22): AdapterInfo, Allow, Artifact, CapabilityPolicy, Endpoints, Filesystem, Limits, Manifest (+14 more)

### Community 62 - "Value"
Cohesion: 0.08
Nodes (33): booleans_enums_and_integers(), check(), from_json(), from_number(), from_text(), integral(), number(), numbers_keep_the_declared_unit_and_range() (+25 more)

### Community 63 - "openmhs-engine/tests/availability.rs"
Cohesion: 0.12
Nodes (19): a_device_that_comes_back_is_used_without_a_restart(), a_device_that_does_not_answer_is_registered_offline(), a_disconnect_seen_in_use_marks_the_device_offline(), a_hung_device_has_at_most_one_outstanding_probe(), a_stop_is_never_refused_for_availability(), a_sweep_probes_a_dead_device_less_and_less_often_and_a_live_one_at_the_base_rate(), a_thousand_offline_devices_start_and_one_sweep_brings_them_all_online(), agent() (+11 more)

### Community 64 - "plan.rs"
Cohesion: 0.13
Nodes (21): codes(), cycles_are_refused(), DEFAULT_PLAN_LEASE_TTL_MS, dependencies_first_then_declaration_order(), execution_order(), LeaseCheck, MAX_PLAN_STEPS, OnFailure (+13 more)

### Community 65 - "openmhs-gateway/src/lib.rs"
Cohesion: 0.09
Nodes (16): runtime_path(), DEFAULT_HEALTH_INTERVAL_MS, DEFAULT_JOURNAL_RESERVE_BYTES, DEFAULT_OPERATOR, DEFAULT_PORT, GatewayConfig, GatewayError, AlreadyRunning (+8 more)

### Community 66 - "Source"
Cohesion: 0.07
Nodes (18): Source, Adapter, Device, Gateway, Simulator, apply(), FakeLamp, Lamp (+10 more)

### Community 67 - "_Server"
Cohesion: 0.11
Nodes (5): H1. One unreachable device stops the gateway — fixed here, Adapter, _BadParams, _param(), _Server

### Community 68 - "openmhs-mqtt/tests/tls.rs"
Cohesion: 0.24
Nodes (17): load(), a_broker_that_wants_a_client_certificate_gets_one(), a_certificate_for_another_name_is_refused(), a_certificate_from_an_untrusted_ca_is_refused(), a_trusted_certificate_connects(), adapter(), an_expired_certificate_is_refused(), pki() (+9 more)

### Community 69 - "octoprint/manifest.json"
Cohesion: 0.07
Nodes (29): adapter, conformance, id, version, capabilities, allow, config, api_key_env (+21 more)

### Community 70 - "NEXT_MILESTONE_ROADMAP.md"
Cohesion: 0.10
Nodes (20): Final decision memo, 1. Developer experience audit, 2. Community adapter SDK, 3. Adapter registry, 4. Governance, 5. Contributor roadmap, 6. First 90 days, in order, Open-source ecosystem plan (+12 more)

### Community 71 - "Fault"
Cohesion: 0.10
Nodes (9): Fault, .ALL, Garbled, LinkDown, LostAck, NoResponse, Refuse, Fixture (+1 more)

### Community 72 - "Permissions"
Cohesion: 0.14
Nodes (7): ConfigError, Settings, ConfigTests, _check_path(), _endpoint(), Permissions, PermissionTests

### Community 73 - "temperature"
Cohesion: 0.14
Nodes (8): connection_state(), temperature(), MappingTests, state(), Known limitations, Quantities and units, Rules, Table (v0.1)

### Community 74 - "OpcUaTests"
Cohesion: 0.14
Nodes (5): ChamberSim, config(), descriptor(), OpcUaTests, SecureServer

### Community 75 - "insights.rs"
Cohesion: 0.12
Nodes (24): a_full_listing_is_reported_as_possibly_truncated(), Acc, CapabilityInsights, confirmed_values_error_and_latency_come_only_from_verified_commands(), denied(), EVIDENCE, finish(), Insights (+16 more)

### Community 76 - "Cmd"
Cohesion: 0.07
Nodes (28): Cmd, Adapter, Approvals, Approve, Audit, Commands, Conformance, Demo (+20 more)

### Community 77 - "broken.rs"
Cohesion: 0.15
Nodes (25): a_correct_non_idempotent_adapter_reaches_l2(), an_adapter_that_is_always_healthy_fails_l1(), an_adapter_that_reports_the_wrong_unit_fails_l1(), an_adapter_that_retries_a_non_idempotent_command_fails_l2(), an_adapter_that_serves_cached_readings_when_disconnected_fails_l1(), an_adapter_that_swallows_errors_fails_l2(), an_adapter_that_turns_refusals_into_failures_fails_l2(), an_adapter_with_the_wrong_id_fails_l0_and_stops() (+17 more)

### Community 78 - "openmhs-modbus/src/sim.rs"
Cohesion: 0.07
Nodes (26): Kind, F32, I16, I32, U16, U32, reg(), Register (+18 more)

### Community 79 - "main"
Cohesion: 0.16
Nodes (8): main(), save(), Demo, main(), say(), step(), octoprint_executable(), OctoPrintServer

### Community 80 - "Capability"
Cohesion: 0.19
Nodes (5): deadline(), Problem, Refused, Setup, Capability

### Community 81 - "Engine"
Cohesion: 0.21
Nodes (4): denied(), Engine, fingerprint(), canonical_json()

### Community 82 - "credentials.rs"
Cohesion: 0.20
Nodes (23): a_credential_cannot_exceed_its_operations_or_capabilities(), a_credential_cannot_reach_a_device_outside_its_scope(), a_credential_without_scopes_authorizes_nothing(), a_revoked_credential_is_rejected_immediately(), a_scope_never_replaces_a_grant(), a_valid_scoped_credential_can_read(), an_agent_cannot_claim_another_identity_or_issue_credentials(), an_expired_credential_is_rejected() (+15 more)

### Community 84 - "src/process.rs"
Cohesion: 0.06
Nodes (25): CallError, Lost, NotSent, Rpc, Timeout, Conn, EXECUTE_GRACE_MS, EXIT_GRACE (+17 more)

### Community 85 - "gateway.rs"
Cohesion: 0.12
Nodes (13): CreateTokenParams, client_disconnect_does_not_abandon_the_command(), denial_messages_do_not_repeat_the_code(), embedded_adapters_are_registered(), psu_write_grant(), requests_need_a_token_and_json(), shutdown_removes_the_runtime_file_and_releases_the_home(), start() (+5 more)

### Community 86 - "tests/plans.rs"
Cohesion: 0.16
Nodes (16): A, a_plan_runs_its_steps_in_order_as_ordinary_commands(), audit_kinds(), axis(), AXIS, B, denied_code(), invalid_codes() (+8 more)

### Community 87 - "Status"
Cohesion: 0.13
Nodes (13): Event, Closed, Stop, State, Quarantined, Restarting, Running, Stopped (+5 more)

### Community 88 - "OpenMhsServer"
Cohesion: 0.31
Nodes (6): client_error(), CommandIdArgs, NoArgs, OpenMhsServer, structured(), text_error()

### Community 90 - "tests/approvals.rs"
Cohesion: 0.28
Nodes (23): a_held_command_gets_a_pending_approval_bound_to_its_fingerprint(), a_modified_command_is_rejected(), a_pre_approval_expires(), a_revoked_approval_is_rejected(), a_spent_approval_is_not_reusable(), an_approval_cannot_be_used_twice(), an_approval_requested_ahead_runs_exactly_its_command(), an_expired_approval_is_rejected() (+15 more)

### Community 91 - "Schema"
Cohesion: 0.10
Nodes (20): COMMAND_SCHEMA, COMMON_SCHEMA, CONFORMANCE_REPORT_SCHEMA, DESCRIPTOR_SCHEMA, ERROR_SCHEMA, EVENT_SCHEMA, parse(), RECEIPT_SCHEMA (+12 more)

### Community 92 - "descriptor.rs"
Cohesion: 0.09
Nodes (25): compare(), condition_holds(), AdapterBinding, CompareOp, Eq, Ge, Gt, In (+17 more)

### Community 93 - "ros2_adapter.py"
Cohesion: 0.10
Nodes (6): ConfigError, message_type(), Settings, config(), SettingsTests, case()

### Community 94 - "AdapterCommand"
Cohesion: 0.10
Nodes (10): AdapterCommand, AdapterError, ExecuteReport, Applied, Rejected, Progress, ProgressEvent, Accepted (+2 more)

### Community 95 - "HardwareAdapter"
Cohesion: 0.11
Nodes (5): HardwareAdapter, Engine, Twin, TWIN_APPROVER, verdict()

### Community 96 - "tests/trust.rs"
Cohesion: 0.27
Nodes (14): a_correctly_signed_manifest_is_accepted(), a_forged_key_id_does_not_borrow_trust(), a_launch_command_outside_the_signature_is_rejected(), a_malformed_signature_is_rejected(), a_missing_artifact_is_rejected(), a_modified_executable_is_rejected(), a_tampered_manifest_is_rejected(), a_valid_signature_does_not_override_the_manifests_permissions() (+6 more)

### Community 97 - "lab"
Cohesion: 0.27
Nodes (21): a_failed_compensation_waits_for_a_decision(), a_plan_leases_what_it_touches(), a_step_held_for_approval_pauses_the_plan_until_an_approver_acts(), a_step_precondition_that_does_not_hold_stops_the_step_before_it_is_sent(), an_approver_can_choose_to_compensate_or_abort(), ask_waits_and_an_approver_decides(), compensate_at_every_step(), halt_at_every_step() (+13 more)

### Community 98 - "plans_http.rs"
Cohesion: 0.19
Nodes (19): a_plan_over_http_runs_is_private_and_is_audited(), a_plan_runs_after_its_preflight(), agent(), approving_a_held_step_over_http_finishes_the_plan(), audit_kinds(), code(), ENABLED, grant() (+11 more)

### Community 99 - "src/plans.rs"
Cohesion: 0.14
Nodes (14): standard_formats(), CancelPlanArgs, GetPlanArgs, is_error(), plan_check_view(), PlanArgs, PlanCheckView, PlanInput (+6 more)

### Community 100 - "src/events.rs"
Cohesion: 0.07
Nodes (28): Ring, CancelWatchArgs, event_view(), events_output(), EventsOutput, EventView, PollArgs, watch_view() (+20 more)

### Community 101 - "openmhs-http/tests/tls.rs"
Cohesion: 0.13
Nodes (20): load(), the_example_profile_loads_and_bad_mappings_are_named(), led_controller_reaches_its_declared_level(), a_certificate_for_another_name_is_refused(), a_certificate_from_an_untrusted_ca_is_refused(), a_failed_handshake_means_the_command_was_not_sent(), a_server_that_wants_a_client_certificate_gets_one(), a_trusted_certificate_connects() (+12 more)

### Community 102 - "OpcUaAdapter"
Cohesion: 0.13
Nodes (5): browse(), walk(), main(), OpcUaAdapter, rfc3339()

### Community 103 - "demo.rs"
Cohesion: 0.16
Nodes (12): AGENT, AXIS, PSU, run(), scenario(), SETPOINT, SLEW_WINDOW, temp_home() (+4 more)

### Community 104 - "tests/process.rs"
Cohesion: 0.16
Nodes (17): a_changed_descriptor_after_restart_is_quarantined(), a_crash_after_dispatch_is_unknown_outcome_and_never_retried(), a_crash_during_a_read_is_contained_and_the_process_restarted(), a_crash_loop_is_quarantined(), a_hung_process_is_killed_and_its_command_is_uncertain(), an_adapter_cannot_submit_commands_or_name_a_principal(), calls_outside_the_manifest_never_reach_the_process(), dose() (+9 more)

### Community 105 - "ok"
Cohesion: 0.08
Nodes (24): Connection, How it behaves, Limits, Mapping, MQTT profiles, Tested against, The simulator, Evidence (+16 more)

### Community 106 - "RpcError"
Cohesion: 0.24
Nodes (14): RpcError, RpcResponse, detached(), dispatch(), forbidden(), gateway_error(), io_error(), ok() (+6 more)

### Community 107 - "scaffold.rs"
Cohesion: 0.14
Nodes (19): cargo_toml(), create(), dep(), FILES, it_never_writes_into_a_non_empty_directory(), names_are_replaced_everywhere(), OPENMHS_DEPS, OPENMHS_DEV_DEPS (+11 more)

### Community 108 - "stale_readback.rs"
Cohesion: 0.18
Nodes (7): a_clock_allowance_admits_a_reading_slightly_older_than_dispatch(), agent(), an_older_value_that_contradicts_the_command_does_not_fail_it(), an_older_value_that_happens_to_match_does_not_verify(), Cached, reconciliation_does_not_conclude_from_a_reading_older_than_the_command(), setup_cached()

### Community 111 - "SimControl"
Cohesion: 0.18
Nodes (7): run(), SimFixture, spec(), apply_fault(), SimControl, SimDevice, virtual_lab()

### Community 112 - "selection.rs"
Cohesion: 0.21
Nodes (12): CapabilityMatch, CapabilityQuery, finds_what_can_be_streamed_in_a_unit(), lab(), names(), select(), Selection, side_effects_and_availability_narrow_the_choice() (+4 more)

### Community 113 - "units.rs"
Cohesion: 0.14
Nodes (12): affine(), ConversionError, QuantityMismatch, UnknownUnit, convert(), every_quantity_has_a_canonical_unit_with_unit_scale(), QUANTITIES, same_quantity_pair() (+4 more)

### Community 114 - "1. Hardware execution"
Cohesion: 0.11
Nodes (19): 1. Hardware execution, 2. Agent usability, 3. Interoperability, 4. Security, 5. Developer experience (summary; detail in the ecosystem plan), Architecture gap analysis, Concurrency, cancellation, leases, H10. Registration was sequential — fixed here (+11 more)

### Community 115 - "adapter/src/lib.rs"
Cohesion: 0.20
Nodes (8): ADAPTER_ID, DESCRIPTOR, flag(), integer(), LampAdapter, malformed(), number(), quantity()

### Community 116 - "HttpSim"
Cohesion: 0.21
Nodes (7): Profile, HttpSim, Reply, Json, Raw, Silent, put()

### Community 117 - "OpenMHS platform architecture: current, target, and what is missing"
Cohesion: 0.12
Nodes (16): 1. What the benchmark is, and is not, 2. Verified gap matrix, 3.1 Current (after this change), 3.2 Target, 3. Architecture, 4. Subsystem contracts, 5.1 Procedures (capability 8), 5.2 Parallel and conditional steps (capability 7) (+8 more)

### Community 118 - "Effect"
Cohesion: 0.17
Nodes (23): A, a_caller_can_only_lease_what_it_could_operate(), a_lease_ends_at_its_deadline(), a_lease_taken_while_a_command_waits_for_approval_stops_it(), AXIS, B, code(), concurrent_acquisitions_grant_exactly_one() (+15 more)

### Community 119 - "approvals_http.rs"
Cohesion: 0.22
Nodes (14): a_held_command_is_approved_through_its_approval(), agent(), agent_credentials_cannot_administer(), code(), deny_and_revoke_end_the_wait(), enable(), ENABLED, every_method_requires_authentication() (+6 more)

### Community 120 - "workcell.rs"
Cohesion: 0.10
Nodes (25): a_supply_that_refuses_stops_the_cell_before_the_axis(), an_axis_that_refuses_is_compensated_back_to_safe(), an_unknown_outcome_waits_for_reconcile_and_a_decision(), AXIS, Lab, lab(), near(), PLAN (+17 more)

### Community 121 - "2. Milestones"
Cohesion: 0.10
Nodes (20): 1. Ranking, 2. Milestones, 3. Technical-debt inventory, 4. Prioritized plan, 5. Architecture diagrams, 6. Competitive positioning (summary), 7. Community contributor roadmap, Current (what the code does) (+12 more)

### Community 123 - "template.rs"
Cohesion: 0.19
Nodes (14): exact_slot(), json_names(), json_slots(), json_templates_keep_types(), match_text(), names(), parse(), put_builds_the_document_a_pointer_describes() (+6 more)

### Community 124 - "LeaseRequest"
Cohesion: 0.25
Nodes (3): denied(), Engine, LeaseRequest

### Community 125 - "StepVerdict"
Cohesion: 0.29
Nodes (6): verdicts(), StepVerdict, Allowed, NeedsApproval, NotReached, Refused

### Community 126 - "journal.rs"
Cohesion: 0.13
Nodes (8): audit_chain_detects_deletion(), audit_chain_detects_tampering(), AuditRecord, GENESIS, hash_record(), Inserted, Existing, New

### Community 127 - "super"
Cohesion: 0.06
Nodes (22): Approval, ApprovalStatus, .ALL, Approved, Consumed, Denied, Expired, Pending (+14 more)

### Community 128 - "Adoption, sustainability and strategic futures"
Cohesion: 0.11
Nodes (19): 1. Where the project stands, 2. Positioning, honestly, 3. Who might actually use it, 4.1 Get one thing to work for one stranger (next 30 days), 4.2 Get one physical result (30–90 days), 4.3 Get one outside adapter (60–120 days), 4.4 Get a second maintainer (90–180 days), 4.5 Then research and pilots (+11 more)

### Community 129 - "Broker"
Cohesion: 0.12
Nodes (3): Broker, ClockServer, tool()

### Community 130 - "with"
Cohesion: 0.11
Nodes (18): jobs, record, name, on, workflow_dispatch, permissions, contents, runs-on (+10 more)

### Community 131 - "check_comparison"
Cohesion: 0.50
Nodes (3): check_comparison(), check_value_against(), Checker<'a>

### Community 132 - "leases_http.rs"
Cohesion: 0.24
Nodes (13): AcquireLeaseParams, a_lease_over_http_keeps_other_agents_off_the_device(), a_read_only_credential_cannot_lease(), agent(), code(), Lab, lab(), PSU (+5 more)

### Community 133 - "path"
Cohesion: 0.12
Nodes (14): HOME_ENV, RUNTIME_FILE, RuntimeInfo, merge(), misbehave(), misbehave_json(), psu_sim(), python() (+6 more)

### Community 134 - "client_command"
Cohesion: 0.15
Nodes (10): approve(), build_request(), client_command(), ExecArgs, parse_enum(), plan_exit(), plan_preflight_exit(), read_plan() (+2 more)

### Community 135 - "Role"
Cohesion: 0.20
Nodes (6): CreatedToken, GatewayInfo, Role, Agent, Operator, TokenInfo

### Community 136 - "fault.rs"
Cohesion: 0.15
Nodes (11): after_apply(), arg(), before_apply(), ExecFault, ApplyThenHang, ApplyThenMalformed, Crash, Disconnect (+3 more)

### Community 137 - "openmhs-types"
Cohesion: 0.44
Nodes (17): openmhs-adapter, openmhs-adapter-template, openmhs-api, openmhs-cli, openmhs-conformance, openmhs-engine, openmhs-gateway, openmhs-host (+9 more)

### Community 138 - "Observer"
Cohesion: 0.13
Nodes (3): domain_id(), Observer, RobotSim

### Community 139 - "ConformanceLevel"
Cohesion: 0.11
Nodes (14): req(), Requirement, requirement(), REQUIREMENTS, fault(), level(), SPEC, the_spec_table_matches_the_registry() (+6 more)

### Community 140 - "untrusted.rs"
Cohesion: 0.20
Nodes (13): CLOSE, descriptor(), descriptor_prose_is_quoted_but_identifiers_are_not(), device_value(), injected_structure_is_flattened(), is_token(), long_text_is_capped_on_a_char_boundary(), MAX_CHARS (+5 more)

### Community 141 - "Threats"
Cohesion: 0.12
Nodes (16): 10. Adapter impersonation, 11. Authorization bypass through public interfaces, 1. Credential theft, 2. Principal spoofing, 3. Scope escalation, 4. Expired and revoked credentials, 5. Approval replay, 6. Approval modification (+8 more)

### Community 142 - "3. Methods"
Cohesion: 0.12
Nodes (16): 1. Framing, 2. Lifecycle, 3. Methods, 4. Errors, 5. Supervision, 6. Process environment, 7. Protocol violations, Adapter-host protocol (v0.1) (+8 more)

### Community 143 - "run"
Cohesion: 0.05
Nodes (14): RecordingProxy, _forward(), _handle(), adapter_for(), FakeOctoPrint, do_GET(), do_POST(), reply() (+6 more)

### Community 144 - "stop_lane.rs"
Cohesion: 0.33
Nodes (13): A, a_stop_interrupts_a_running_move(), a_stop_is_still_authorized_and_validated(), a_stop_passes_another_holders_lease(), AXIS, B, homed(), invoke() (+5 more)

### Community 145 - "ProcessAdapter"
Cohesion: 0.21
Nodes (3): call_error(), ProcessAdapter, ProcessFixture

### Community 147 - "Writing an OpenMHS adapter"
Cohesion: 0.13
Nodes (15): 10. What is not covered yet, 1. What an adapter is, 2. Before you start, 3. Create your adapter, 4. Run the conformance suite, 5. The rules your adapter must follow, 6. Worked example: add a capability, 7. Make it your device (+7 more)

### Community 148 - "attributes"
Cohesion: 0.13
Nodes (14): description, label, options, placeholder, value, body, attributes, id (+6 more)

### Community 149 - "Code registry"
Cohesion: 0.13
Nodes (15): Adapter, Adapter signatures (M8), Approvals (M8), Availability (ADR-0019), Code registry, Commands, Credentials and scopes (M8), Error object (+7 more)

### Community 150 - "psu-sim/manifest.json"
Cohesion: 0.14
Nodes (13): adapter, id, version, capabilities, allow, config, seed, PYTHONPATH (+5 more)

### Community 151 - "storage.rs"
Cohesion: 0.22
Nodes (13): StorageLimits, codes(), agent(), at_the_hard_limit_nothing_reaches_a_device_without_its_dispatch_recorded(), at_the_soft_limit(), audit_count(), AXIS, lab() (+5 more)

### Community 152 - "spec_fixtures.rs"
Cohesion: 0.33
Nodes (10): command_examples(), documented_descriptor_codes_match_fixtures(), Expected, invalid_descriptors_fail_with_exactly_the_expected_code(), json_files(), read_json(), receipt_and_event_examples(), spec_dir() (+2 more)

### Community 153 - "ROS 2 (observation only)"
Cohesion: 0.12
Nodes (14): Configure a real robot, Containers and supervision, Readings, ROS 2 (observation only), Try it against the simulator, What it does not do, ADR-0012: MCP server design, agent guidance and the Python SDK, Alternatives considered (+6 more)

### Community 154 - "M8: identity and trust"
Cohesion: 0.14
Nodes (14): Breaking changes, Breaking changes, Database changes, Database changes, Gate results (Windows 11, 2026-10-24), Gate results (Windows 11, 2026-10-25), Implementation status, Known limitations (+6 more)

### Community 155 - "Landscape: AI agents ↔ physical hardware (as of 2026-10-23)"
Cohesion: 0.29
Nodes (7): 1. Anthropic's Model Hardware Standard (MHS), 2. The "open MHS" ecosystem that appeared in the two months since, 3. Established standards OpenMHS must build on, not replace, 4. Where OpenMHS adds independent value, 5. Reusable libraries (verified on crates.io 2026-10-23), 6. Open questions carried forward, Landscape: AI agents ↔ physical hardware (as of 2026-10-23)

### Community 156 - "Troubleshooting"
Cohesion: 0.14
Nodes (14): A command is `DENIED`, `another gateway is already running with home ...`, `AWAITING_APPROVAL`, `command.outcome_unresolved`, `gateway.forbidden: changing grants requires an operator token`, `no running gateway found`, `openmhs conformance` is slow or fails to start an adapter, `policy.denied.no_grant` (+6 more)

### Community 157 - "FirmwareTests"
Cohesion: 0.16
Nodes (4): Rig, FirmwareTests, parse_time(), wait_for()

### Community 158 - "body"
Cohesion: 0.14
Nodes (13): description, label, placeholder, value, body, attributes, id, type (+5 more)

### Community 159 - "ADR-0023: The state fabric: events, watches, and the `subscribe` operation"
Cohesion: 0.20
Nodes (10): ADR-0023: The state fabric: events, watches, and the `subscribe` operation, Alternatives considered, Consequences, Context, Decision, Events, Events are not truth, Watches (+2 more)

### Community 160 - "RFC: Bounded storage, audit lifecycle and fleet-scale scheduling"
Cohesion: 0.15
Nodes (13): 10. Provisional decisions (2026-10-28), 1. Problem, 2. Principles, 3. Event classes, 4. Storage layout, 5. Failure behaviour, 6. Audit segments and checkpoints, 7. Fleet-scale health scheduling (extends ADR-0019) (+5 more)

### Community 161 - "openmhs-http/src/sim.rs"
Cohesion: 0.28
Nodes (3): discard_pending(), Stream, T

### Community 162 - "Script"
Cohesion: 0.25
Nodes (6): a_crash_mid_plan_comes_back_awaiting_a_decision(), Fault, Reject, Uncertain, register_lab(), Script

### Community 164 - "PlanState"
Cohesion: 0.17
Nodes (10): PlanState, Aborted, AwaitingDecision, Cancelled, Compensated, Compensating, Halted, Running (+2 more)

### Community 165 - "Independent hardware validation plan"
Cohesion: 0.17
Nodes (12): 1. Choosing the first devices, 2. Test equipment, 3. Reference measurements, 4.1 Commands, 4.2 Permissions, 4.3 Timeouts, disconnects, restarts, 4.4 Repeatability, 4. Test cases (+4 more)

### Community 166 - "body"
Cohesion: 0.17
Nodes (11): label, value, body, attributes, id, type, validations, description (+3 more)

### Community 167 - "host"
Cohesion: 0.17
Nodes (12): permissions, if, needs, outputs, permissions, runs-on, host, val (+4 more)

### Community 169 - "Health"
Cohesion: 0.29
Nodes (5): Health, Degraded, Disconnected, Healthy, parse_health()

### Community 170 - "Connect your own device"
Cohesion: 0.18
Nodes (11): 1. Install, 2. Pick how OpenMHS talks to your device, 3. Describe the device, 4. Start the gateway, 5. Decide what is allowed, 6. Connect an agent, a script or yourself, Check what happened, Connect your own device (+3 more)

### Community 171 - "HardwareBench: research plan"
Cohesion: 0.18
Nodes (11): 10. Plan, 1. What exists to build on, 2. Three layers, three subjects, 3. Scenarios, 4. Metrics, 5. Running against other implementations, 6. Research questions, 7. Baselines (+3 more)

### Community 172 - "jobs"
Cohesion: 0.18
Nodes (10): if, needs, runs-on, jobs, announce, name, on, pull_request (+2 more)

### Community 173 - "steps"
Cohesion: 0.40
Nodes (11): steps, steps, steps, steps, steps, id, if, name (+3 more)

### Community 174 - "PlanDecision"
Cohesion: 0.29
Nodes (6): DecisionRecord, PlanDecision, Abort, Compensate, Resume, Retry

### Community 175 - "start_hosted"
Cohesion: 0.40
Nodes (4): Trust, conformance(), ConformanceArgs, start_hosted()

### Community 176 - "ADR-0013: Conformance suite and adapter scaffolding"
Cohesion: 0.33
Nodes (6): ADR-0013: Conformance suite and adapter scaffolding, Alternatives considered, Consequences, Context, Decision, Deviations from the M1 level text

### Community 177 - "HTTP profiles"
Cohesion: 0.40
Nodes (5): Connection, HTTP profiles, Limits, Mapping, What each answer means

### Community 178 - ".initialize"
Cohesion: 0.18
Nodes (4): Misbehave, noise(), now(), reading()

### Community 179 - "Decision"
Cohesion: 0.20
Nodes (10): ADR-0018: Capability graph, leases, stop lane and plans, Alternatives considered, Consequences, Context, Decision, Graph, Leases, Plans (+2 more)

### Community 180 - "Decision"
Cohesion: 0.20
Nodes (10): ADR-0019: Device availability, Consequences, Context, Decision, Not decided here, Registration, States, Visibility (+2 more)

### Community 181 - "M10 release audit"
Cohesion: 0.20
Nodes (9): Experimental, or limited scope, Hardware, Implemented and tested, Inaccuracies found in the README, M10 release audit, Planned, not built, Platforms, Release state (+1 more)

### Community 182 - "M12 runbook: bounded, independently observed ESP32 test"
Cohesion: 0.06
Nodes (30): Experiment 02: an agent and a simulated ESP32 over MQTT, Reproduce it, What it validates, and what it does not, What was built, Why QEMU and not Wokwi, Experiment 02: results, Findings, How the lifecycle behaved (+22 more)

### Community 183 - "OpenMHS Playground"
Cohesion: 0.20
Nodes (10): 1. What already exists, 2. Simulator connector vs protocol adapter, 3. The local playground, 4. External simulators, 5. Hosted playground (later, carefully), 6. Proof of concept in this branch, Execution timeline, OpenMHS Playground (+2 more)

### Community 184 - "AdapterError"
Cohesion: 0.09
Nodes (9): _NoRedirect, OctoPrintAdapter, OctoPrintClient, rfc3339(), ReadOnlyByConstruction, convert(), bad(), ConversionTests (+1 more)

### Community 185 - "PlansCmd"
Cohesion: 0.22
Nodes (8): parse_decision(), PlansCmd, Cancel, Decide, List, Preflight, Show, Submit

### Community 187 - "Connecting an AI agent over MCP"
Cohesion: 0.22
Nodes (9): 1. Start a gateway and give the agent a token, 2. Register the server with your MCP host, 3. What the agent gets, 4. Verified with, Any other MCP host, Claude Code, Connecting an AI agent over MCP, Fastest start: one command, no setup (+1 more)

### Community 188 - "Releasing OpenMHS"
Cohesion: 0.22
Nodes (9): Before tagging, crates.io is blocked, Cutting the release, Documentation site, PyPI, Recommended repository settings, Releasing OpenMHS, What a release consists of (+1 more)

### Community 189 - "RFC NNNN: Title"
Cohesion: 0.22
Nodes (9): Alternatives considered, Compatibility, Design, How it will be tested, Motivation, Open questions, RFC NNNN: Title, Safety and security (+1 more)

### Community 190 - "steps"
Cohesion: 0.28
Nodes (9): spec, steps, steps, name, runs-on, steps, if, name (+1 more)

### Community 191 - "GH_TOKEN"
Cohesion: 0.25
Nodes (9): env, env, needs, runs-on, env, BUILD_MANIFEST_NAME, GH_TOKEN, env (+1 more)

### Community 192 - "build-local-artifacts"
Cohesion: 0.22
Nodes (9): container, if, name, needs, runs-on, strategy, build-local-artifacts, fail-fast (+1 more)

### Community 193 - ".__init__"
Cohesion: 0.25
Nodes (5): ConfigError, is_loopback(), load_trusted(), parse_node(), Settings

### Community 194 - "ApprovalsCmd"
Cohesion: 0.25
Nodes (7): ApprovalsCmd, Approve, Deny, List, Request, Revoke, Show

### Community 195 - "ADR-0001: Single trusted gateway process; all interfaces are clients"
Cohesion: 0.40
Nodes (5): ADR-0001: Single trusted gateway process; all interfaces are clients, Alternatives considered, Consequences, Context, Decision

### Community 196 - "principal.rs"
Cohesion: 0.32
Nodes (5): is_identifier(), is_kind(), is_principal(), MAX_KIND, MAX_NAME

### Community 197 - "ADR-0014: Protocol adapters driven by profiles"
Cohesion: 0.25
Nodes (8): ADR-0014: Protocol adapters driven by profiles, Alternatives considered, Amendment (M7): publishing, Amendment (M7): TLS dependencies, Consequences, Context, Decision, No core changes

### Community 198 - "Core concepts"
Cohesion: 0.25
Nodes (8): Adapters, Authorization, Command outcomes, Commands, Coordination, Core concepts, Devices and capabilities, What OpenMHS is not

### Community 199 - "M10_TEST_REPORT.md"
Cohesion: 0.04
Nodes (44): Demo and walkthrough scripts, Narrated walkthrough (about 4 minutes), The 60-second version, Configuration, How failures are classified, Limits, OctoPrint adapter (read-only), Setting up a virtual printer (+36 more)

### Community 200 - "ADR-0003: Small typed authorization engine now; Cedar re-evaluated in M8"
Cohesion: 0.40
Nodes (5): ADR-0003: Small typed authorization engine now; Cedar re-evaluated in M8, Alternatives considered, Consequences, Context, Decision

### Community 201 - "Transport security"
Cohesion: 0.25
Nodes (8): Configuration (MQTT and HTTP profiles), Local development without disabling verification, Not covered yet, OPC UA, ROS 2, Tests, The rules, Transport security

### Community 202 - "Current state audit"
Cohesion: 0.25
Nodes (8): 1. Scope and method, 2. Repository facts, 4. Feature classification, 5. Quality of the documentation and governance, 6. Technical debt, 8. Test results on this branch, Current state audit, Not run, and why

### Community 203 - "jobs"
Cohesion: 0.25
Nodes (7): env, CARGO_TERM_COLOR, RUSTFLAGS, jobs, name, permissions, contents

### Community 204 - "steps"
Cohesion: 0.25
Nodes (8): CARGO_TARGET_DIR, OPENMHS_BIN, OPENMHS_OCTOPRINT, octoprint, name, runs-on, steps, env

### Community 205 - "with"
Cohesion: 0.25
Nodes (8): with, merge-multiple, name, path, pattern, persist-credentials, subject-path, submodules

### Community 206 - "Authorization"
Cohesion: 0.25
Nodes (8): 1. Principles, 2. Principals, 3. Grants, 4. Evaluation, 5. Approval, 6. Out of scope for v0.1, 7. Credentials and scopes, Authorization

### Community 207 - "Command lifecycle"
Cohesion: 0.25
Nodes (8): 1. States and transitions, 2. Outcome classes, 3. Rules, 3a. Validation order, 4. Durability and crash recovery, 5. Reconciliation, Command lifecycle, What the gateway reports after a timeout or cancellation

### Community 208 - "Plans (v0.1)"
Cohesion: 0.25
Nodes (8): 1. Shape, 2. Running, 3. Failure, 4. States, 5. Decisions, 6. Restart, 7. Who sees and controls a plan, Plans (v0.1)

### Community 209 - "Stager"
Cohesion: 0.36
Nodes (3): Stager, attr(), md()

### Community 210 - "CommandsCmd"
Cohesion: 0.29
Nodes (7): CommandsCmd, Approve, Cancel, List, Reconcile, Resolve, Show

### Community 211 - "openmhs-cli/src/values.rs"
Cohesion: 0.52
Nodes (3): parse_param(), parse_value(), split_number()

### Community 212 - "ADR-0005: Local API is JSON-RPC 2.0 over HTTP on loopback, token-authenticated"
Cohesion: 0.40
Nodes (5): ADR-0005: Local API is JSON-RPC 2.0 over HTTP on loopback, token-authenticated, Alternatives considered, Consequences, Context, Decision

### Community 213 - "openmhs-host/tests/conformance.rs"
Cohesion: 0.20
Nodes (4): HostOptions, Trust, AllowUnsigned, Require

### Community 214 - "ADR-0008: Rust for the trusted core, Python for clients and out-of-process adapters"
Cohesion: 0.40
Nodes (4): ADR-0008: Rust for the trusted core, Python for clients and out-of-process adapters, Consequences, Context, Decision

### Community 215 - "Protocol profiles"
Cohesion: 0.29
Nodes (7): Layout, Limits, Protocol profiles, Templates, The simulator, Using a profile, Values on the wire

### Community 216 - "SCPI profiles"
Cohesion: 0.29
Nodes (7): Connection, Limits, Mapping, SCPI profiles, Serial ports, Timeouts, cancellation and retries, What each answer means

### Community 217 - "ADR-0010: Engine execution model"
Cohesion: 0.29
Nodes (7): ADR-0010: Engine execution model, Alternatives considered, Amendment (M3): stop cannot preempt a running command, Amendment (M9): the stop lane, Consequences, Context, Decision

### Community 218 - "ADR-0022: Insights: learning from the journal, never from a back door"
Cohesion: 0.29
Nodes (7): ADR-0022: Insights: learning from the journal, never from a back door, Alternatives considered, Boundaries, Context, Decision, Tests, What this does not do

### Community 219 - "SUMMARY.md"
Cohesion: 0.29
Nodes (6): Core Concepts, Getting Started, Guides, Project, Reference, Summary

### Community 220 - "`openmhs` command line"
Cohesion: 0.29
Nodes (7): Commands, Exit codes, Grants, `openmhs` command line, Quick start, Simulator faults, Values

### Community 221 - "Configuration"
Cohesion: 0.29
Nodes (7): Clients, Configuration, Device profiles, descriptors and manifests, Other variables, Running the gateway, The gateway home, Transport security

### Community 222 - "ADR-0011: Gateway API, roles and home directory"
Cohesion: 0.40
Nodes (5): ADR-0011: Gateway API, roles and home directory, Alternatives considered, Consequences, Context, Decision

### Community 223 - "ADR-0015: Out-of-process adapters and the adapter host"
Cohesion: 0.40
Nodes (5): ADR-0015: Out-of-process adapters and the adapter host, Alternatives considered, Consequences, Context, Decision

### Community 224 - "Installation"
Cohesion: 0.10
Nodes (17): 1. The power supply, 2. Check the audit log, First simulated device, Where next, From a release archive, From source, Installation, Optional (+9 more)

### Community 225 - "GitHub presentation and recommended settings"
Cohesion: 0.29
Nodes (7): Before the repository is made public (or announced), Dependabot (proposal, not added), Description, GitHub presentation and recommended settings, Settings to consider, Social preview, Topics

### Community 226 - "main.c"
Cohesion: 0.08
Nodes (17): ack(), handle_clock(), handle_pulse(), handle_set(), now_rfc3339(), on_message(), on_mqtt(), publish_state() (+9 more)

### Community 227 - "with"
Cohesion: 0.29
Nodes (7): opcua, name, runs-on, steps, with, components, python-version

### Community 228 - "rust"
Cohesion: 0.29
Nodes (7): rust, os, name, runs-on, strategy, fail-fast, matrix

### Community 229 - "plan"
Cohesion: 0.29
Nodes (7): plan, publishing, tag, tag-flag, env, outputs, runs-on

### Community 230 - "Conformance"
Cohesion: 0.29
Nodes (7): 1. Levels, 2. Reaching a level, 3. Fixtures and faults, 4. Requirements, 5. How the suite exercises a device, 6. Report, Conformance

### Community 231 - "jsonschema"
Cohesion: 0.14
Nodes (12): 1. Top level, 2.1 Side-effect classes, 2. Capabilities, 4. Preconditions, 5. Verification, 6. Validation rules, 7. Relationships, Device descriptors (+4 more)

### Community 232 - "LeasesCmd"
Cohesion: 0.33
Nodes (6): LeasesCmd, Acquire, List, Release, Renew, Show

### Community 233 - "TokensCmd"
Cohesion: 0.33
Nodes (5): TokensCmd, Create, List, Revoke, Rotate

### Community 234 - "Lookup"
Cohesion: 0.53
Nodes (5): Lookup, Expired, Revoked, Unknown, Valid

### Community 235 - "ADR-0017: Identity and trust"
Cohesion: 0.40
Nodes (5): ADR-0017: Identity and trust, Alternatives considered, Consequences, Context, Decision

### Community 236 - "ADR-0002: Canonical command lifecycle with explicit uncertainty"
Cohesion: 0.33
Nodes (6): ADR-0002: Canonical command lifecycle with explicit uncertainty, Alternatives considered, Amendment (M1, 2026-10-23), Consequences, Context, Decision

### Community 237 - "ADR-0006: MCP via the official Rust SDK (`rmcp`), as a gateway client"
Cohesion: 0.40
Nodes (5): ADR-0006: MCP via the official Rust SDK (`rmcp`), as a gateway client, Alternatives considered, Consequences, Context, Decision

### Community 238 - "ADR-0020: Readback must be measured after the command was sent"
Cohesion: 0.33
Nodes (6): ADR-0020: Readback must be measured after the command was sent, Clock allowance, Consequences, Context, Decision, Tests

### Community 241 - "contact_links"
Cohesion: 0.33
Nodes (5): blank_issues_enabled, contact_links, about, name, url

### Community 242 - "Governance"
Cohesion: 0.33
Nodes (6): Changing this document, Governance, How decisions are made, Principles that decisions follow, Releases, Roles

### Community 243 - "Line"
Cohesion: 0.50
Nodes (4): Line, Data, Eof, TooLong

### Community 244 - "AdapterCmd"
Cohesion: 0.40
Nodes (5): AdapterCmd, Check, Keygen, New, Sign

### Community 245 - "GrantsCmd"
Cohesion: 0.40
Nodes (5): GrantArgs, GrantsCmd, Add, List, Revoke

### Community 246 - "TrustCmd"
Cohesion: 0.40
Nodes (5): trust_cmd(), TrustCmd, Add, List, Revoke

### Community 247 - "Modbus TCP profiles"
Cohesion: 0.40
Nodes (5): Connection, Limits, Mapping, Modbus TCP profiles, What each answer means

### Community 248 - "ADR-0021: Journal quota and critical reserve"
Cohesion: 0.40
Nodes (5): ADR-0021: Journal quota and critical reserve, Context, Decision, Tests, What this does not do

### Community 249 - "The adapter host"
Cohesion: 0.40
Nodes (5): Failure handling, Limits, The adapter host, What stays in the gateway, What the tests prove

### Community 250 - "LeaseStatus"
Cohesion: 0.50
Nodes (4): LeaseStatus, Active, Expired, Released

### Community 251 - "ADR-0007: Independent of Anthropic MHS; compatibility via a future façade"
Cohesion: 0.50
Nodes (4): ADR-0007: Independent of Anthropic MHS; compatibility via a future façade, Consequences, Context, Decision

### Community 252 - "uses"
Cohesion: 0.40
Nodes (5): name, runs-on, steps, deny, uses

### Community 253 - "ros2"
Cohesion: 0.40
Nodes (5): ROS_IMAGE, ros2, env, name, runs-on

### Community 254 - "env"
Cohesion: 0.40
Nodes (5): ANNOUNCEMENT_BODY, ANNOUNCEMENT_TITLE, PRERELEASE_FLAG, RELEASE_COMMIT, env

### Community 256 - "Adapter permission manifest (v0.1)"
Cohesion: 0.40
Nodes (5): Adapter permission manifest (v0.1), Fields, Platform notes, Signatures, What a manifest guarantees, and what it does not

### Community 257 - "Capability graph and topology (v0.1)"
Cohesion: 0.40
Nodes (5): Capability graph and topology (v0.1), Checks, Leases, Sources, What the graph decides: claims

### Community 258 - "OpenMHS website"
Cohesion: 0.40
Nodes (4): Keeping it accurate, OpenMHS website, Preview, Publishing

### Community 259 - "vercel.json"
Cohesion: 0.40
Nodes (4): cleanUrls, headers, $schema, trailingSlash

### Community 260 - "DevicesCmd"
Cohesion: 0.50
Nodes (4): DevicesCmd, Inspect, List, Status

### Community 261 - "ProfileCmd"
Cohesion: 0.67
Nodes (3): ProfileCmd, Check, New

### Community 262 - "SimCmd"
Cohesion: 0.67
Nodes (3): SimCmd, Fault, Faults

### Community 264 - "PULL_REQUEST_TEMPLATE.md"
Cohesion: 0.50
Nodes (3): Checklist, How it was tested, What and why

### Community 265 - "steps"
Cohesion: 0.50
Nodes (4): release-tooling, name, runs-on, steps

### Community 266 - "on"
Cohesion: 0.50
Nodes (4): on, pull_request, push, branches

### Community 268 - "AuditCmd"
Cohesion: 0.67
Nodes (3): AuditCmd, Tail, Verify

### Community 270 - "OpenMHS Specification v0.1 (draft)"
Cohesion: 0.67
Nodes (3): Conventions, OpenMHS Specification v0.1 (draft), Versioning

## Knowledge Gaps
- **340 isolated node(s):** `Post`, `Put`, `Patch`, `Silent`, `Timeout` (+335 more)
  These have ≤1 connection - possible missing edges or undocumented components. (Counts symbols only; 2183 node(s) total have ≤1 connection when file, concept and rationale nodes are included.)
- **17 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `serve()` connect `adapter.py` to `1. Hardware execution`, `_Server`, ``openmhs` command line`, `ADR-0014: Protocol adapters driven by profiles`?**
  _High betweenness centrality (0.084) - this node is a cross-community bridge._
- **Are the 10 inferred relationships involving `Value` (e.g. with `canonical_json()` and `status()`) actually correct?**
  _`Value` has 10 INFERRED edges - model-reasoned connections that need verification._
- **What connects `Post`, `Put`, `Patch` to the rest of the system?**
  _340 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `pathlib` be split into smaller, more focused modules?**
  _Cohesion score 0.0644384221619994 - nodes in this community are weakly interconnected._
- **Why does `openmhs (Python client)` connect `Client` to `__init__.py`, `connect-a-device.md`?**
  _High betweenness centrality (0.072) - this node is a cross-community bridge._
- **Are the 21 inferred relationships involving `EngineError` (e.g. with `denied()` and `.approval()`) actually correct?**
  _`EngineError` has 21 INFERRED edges - model-reasoned connections that need verification._
- **Should `Client` be split into smaller, more focused modules?**
  _Cohesion score 0.042053184910327765 - nodes in this community are weakly interconnected._