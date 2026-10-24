# Implementation status

What exists, and how much of it has been checked. "Built" means the code compiles in the
workspace. "Verified" means the named tests were run and passed. This page is updated at
the end of each milestone and says plainly when something has not been run.

Last updated: 2026-10-25, on `main` (`m9` pushed to `main` and `m9`).

## Milestones

| # | Status | Verified |
|---|---|---|
| M0–M6 | Done, on `main` | Yes, at their milestones |
| M7 | Done on branch `m7` (`a30aa63`), awaiting review and a push decision | Full gate run at the milestone, except the ROS 2 conformance (Docker was unavailable) |
| M8 | Built on branch `m8`; full gate run on Windows except ROS 2 | Yes, see below |
| M9 | Built and pushed to `main`; full gate run on Windows; CI green except macOS (queued) | Yes, see below |

## M9: capability graph and multi-device orchestration

| Component | Built | Tests |
|---|---|---|
| A. Relationships, topology file, capability graph (`openmhs-types::{topology, graph}`, `graph.get`) | Yes | Pass |
| B. Resource leases: all-or-nothing claims through `contains` and `shares_resource`, deadlines, checked at evaluate and approval, released with revoked credentials | Yes | Pass |
| C. Priority stop lane: `stop: true` actions skip the device lock and leases, never authorization or validation (ADR-0010 amendment) | Yes | Pass |
| D. Plans: DAG order, step preconditions, one lease, halt / compensate / ask, human decisions, crash recovery, every step through the single-command path | Yes | Pass |
| E. Twin preflight: an in-memory engine with twins seeded from real readings; no physics | Yes | Pass |
| F. Interfaces: API (`plans.*`), CLI (`openmhs plans`), MCP (`check_plan`, `submit_plan`, `get_plan`, `cancel_plan`; no decide tool), Python SDK (leases and plans) | Yes | Pass |
| G. Workcell example ([examples/workcell](../examples/workcell/README.md)) | Yes | Pass |
| H. L3 conformance (`L3.leases`, `L3.plan`, `L3.stop`) | Yes | Pass |

Also written: [ADR-0018](adr/0018-orchestration.md), the ADR-0010 amendment,
[spec/topology.md](../spec/topology.md), [spec/plans.md](../spec/plans.md), and updates
to `spec/api.md`, `spec/errors.md`, `spec/descriptor.md`, `spec/lifecycle.md`,
`spec/conformance.md`, `docs/cli.md`, `docs/mcp.md`.

### Gate results (Windows 11, 2026-10-25)

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass |
| `cargo test --workspace` | 354 passed, 0 failed |
| `cargo deny check` | advisories, bans, licences, sources ok |
| `tools/check_schemas.py` | Pass |
| Python SDK, examples and CLI/MCP/SDK parity (`python/tests`) | 41 passed (Python 3.11) |
| `openmhs demo` | every step behaved as expected |
| Conformance, built-in simulators | thermo L1 (declared L1); PSU and axis L3 (declared L3) |
| Conformance, four protocol profiles | HTTP, Modbus, MQTT, SCPI each reached L3 (they declare L2, so pass) |
| Conformance, Python PSU over the adapter host | L3, same checks and results as the Rust PSU |
| OPC UA unit tests and conformance | **Not run locally**: `asyncua` is not installed in this Python. CI runs both; pass |
| ROS 2 conformance | **Not run locally** (Docker not running). CI runs it; pass |
| Linux, Windows CI | Pass. **macOS CI** had not started (queued) when this was written |

The first workspace test run failed to compile: the disk was 99% full and the build
cache was left corrupt. After `cargo clean` (26.7 GiB) the run passed. No test was
changed for it.

Problems found while building M9 and fixed:

- Approving a held command skipped the lease check; another holder's lease could be
  bypassed through an approval. Fixed: leases are checked again at approval.
- Preflight's twin engine logged its registrations and command transitions like the
  real engine, so a log reader could see `Verified` for a command that never ran. Its
  log lines are now inside a `twin` span.
- An `abort` decision was not audited as `plan.decision`. Fixed.
- The example agent's `--confirm` covered only `submit_command`; a model could have
  used `submit_plan` without the human prompt. It now covers every destructive tool.
