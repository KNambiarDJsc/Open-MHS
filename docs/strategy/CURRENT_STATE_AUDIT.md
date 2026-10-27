# Current state audit

What OpenMHS actually is on `main` at `37c56fb` (2026-10-26), found by reading the source,
running the tests and querying GitHub, on 2026-10-28. Every finding names a file, a test or a
log. Where something was not run, it says so.

Companion documents: [ARCHITECTURE_GAP_ANALYSIS.md](ARCHITECTURE_GAP_ANALYSIS.md) (what is
missing), [NEXT_MILESTONE_ROADMAP.md](NEXT_MILESTONE_ROADMAP.md) (what to do, and the decision
memo).

## 1. Scope and method

| Done | How |
|---|---|
| Repository cloned and read | `git clone`; 41 commits, branches `m5`–`m10`, `website`, `main` |
| Rust workspace sized | 17 crates; about 40,700 lines of Rust (src and tests, `wc -l`) plus about 8,800 lines of Python (SDK, adapters, examples, firmware tests, tools) |
| Command path traced end to end | §3, with line numbers at `37c56fb` |
| Baseline tests run | `cargo test --workspace --no-fail-fast` on Windows 11, Rust 1.89: **369 passed, 0 failed, 0 ignored, 68 test binaries**, exit 0, 10 m 31 s compile |
| CI history queried | `gh run list` / `gh run view` on `KNambiarDJsc/Open-MHS` |
| Linux failure reproduced | Partly: the failing test is `#[cfg(unix)]`, so it cannot run on Windows. A Linux build in WSL was started and stopped for lack of disk; the failure is established from CI log run 40376433981 and the cause by reading `link.rs` and `sim.rs`. The fix's regression test is portable and was shown to fail on the old code on Windows |
| Not done | Linux and macOS test runs locally; ROS 2 and OPC UA adapter tests (no Docker, `asyncua` not installed); ESP32 QEMU tests (ESP-IDF and QEMU not installed); `cargo deny`; the docs-site build (mdBook and lychee not installed); any real hardware |

## 2. Repository facts

| Item | Fact | Evidence |
|---|---|---|
| Created | 2026-10-22 | `gh repo view` |
| Public signals | 3 stars, 1 fork, 0 issues, 2 pull requests (both merged, both by the maintainer), Discussions off | `gh` |
| Releases | None | `docs/limitations.md`; `gh` |
| Maintainers | 1 | `GOVERNANCE.md` |
| CI on `main` at `37c56fb` | **Linux and macOS failed**, Windows passed. Four earlier runs were cancelled after 6 h because macOS runners never started | run 40376433981; runs 40358360206, 40358113473, 40352762553, 40348208873 |
| Cause of the red | `bench_psu_over_a_serial_line_reaches_its_declared_level` (Unix-only): the SCPI link refused every request after the simulator dropped sync queries during a link outage | CI log; `adapters/openmhs-scpi/src/link.rs`, `sim.rs` L170–175 |
| Website | Static, hand-written HTML in `website/`, deployed to Vercel with a strict CSP | `website/vercel.json` |
| Stale claim | `website/reports.html` hard-codes "354 tests" (the M9 gate); the suite now has 369 | `grep` |

## 3. One command, end to end

An agent asks for 2 V on the simulated supply through MCP `submit_command`.

| Step | Where | What happens |
|---|---|---|
| 1 | `crates/openmhs-mcp/src/lib.rs` `submit_command`, `request` | Fetches the descriptor, builds a `commands.submit` request with `operation: write`, the typed value and an idempotency key |
| 2 | `crates/openmhs-gateway/src/server.rs` `dispatch` | Authenticates the bearer token, finds the session and its `Caller` (principal, role, scopes); `COMMANDS_SUBMIT` runs the engine detached from the HTTP request |
| 3 | `Engine::submit_inner` (`engine.rs` L490 at `37c56fb`) | Validates against `command.schema.json`; journals the command with a request hash; same key and body returns the existing receipt, different body is `command.idempotency.conflict` (`journal.rs` L248–271) |
| 4 | `Engine::evaluate` (L579) | Credential scope (before any lookup) → device and capability → lease → policy (`openmhs-policy`) → stateless checks (units, range) → **availability gate (new)** → device lock → unresolved-outcome hold → live checks on fresh readings (preconditions, freshness, rate, slew) |
| 5 | `Engine::run` (L680) | `APPROVED`, or `AWAITING_APPROVAL` for held side-effect classes, with a single-use approval bound to a fingerprint |
| 6 | `Engine::dispatch` (L856) | Journals the readback expectation, then `DISPATCHED` ("intent journaled"), then runs the adapter against the capability timeout |
| 7 | Adapter `execute` | Reports `Applied` or `Rejected`, or fails with a classified error; calls `progress.accepted()` on acknowledgment |
| 8 | `Engine::verify` (L1037) | Polls readback until it matches (`VERIFIED`), mismatches (`VERIFICATION_FAILED`) or is unusable (`UNVERIFIABLE`) |
| 9 | Return path | Receipt with transitions, policy record, adapter result and verification; MCP `command_view` renders `confirmed`, `reasons`, `next_steps` |

