# M10 release audit

What the repository contains on 2026-10-25, at the start of M10 (`main` = `b7dd195`), and
how much of it has evidence behind it. This is the baseline the README, the docs site
and the compatibility matrix are written from. "Tested" means an automated test in the
repository exercises it and passed in the M9 gate (Windows 11) and in CI (Linux,
Windows; macOS queued at the time of writing).

## Implemented and tested

| Area | Evidence |
|---|---|
| Descriptor format, JSON Schemas, validation with stable error codes | `openmhs-types` tests, `tools/check_schemas.py` (68 documents, 9 schemas) |
| Policy: grants, scoped credentials, approvals, units, ranges, rate and slew limits, preconditions, freshness | `openmhs-policy`, engine and gateway tests |
| Command lifecycle: idempotency, deadlines, `UNKNOWN_OUTCOME`, readback verification, reconciliation, crash recovery | `openmhs-engine` tests |
| SQLite journal, hash-chained audit log | engine tests, `openmhs demo` |
| Gateway with local JSON-RPC API, CLI (`openmhs`, `openmhsd`) | gateway and CLI tests, `openmhs demo` |
| MCP server over stdio | `openmhs-mcp` tests (in-process client), Python parity test (real binary) |
| Python client SDK (standard library only) | `python/tests` (41 tests) |
| Virtual lab: thermometer, power supply, linear axis, with fault injection | `openmhs-sim` tests, conformance |
| Conformance suite L0–L3 with JSON reports | `openmhs-conformance` tests, `openmhs conformance` |
| Protocol adapters (Modbus TCP, MQTT, SCPI, HTTP) from JSON profiles | conformance against in-repo protocol simulators: each reaches L3 |
| Out-of-process adapter host, Python adapter SDK, permission manifests, Ed25519-signed manifests | `openmhs-host` tests; the Python PSU reaches L3 through the host |
| OPC UA (read-only) and ROS 2 (observation-only) adapters | L1 against simulated servers/publishers, in CI only (need `asyncua` / Docker) |
| Capability graph, leases, plans, twin preflight, stop lane | engine, gateway and workcell tests |

## Experimental, or limited scope

- **Adapter contract** (`openmhs-adapter`) may still change before 1.0.
- **Twin preflight** has no physics.
- **Serial SCPI** is untested on Windows; there is no Modbus RTU.
- **MCP** is stdio only: no HTTP transport, resources or notifications.
- **Adapter host** has no OS sandbox; a permission manifest is declared and checked, not
  enforced by the OS.

## Planned, not built

Writes over OPC UA; ROS 2 services and actions; signed descriptors and signed audit
export; parallel plan steps; everything in the M11+ track.

## Platforms

| Platform | Build and tests | Evidence |
|---|---|---|
| Windows 11 x86_64 | Yes | Local gate and CI |
| Linux x86_64 (Ubuntu) | Yes | CI |
| macOS (GitHub's `macos-latest`, arm64) | Configured in CI | Rust job was queued, not yet run, for the M9 commits; earlier milestones passed |
| macOS x86_64 | Not tested | No runner in CI |

## Hardware

**No adapter has been run against physical hardware by the project.** Everything is
tested against simulators: the built-in virtual lab and the in-repo protocol simulators.
`docs/hardware/` holds unverified guides. Conformance levels are statements about the
integration under the tested faults, never safety certification.

## What a newcomer can try without hardware or an AI account

- `openmhs demo`: a guided run of the real gateway over its API (about 10 seconds).
- `openmhs serve --sim`, then the CLI against three simulated devices.
- `python examples/agent-loop/mock_agent.py`: a scripted agent over MCP; no model, no key.
- `openmhs conformance`: reports for the simulators.
- `openmhs adapter new`: a working adapter crate with a fake device.

The Claude-backed agent example needs an Anthropic API key and is optional.

## Release state

- **Binaries:** none released. Two binaries, `openmhs` and `openmhsd`, both in
  `openmhs-cli`. Nothing release-related was configured before M10.
- **Crates:** none published. Six crates are marked publishable (ADR-0014), but
  `openmhs-types` embeds files from `spec/` with `include_str!`, which a published crate
  cannot do. That blocks crates.io until the schemas move into the crate or are copied
  at build time. M10 does not publish crates.
- **Python:** `python/pyproject.toml` exists, version `0.1.0.dev0`, marked
  `Private :: Do Not Upload`. Not on PyPI.
- **Docs:** Markdown in `docs/` and `spec/`; no site.
- **Community files:** `LICENSE` (Apache-2.0) only. No CONTRIBUTING, SECURITY, CODE_OF_CONDUCT,
  GOVERNANCE, CHANGELOG, or issue and PR templates.
- **CI:** one workflow (`ci.yml`) on pushes to `main` and on PRs, `contents: read`.
  Third-party actions are pinned to release tags, not commit SHAs.
- **Dependencies:** `cargo deny check` passes (licences, advisories, bans, sources).

## Inaccuracies found in the README

- It says "M7" status, and that there is no TLS for MQTT or HTTP; verified TLS shipped
  in M7.
- It says protocol adapters are L2; with M9 they reach L3 against their simulators
  (their profiles still declare L2).
