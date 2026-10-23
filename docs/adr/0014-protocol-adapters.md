# ADR-0014: Protocol adapters driven by profiles

- Status: Accepted
- Date: 2026-10-24
- Builds on: [ADR-0013](0013-conformance-suite.md) (conformance suite), [ADR-0009](0009-descriptor-model.md) (descriptor)

## Context

M6 connects OpenMHS to real protocols: Modbus TCP, MQTT, SCPI over TCP and serial, and
HTTP. Each adapter must reach its declared conformance level against a simulator in this
repository, over real sockets, with no network or hardware in tests. The core (types,
adapter contract, engine, policy, gateway, conformance suite) should not need to change.
If it does, the contract was wrong.

Devices that share a protocol differ in register maps, topics and command sets. A Rust
adapter per device model would not scale, and most people who own a PLC or a bench
supply will not write Rust.

## Decision

**A profile per device, an adapter per protocol.** A profile is one JSON file with:

- the device descriptor;
- how to reach the device;
- how each capability maps onto the protocol;
- the simulator's initial state.

Each protocol is a crate under `adapters/`, outside the core `crates/`. The shared crate
`openmhs-profile` provides:

- loading and validation, with every problem named by JSON pointer;
- value encoding;
- `{{placeholder}}` templates;
- `VirtualDevice`, the device state and conformance fixture behind every simulator.

The CLI opens profiles with `serve --profile`, `serve --profile-sim`,
`conformance --profile` and `profile check` ([profiles.md](../adapters/profiles.md)).

**The descriptor stays the contract.** A profile's mapping must fit its capabilities, or
the profile does not load. A writable property needs a write mapping, a template may
name only the value, the action's parameters or the command id, and so on. The adapter
returns the descriptor as written, and its `conformance` claim is what
`openmhs conformance --profile` checks.

**One command, sent once, honestly classified.** Each adapter sends a command exactly
once and never queues it for a later connection. It then decides from the protocol
whether the device acted:

| Protocol | Done | Refused (`REJECTED`) | Uncertain (`UNKNOWN_OUTCOME`) |
|---|---|---|---|
| Modbus TCP | echo matches the request | exception 01, 02, 03, 06 | timeout, dropped connection, mismatched reply |
| SCPI | `*OPC?` answers `1`, error queue empty | an error queued | timeout, dropped connection, answer other than `1` |
| HTTP | 2xx, and `ok: true` if declared | 4xx, 501, 503, `ok: false` | timeout, 408/504, other 5xx, unreadable 2xx |
| MQTT | acknowledgement `ok: true` | acknowledgement `ok: false` | no acknowledgement, dropped connection, unreadable acknowledgement |

"Not sent" (connection refused, not connected) is reported as such, and only when
nothing can have left. After a timeout, every adapter makes sure a late answer is never
read as the answer to the next request:

- Modbus reconnects;
- SCPI discards late input;
- HTTP uses a request per command;
- MQTT matches acknowledgements by command id.

**MQTT needs acknowledgements, and reads prove the link.** Publishing to a topic tells
you nothing about whether the device acted. A profile therefore requires an
acknowledgement topic and `{{command_id}}` in every command payload.

MQTT is push, so the adapter's latest value can outlive the connection. Each read first
does a SUBSCRIBE round trip, which also makes the broker re-send the retained state, so
a cached value is never reported over a dead link (`L1.disconnect.read`).

rumqttc keeps unsent requests and replays them after a reconnect. The adapter therefore
creates a fresh client and event loop for every connection, so nothing published during
one connection can go out on the next.

**Simulators that can disagree with the adapter.** The Modbus, SCPI and HTTP simulators
are written from the protocol specifications, independently of the client libraries, so
the two sides check each other. The MQTT simulator is a small broker written for the
purpose, with the device inside. It uses rumqttc's packet codec, so MQTT packet encoding
is the one place where client and simulator share code. That is acceptable because the
codec is rumqttc's tested and widely used part, and the adapter's own logic is what the
suite judges. Every simulator injects all five conformance faults and counts
deliveries.

**Dependencies.** Each was chosen over writing the protocol client by hand. Each passes
`cargo deny check` (new `deny.toml`, run in CI), and each release is at least two weeks
old.