Every state change is a row in `commands` and a record in the hash-chained `audit` table.
`recover()` (L1485) runs at start: anything in flight becomes `UNKNOWN_OUTCOME`, nothing is
re-sent.

## 4. Feature classification

| Class | Feature | Evidence |
|---|---|---|
| **Implemented and tested** | Command lifecycle, journal, idempotency, `UNKNOWN_OUTCOME`, reconciliation, crash recovery | `crates/openmhs-engine/tests/{invariants,scenarios,unresolved}.rs`; baseline run |
| | Deny-by-default policy, scoped credentials, roles | `crates/openmhs-policy`; `crates/openmhs-gateway/tests/credentials.rs` |
| | Single-use approvals bound to a fingerprint | `crates/openmhs-engine/tests/approvals.rs`, `gateway/tests/approvals_http.rs` |
| | Leases, stop lane, plans with compensation, twin preflight | `engine/tests/{leases,stop_lane,plans,preflight}.rs`, `gateway/tests/workcell.rs` |
| | Hash-chained audit and verification | `journal.rs` `verify_audit`; used by many tests |
| | Modbus TCP, MQTT, SCPI (TCP), HTTP profile adapters against in-repo protocol simulators | per-crate tests and `tests/conformance.rs`; reports in `conformance/reports/` (L3, declaring L2) |
| | Conformance suite L0–L3 with negative controls | `crates/openmhs-conformance/tests/broken.rs` |
| | Adapter host, signed manifests, process IPC | `crates/openmhs-host/tests/{process,trust,conformance}.rs` |
| | MCP server (stdio), CLI, Python SDK | `openmhs-mcp/tests/mcp.rs`, `openmhs-cli/tests/cli.rs`, `python/tests` |
| | Python adapter SDK | `adapters/python/psu-sim` reaches L3 through the host (M9 gate) |
| **Implemented, insufficiently tested** | SCPI over a serial port | Unix-only test; untested on Windows (`docs/limitations.md`). Its resync logic had a defect that CI caught on Linux and macOS |
| | OPC UA (read-only) and ROS 2 (observation-only) adapters | Tested in CI only; no committed conformance report; not run locally |
| | ESP32 firmware over MQTT | 18 tests in QEMU + Mosquitto, outside CI |
| | OctoPrint (read-only) | Tested in CI against OctoPrint's Virtual Printer; one version, one platform |
| | macOS | arm64 configured in CI but its runners did not start for 6 h; x86_64 never run |
| **Partially implemented** | Agent transport | MCP over stdio only: no HTTP transport, resources or notifications |
| | Plans | Sequential only; no parallel steps; no automatic resume after restart |
| | Twin preflight | Fits the contract, has no physics |
| | Adapter isolation | Declared permissions are checked, not enforced by the OS |
| | Audit integrity | Tamper-evident, not signed; no export; no retention |
| | Playground | `openmhs demo`, `serve --sim`, `--profile-sim` and a fault set exist; no one-command setup or timeline |
| | Registry | Format, checks and matrix exist; no outside entries; one report per entry |
| **Documented but not implemented** | Hardware guides (`docs/hardware/`) | Marked unverified, never run on a device |
| | Advanced track (WASM adapters, OpenTelemetry, federated gateways, fleet, ros2_control / SiLA 2 bridges, device attestation, signed descriptors, signed audit export) | `docs/roadmap.md` M11+ |
| **Missing** | Any release (binaries, PyPI, crates, docs site) | `docs/release/M10_TEST_REPORT.md` |
| | Physical hardware validation | Every report has `sources: ["simulator"]` |
| | Bounded storage / retention | [ARCHITECTURE_GAP_ANALYSIS.md](ARCHITECTURE_GAP_ANALYSIS.md) H8 |
| | Benchmarks of any kind | — |
| | Modbus RTU, CAN | — |
| **Blocked by hardware or external dependency** | Physical validation | No device in this session; see [HARDWARE_VALIDATION_PLAN.md](HARDWARE_VALIDATION_PLAN.md) |
| | ROS 2 conformance locally | Needs Docker (Docker Desktop was stopped) |
| | Release owner actions | Private vulnerability reporting, PyPI trusted publisher, Pages: repository owner only |
| | macOS CI | Runner availability |

## 5. Quality of the documentation and governance

| Area | Finding |
|---|---|
| README | Clear value proposition, real recording, explicit limits. Long, with a compatibility table generated from reports |
| Specification | Complete for v0.1: descriptor, lifecycle, authorization, errors, API, topology, plans, conformance, adapter protocol, permission manifest |
| ADRs | 18 before this work (19 with ADR-0019), consistently structured |
| Threat model | `docs/security/M8_THREAT_MODEL.md`: eleven threats, each with an attack, defense, tests and "remaining" |
| Honesty | Unusually high: every limitation is stated, simulated results are never called validated, the independence from Anthropic's MHS is stated |
| Governance | Present and accurate for one maintainer; no process yet for sharing review |
| Contributor path | Good first issues are drafted but not filed; Discussions off |

