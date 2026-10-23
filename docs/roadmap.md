# Roadmap

Each milestone ends with a review checkpoint. Nothing here is implemented unless
the milestone is marked **Done**.

| # | Milestone | Status |
|---|---|---|
| M0 | Research, positioning, naming, ADRs | Done |
| M1 | Specification v0.1: JSON Schemas, lifecycle/units/errors/authorization specs, 3 dissimilar example devices, invalid fixtures | Done |
| M2 | Core engine: registry, constraints, authorization, command state machine, SQLite journal with crash recovery, state store, hash-chained audit, adapter trait, virtual PSU with fault injection | Done |
| M3 | Gateway (`openmhsd`), local JSON-RPC API, CLI, three virtual devices, `openmhs demo`, CI on Windows/macOS/Linux | Done |
| M4 | MCP server (gateway client), pure-Python client SDK, mock-agent replanning example | Done |
| M5 | Conformance suite (L0–L2 tested; L3 defined in M9), `openmhs adapter new`, adapter author guide | Done (pending review) |
| M6 | Protocol adapters: Modbus TCP, MQTT, SCPI (TCP/serial), HTTP — each tested against in-repo protocol simulators | Planned |
| M7 | Out-of-process adapter host, Python adapter SDK, OPC UA and ROS 2 adapters, adapter permission manifests | Planned |
| M8 | Identity and trust: scoped credentials, approval workflows, signed descriptors/manifests, signed audit export, threat model | Planned |
| M9 | Capability graph and multi-device orchestration: relationships, resource leases, plans with twin preflight, partial-failure handling, priority stop lane (ADR-0010 amendment) | Planned |
| M10 | Release readiness: binaries, PyPI, docs site, adapter registry index, generated compatibility matrix, governance | Planned |
| M11+ | Advanced track (design docs first): WASM-sandboxed adapters, OpenTelemetry, remote/federated gateways, fleet, assisted adapter generation, HIL rigs, OPC UA / ros2_control / SiLA 2 bridges, device attestation | Planned |

Conformance levels (defined in M1, tested from M5):

- **L0 Discoverable** — identity and capabilities can be inspected.
- **L1 Observable** — state can be read with timestamps and quality.
- **L2 Controllable** — authorized commands execute with verified outcomes.
- **L3 Coordinated** — participates in multi-device plans with leases.

A conformance level is a statement about interface behavior under test. It is not
a safety certification.
