# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). OpenMHS is pre-alpha: until 1.0,
any minor version may change the specification, the adapter contract and the local API,
and the entry will say so under **Changed** or **Removed**.

No release has been published yet.

## [Unreleased]

Work towards the first public release (0.1.0). Milestones M0–M9 are described in
[docs/roadmap.md](docs/roadmap.md) and [docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md).

### Added

- Specification v0.1: descriptors, units, command lifecycle, authorization, errors,
  conformance levels, capability graph, leases and plans (`spec/`).
- Gateway (`openmhsd`) and CLI (`openmhs`) with a local JSON-RPC API, scoped credentials,
  operator and agent roles, single-use approvals, a SQLite journal and a hash-chained audit
  log.
- Command engine: deny-by-default policy, units and ranges, rate and slew limits,
  preconditions, freshness, idempotency, deadlines, `UNKNOWN_OUTCOME`, readback
  verification, reconciliation and crash recovery.
- MCP server over stdio (`openmhs mcp`) and a standard-library-only Python client SDK.
- Virtual lab (thermometer, power supply, linear stage) with fault injection.
- Conformance suite L0–L3 with JSON evidence reports.
- Protocol adapters driven by JSON profiles (Modbus TCP, MQTT, SCPI, HTTP), an
  out-of-process adapter host with permission manifests and signed manifests, a Python
  adapter SDK, and read-only OPC UA and observation-only ROS 2 adapters.
- Leases, plans with twin preflight, compensation and a stop lane.
- Release engineering: cargo-dist configuration, Python packaging, a documentation site,
  an adapter registry, a generated compatibility matrix, and governance files.

### Known limitations

- No adapter has been run against physical hardware by the project.
- Twin preflight has no physics.
- The adapter host has no OS sandbox.
- Crates are not published to crates.io (see [docs/release/RELEASING.md](docs/release/RELEASING.md)).