| Crate | Version | Licence | Why | Not chosen |
|---|---|---|---|---|
| tokio-modbus | 0.17.0 | MIT OR Apache-2.0 | Modbus TCP client; `tcp` feature only | A hand-written client: it would share its mistakes with the hand-written simulator |
| rumqttc | 0.25.1 | Apache-2.0 | MQTT 3.1.1 client, pure Rust; no TLS or websocket features | paho-mqtt (C library and CMake, a burden on Windows); a hand-written client |
| tokio-serial | 5.5.0 | MIT | Async serial ports for SCPI; default features off, so no libudev on Linux | Blocking serialport on a thread |
| serialport (via tokio-serial) | 4.10.1 | MPL-2.0 | — | — |
| bytes | 1 | MIT | Buffers for the simulated broker; already a dependency of rumqttc | — |

reqwest (HTTP) was already a workspace dependency, built without TLS. About 30
third-party packages are new in `Cargo.lock`, most of them platform crates of the above
(`windows-*`, `nix`, `io-kit-sys`, `mach2`).

**serialport is MPL-2.0.** That is file-level copyleft: it covers changes to
serialport's own files, which OpenMHS does not modify, and it is compatible with Apache-2.0
distribution. `deny.toml` allows it for that crate only.

**cargo-deny's first run** checked the whole tree, including dependencies from M1–M5,
and found three things:

- `time` 0.3.45 had RUSTSEC-2026-0009, a stack-exhaustion DoS when parsing RFC 2822
  dates. OpenMHS parses only RFC 3339, so the flaw was not reachable. `Cargo.lock` now
  pins 0.3.55, the newest release at least two weeks old.
- `option-ext`, through `dirs`, is MPL-2.0. It gets the same crate-scoped exception as
  serialport.
- `borrow-or-share`, through `jsonschema`, is MIT-0, which is now allowed. MIT-0 is MIT
  without the attribution requirement.

The workspace's crates depend on each other by path, and none is marked
`publish = false`, so the wildcard ban is off for now (see `deny.toml`).

## No core changes

The protocol adapters use only the public adapter contract, types and conformance API.
No file in `openmhs-types`, `openmhs-adapter`, `openmhs-engine`, `openmhs-policy`,
`openmhs-gateway` or `openmhs-conformance` changed in M6. Changes outside `adapters/`:

- **CLI**: profile options for `serve` and `conformance`, and `profile check`. `serve`
  already hosts the gateway and its virtual lab in-process. Profiles register through
  the existing `GatewayConfig.adapters`.
- **Workspace**: new members, dependencies, `deny.toml`, and a CI job.

## Alternatives considered

- **One crate per device model, in Rust.** Rejected: it does not scale, and the people
  with the devices are not the people who write adapters.
- **A scripting language in profiles** (Lua, Rhai) for conversions and odd protocols.
  Rejected for now. It is a second language to sandbox and audit. Linear scaling,
  templates and pointers cover the devices in the examples. Out-of-process adapters
  (M7) are the escape hatch.
- **Fire-and-forget MQTT** (publish, then infer success from the next state message).
  Rejected: it cannot tell "refused" from "not yet", and would turn every refusal into
  `UNKNOWN_OUTCOME` or, worse, report a later unrelated state change as success.
- **An embedded third-party MQTT broker** (rumqttd) as the simulator. Rejected: a large
  dependency, and it could not drop clients or misbehave on demand.

## Consequences

- A profile that passes conformance is consistent with itself, not proven against the
  device. The hardware guide is marked unverified, and no adapter has been run against
  physical hardware by the project.
- Profiles cover devices that fit the shapes above:
  - Modbus: one value per register or coil; actions write one thing.
  - MQTT: JSON state, acknowledgements.
  - SCPI: line-based commands with `*OPC?` and an error queue.
  - HTTP: a JSON API.

  Anything else needs an adapter.
- Not supported yet: TLS (MQTT, HTTP), Modbus RTU, MQTT 5 and last-will availability,
  SCPI binary blocks. Serial SCPI is tested over a pseudo-terminal on Unix only; Windows
  serial ports are untested.
- Each read is a round trip, also for MQTT. That is fine for supervisory rates, not
  for high-rate telemetry; subscriptions (deferred in ADR-0013) are the place for that.