- The Python test harness never drained the gateway's stderr; on Windows the pipe filled
  and the gateway blocked mid-request. The harness now drains it.

No security test was weakened.

### Required behaviour and where it is tested

| Requirement | Test |
|---|---|
| Leases are all or nothing, follow the graph, and keep others off | `crates/openmhs-engine/tests/leases.rs`, `crates/openmhs-gateway/tests/leases_http.rs` |
| A stop gets through a lease and a busy device, and is still authorized | `crates/openmhs-engine/tests/stop_lane.rs` |
| No plan step bypasses the single-command policy path (deny, scope, slew) | `plans.rs`: `no_step_bypasses_the_single_command_policy_path` |
| Halt, compensate and an uncertain outcome at every step of the workcell | `plans.rs` (scripted faults per step); `crates/openmhs-gateway/tests/workcell.rs` (simulator faults) |
| An unknown outcome is never guessed; decisions are human-only and refused while uncertain | `plans.rs`, `plans_http.rs`, `workcell.rs` |
| Crash mid-move comes back `AWAITING_DECISION` | `plans.rs` |
| Preflight changes nothing real and reports per-step verdicts | `plans.rs` (5 preflight tests), `plans_http.rs` |
| Agents see only their own plans; revoking a credential cancels its plans | `plans.rs`, `plans_http.rs` |
| L3 is testable | `crates/openmhs-conformance/tests/sims.rs`, `crates/openmhs-host/tests/conformance.rs` |

### Known limitations

- Twins have no physics: preflight cannot know a device will stall, refuse or drift.
- Steps run one at a time; parallel steps are not in v0.1.
- L3 runs on one device. Multi-device claims, ordering and compensation are tested in the
  engine, not by the suite. `L3.stop` shows a stop gets through a lease; it does not show
  that a stop interrupts a command in flight.
- A stop is a command, not a safety function.
- Plans are not resumed automatically after a restart; a person decides.

### Breaking changes

- The simulator PSU and axis descriptors declare `conformance: L3` (their descriptor
  hashes change). Reports gain `L3.plan` and `L3.stop`; `L3.leases` is now a real check.
- `Capability` has a `stop` field (default false).

### Database changes

New tables `leases`, `plans` and `plan_commands`, with indexes, created with
`CREATE TABLE IF NOT EXISTS` when the journal opens. Existing tables are unchanged, so
journals from M8 open as they are.

## M8: identity and trust

| Component | Built | Tests |
|---|---|---|
| A. Scoped credentials (`openmhs-policy::auth`, gateway token store) | Yes | Pass |
| B. Approval workflow (`openmhs-types::approval`, engine, journal) | Yes | Pass |
| C. Signed adapter manifests (`openmhs-host::trust`) | Yes | Pass |
| D. Shared authenticated execution context (`Caller` through engine, gateway, CLI, MCP, Python SDK) | Yes | Pass |
| E. Threat model ([M8_THREAT_MODEL.md](security/M8_THREAT_MODEL.md)) | Yes | n/a |

Also written: [ADR-0017](adr/0017-identity-and-trust.md); updates to
`spec/authorization.md` (sections 5 and 7), `spec/permission-manifest.md` (Signatures),
`docs/cli.md`, `docs/adapters/python-sdk.md`, `docs/mcp.md`. The Python SDK gained
scoped `create_token`, `rotate_token`, the approvals methods and `submit(approval_id=)`.

### Gate results (Windows 11, 2026-10-24)

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass |
| `cargo test --workspace` | 294 passed, 0 failed (after two test fixes, below) |
| `cargo deny check` | advisories, bans, licences, sources ok |
| `tools/check_schemas.py` | 57 documents against 8 schemas ok |
| Python SDK, examples and CLI/MCP/SDK parity (`python/tests`) | 38 passed, 0 skipped (Python 3.11) |
| OPC UA adapter unit tests | 11 passed |
| `openmhs demo` | every step behaved as expected |
| Conformance: built-in simulators, four protocol profiles, Python PSU, OPC UA | all reached their declared levels |
| ROS 2 conformance | **Not run**: Docker Desktop not running on this machine (also not rerun at M7) |
| Python 3.10, Linux, macOS | **Not run** locally; CI covers them once the branch is pushed |

