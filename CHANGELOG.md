# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). OpenMHS is pre-alpha: until 1.0,
any minor version may change the specification, the adapter contract and the local API,
and the entry will say so under **Changed** or **Removed**.

## [Unreleased]

## [0.1.0] - 2026-10-28

The first release. **Pre-alpha: no adapter has been run against physical hardware, and
OpenMHS is not a safety system.** Everything below was tested against simulators. Milestones
M0–M11 are described in [docs/roadmap.md](docs/roadmap.md) and
[docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md); what is known to be missing is
in [docs/limitations.md](docs/limitations.md) and [docs/strategy/](docs/strategy/).

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
- Experiment 01: a read-only OctoPrint adapter tested against OctoPrint's Virtual Printer, with
  a harness, fault-injecting proxy, demo and L1 conformance report. Simulated peer only; no
  physical printer has been tested ([docs/experiments/01-octoprint](docs/experiments/01-octoprint/README.md)).
- `openmhs profile new <protocol>`: a starter profile for your device from the tested example
  of each protocol, checked on creation; `serve` says access is deny-by-default; a hint when a
  bare number is given where a unit is required; help text for every subcommand; and a
  [Connect your own device](docs/connect-a-device.md) guide from install to an agent.
- Experiment 02: ESP32 firmware for the MQTT profile run in Espressif's QEMU beside Mosquitto, with
  a harness, 18 tests and a demo covering state-changing commands, acknowledgements, late
  application, reconciliation and failures. Simulated only; no physical ESP32 has been tested
  ([docs/experiments/02-esp32-qemu-mqtt](docs/experiments/02-esp32-qemu-mqtt/README.md)).
- Leases, plans with twin preflight, compensation and a stop lane.
- No blind retries, enforced: while a write or invoke is `UNKNOWN_OUTCOME`, a new one to
  the same capability is refused (`command.outcome_unresolved`) unless the capability is
  idempotent or a stop. `commands.resolve` / `openmhs commands resolve` lets an operator
  settle an outcome that cannot be read back, with the reason recorded.
- Device availability ([ADR-0019](docs/adr/0019-device-availability.md)): a device that
  does not answer at start is registered `OFFLINE` instead of stopping the gateway, the
  others and crash recovery; probes (`connect`, then `health`) follow it, at most 16 at a
  time; writes and invokes to an `OFFLINE` device are refused before anything is sent
  (`device.unavailable`), stops never are; `devices.status`, `openmhs devices status`,
  MCP `list_devices` (`availability`, `reason`, `busy`, `lease_holder`) and
  `Client.device_status` in the Python SDK. `serve --require-devices` keeps the strict
  start; `--health-interval-ms` sets the probe interval.
- Readback freshness ([ADR-0020](docs/adr/0020-readback-must-follow-dispatch.md)): a reading
  measured before a command was sent no longer verifies it or reconciles it, so a cached or
  retained value cannot produce `VERIFIED` or a false `RECONCILED_NOT_APPLIED` (which invites
  a duplicate physical effect). Such commands end `UNVERIFIABLE`; reconciliation is refused.
  `serve --readback-skew-ms` allows for a device clock behind the gateway's.
- Journal quota ([ADR-0021](docs/adr/0021-journal-quota-and-reserve.md)):
  `serve --journal-max-mib` / `--journal-reserve-mib` limit the journal; within the reserve
  new commands and audited reads are refused (`storage.exhausted`) and only stops and
  recovery may write. Segments, retention and archives are still a draft RFC.
- An unconfirmed stop is never reported as stopped; MCP advises the equipment's own emergency
  stop and that resending the stop is safe.
- SCPI: a timed-out request can no longer desynchronize the link. Exchanges run to
  completion even if the caller gives up, the link resynchronizes with `*IDN?` before
  the next request, and a request is not sent while the instrument is still busy.
- Release engineering: cargo-dist configuration, Python packaging, a documentation site,
  an adapter registry, a generated compatibility matrix, and governance files.

### Changed

- `openmhs serve` and `openmhs_gateway::start` no longer refuse to start when a device does
  not answer; it is registered `OFFLINE` (ADR-0019). `--require-devices` /
  `GatewayConfig::require_devices` restores the old start. `GatewayConfig` gains
  `require_devices` and `health_interval_ms`, `EngineOptions` gains `probe_timeout_ms`
  (code that builds them with struct literals must add the fields; `::new` and
  `::default()` are unchanged). The `device.registered` audit record gains `availability`
  and `reason`.

### Fixed

- SCPI: a link no longer refuses every request for good after an instrument dropped
  requests while the line was down (a serial port that never closes). This made CI
  fail on Linux and macOS since the resynchronization change.

### Known limitations

- No adapter has been run against physical hardware by the project.
- Twin preflight has no physics.
- The adapter host has no OS sandbox.
- Crates are not published to crates.io (see [docs/release/RELEASING.md](docs/release/RELEASING.md)).