## 6. Technical debt

See the inventory in [NEXT_MILESTONE_ROADMAP.md](NEXT_MILESTONE_ROADMAP.md) §3.

## 7. What was implemented on this branch

Branch `strategy/availability`, five commits on top of `main` (`git log main..`).
**Nothing is pushed or merged**, pending review.

| Change | Files | Status |
|---|---|---|
| **SCPI serial CI fix** with a portable regression test that fails on the old code | `adapters/openmhs-scpi/src/link.rs` | Committed (`0dc4199`) |
| **Device availability** ([ADR-0019](../adr/0019-device-availability.md)): offline registration, bounded connect, probes with bounded concurrency and one outstanding probe per device, passive observation, `device.unavailable` gate that never applies to stops, `devices.status` | `openmhs-types/src/availability.rs`, `openmhs-engine/src/engine/availability.rs`, `engine.rs`, `openmhs-api`, `openmhs-gateway` | Committed |
| Propagation | CLI `devices status`, `serve --require-devices --health-interval-ms`, offline lines in the startup banner; MCP `list_devices` fields and instructions; Python `Client.device_status`, `DeviceStatus`, `Availability` | Committed |
| Specs and docs | `spec/api.md`, `spec/errors.md`, `docs/cli.md`, `docs/mcp.md`, `docs/limitations.md`, `CHANGELOG.md`, ADR index | Committed |
| **Lean build profile** (`debug = 1`, `incremental = false`) | `Cargo.toml` | Committed. Measured: full workspace test build 3.9 GB in 511 s, against 17 GB with defaults |
| Strategy documents (10) | `docs/strategy/` | Committed |
| Storage and scaling RFC | `docs/strategy/STORAGE_AND_SCALING_RFC.md` | **Draft for review; no destructive rotation implemented** |

Security boundaries: none changed. A stop is still authenticated, scoped, authorized and
validated; it skips one new gate (availability), as it already skipped the device lock and
leases. `DEGRADED` gates nothing. Availability status is visible to every caller, like the
device and lease lists it derives from.

## 8. Test results on this branch

Windows 11 x86_64, Rust 1.89, Python 3.13, 2026-10-28, with the lean profile.

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass (one finding in my own code was fixed first) |
| `cargo test --workspace --no-fail-fast` | **382 passed, 0 failed, 0 ignored, 70 test binaries**, exit 0. Baseline on `37c56fb` was 369 in 68 binaries, so the 13 new tests (9 engine availability, 3 gateway, 1 MCP) are the whole difference |
| Python SDK, examples, parity (`python/tests`) | 43 tests, OK (Python 3.13). Interpreter-exit `ResourceWarning`s print in the full run; the new test and an existing one each run clean alone with warnings as errors. I did not trace the source of the warnings in the full run |
| New SCPI regression test on the **old** resync | Fails with the CI error (`Down("the instrument has not caught up ...")`); passes on the fix |
| 1,000 offline devices register without blocking; one sweep reaches all | Pass, in virtual time: 1,000 devices answering in 100 ms swept within 64 x 100 ms, at most 16 at once |
| Hung device: registered `OFFLINE` after one probe timeout, probed once across five sweeps | Pass |
| Real binary smoke (`openmhs serve --sim`, throwaway home) | `devices status` all ONLINE → `sim fault link-down` → OFFLINE with reason → agent command `DENIED device.unavailable`, nothing sent (exit 2) → fault cleared, ONLINE again within seconds by the background monitor → command `VERIFIED` → `audit verify` intact, exactly two `device.availability` records |
| Build size | Full workspace test build: 3.9 GB, 511 s (lean profile). After clippy and tests together, `target/` was 5.1 GB. With default profiles the same work had reached 17 GB |

### Not run, and why

| Not run | Reason |
|---|---|
| Linux and macOS tests | No Linux Rust toolchain was completed (the WSL build was stopped to save disk); CI is the check once pushed. **The SCPI serial test that failed in CI is `#[cfg(unix)]` and has not been run against the fix on any Unix**; the fix is reasoned from the logs and proven only by its portable regression test |
| `cargo deny check` | `cargo-deny` is not installed. The only manifest change is `async-trait` (already a workspace dependency) as a gateway dev-dependency |
| `tools/check_schemas.py` and registry/matrix checks | Not run; no schema, spec example or registry file was changed. `spec/api.md` and `spec/errors.md` (prose) were |
| OPC UA, ROS 2, OctoPrint, ESP32 QEMU tests, docs-site build and link check | Dependencies not installed (`asyncua`, Docker, ESP-IDF/QEMU, mdBook, lychee). None of the touched code is in their path except the MCP/API contract they do not use |
| Anything on hardware | None available |