Failures found by the first run and how they were resolved:

- `a_modified_command_is_rejected` (test bug). It sent `params` on a write, which the
  command schema forbids, so the request was refused before approvals were consulted. The
  test now asserts that refusal and checks directly that the fingerprint covers `params`.
- `inv08_core_has_no_device_specific_code` (test bug, caused by M8). It read
  `src/engine/` as a file. It now walks subdirectories, so the new approvals module is
  checked too.
- Python suites: 8 failures and 2 errors, from the documented breaking change. The SDK's
  `create_token` could not send scopes, so every credential it made authorized nothing
  (`auth.scope.denied`, the intended default). Fixed in the SDK; tests issue
  `any_scope=True` credentials, as the Rust tests do. The parity test's credential-id
  normalizer was updated for M8's random 16-digit ids.

No security test was weakened, and no security failure was found.

### Required end-to-end tests and where they are

| Required test | Test |
|---|---|
| Valid credential can read | `a_valid_scoped_credential_can_read` |
| Missing or invalid credential rejected | `missing_and_invalid_credentials_are_rejected`, `every_method_requires_authentication` |
| Expired credential rejected | `an_expired_credential_is_rejected` |
| Revoked credential rejected | `a_revoked_credential_is_rejected_immediately` |
| Wrong device scope rejected | `a_credential_cannot_reach_a_device_outside_its_scope` |
| Wrong operation scope rejected | `a_credential_cannot_exceed_its_operations_or_capabilities` |
| Approval-required command cannot dispatch without approval | `a_held_command_gets_a_pending_approval_bound_to_its_fingerprint` |
| Approved command executes | `approved_commands_run_and_spend_their_approval` |
| Modified approved command rejected | `a_modified_command_is_rejected`, `tampering_with_a_held_command_is_caught_at_approval` |
| Expired approval rejected | `an_expired_approval_is_rejected` |
| Revoked approval rejected | `a_revoked_approval_is_rejected` |
| Approval cannot be consumed twice | `an_approval_cannot_be_used_twice` |
| Concurrent consumption does not duplicate | `concurrent_consumption_runs_one_command` |
| Correctly signed manifest accepted | `a_correctly_signed_manifest_is_accepted` |
| Tampered manifest rejected | `a_tampered_manifest_is_rejected` |
| Modified executable rejected | `a_modified_executable_is_rejected` |
| Unknown or revoked signer rejected | `unknown_revoked_and_out_of_scope_signers_are_rejected` |
| Adapter IPC cannot bypass the gateway | `an_adapter_cannot_submit_commands_or_name_a_principal` |
| M1–M7 behavior still works | The existing Rust, Python, demo and conformance suites; pass |

### Known limitations

See the threat model's "Not mitigated" list. In short: same-user attackers, a compromised
signer, adapters without an OS sandbox, plaintext loopback HTTP, no zeroization of secrets
in memory, no signed audit export or signed descriptors, and the interpreter and libraries
of a signed adapter are not covered by its signature.

### Breaking changes

- Credentials without scopes authorize nothing. `tokens create` requires `--scope` or
  `--any-scope`.
- Engine entry points take `&Caller`. `Journal::insert_command` takes an origin.
  `Resolved` has a `dir` field.
- `Client::create_token` takes `CreateTokenParams`. The Python `Client.create_token`
  requires `scopes=[...]` or `any_scope=True`.
- `serve --adapter-manifest` refuses unsigned manifests unless `--allow-unsigned-adapters`.
  `HostOptions::default()` requires signatures.

### Database changes

One new table, `approvals`, with indexes, created with `CREATE TABLE IF NOT EXISTS` when
the journal opens, like the existing tables (the project has no migration framework). The
command table gains no columns. Existing journals open unchanged. Credentials remain in
`tokens.json`; files written before M8 load, with no scopes. A new registry file,
`trusted_keys.json`, appears in the gateway home when the first key is trusted.
