# OpenMHS

An open, model-agnostic, supervisory interface between AI agents and programmable
hardware.

> **Status: pre-alpha (M6: protocol adapters).** A local gateway serves devices to AI
> agents over MCP, to scripts through a Python SDK, and to people through a CLI. Modbus
> TCP, MQTT, SCPI and HTTP devices are described by JSON profiles. Each protocol adapter
> reaches L2 conformance against an in-repo simulator. None has been run against
> physical hardware by the project.
>
> OpenMHS is an independent project. It is not affiliated with or endorsed by
> Anthropic, and it does not claim compatibility with Anthropic's Model Hardware
> Standard, whose specification is not public.
>
> Other projects also use the name "OpenMHS"; this one keeps it knowingly. See
> [docs/research/naming.md](docs/research/naming.md).

## What it aims to be

A single trusted gateway that lets any agent discover, inspect, simulate and
operate hardware through one contract — with deterministic policy, a durable
command journal, explicit handling of uncertain outcomes, and readback
verification. Real-time control stays in the device's own controller; hardware
safety systems keep final authority.

## What works today

| Area | Status |
|---|---|
| Specification v0.1 (descriptors, units, lifecycle, authorization, errors, conformance levels) | Draft, in [`spec/`](spec/README.md) |
| JSON Schemas (Draft 2020-12), checked with Rust and Python validators | Implemented |
| Descriptor validation with stable error codes (`openmhs-types`) | Implemented |
| Deterministic policy: grants, approvals, units/ranges, rate and slew limits, preconditions, freshness (`openmhs-policy`) | Implemented |
| Command engine: full lifecycle, idempotency, timeouts → `UNKNOWN_OUTCOME`, readback verification, reconciliation, crash recovery (`openmhs-engine`) | Implemented |
| SQLite journal and hash-chained, tamper-evident audit log | Implemented |
| Adapter contract (`openmhs-adapter`) | Implemented; may change before M7 |
| Virtual lab: temperature sensor, power supply, linear stage, with fault injection (`openmhs-sim`) | Implemented |
| Gateway process with local JSON-RPC API, per-principal tokens, operator/agent roles (`openmhs-gateway`, [spec/api.md](spec/api.md)) | Implemented |
| CLI and guided demo (`openmhs`, `openmhsd`; [docs/cli.md](docs/cli.md)) | Implemented |
| MCP server (`openmhs mcp`): structured refusals with next steps, preflight checks, unattested-text marking; verified with Claude Code ([docs/mcp.md](docs/mcp.md)) | Implemented |
| Python client SDK, standard library only, models checked against the schemas ([python/](python/README.md)) | Implemented; unpublished |
| Agent-loop examples: scripted replanning agent, optional Claude variant ([examples/agent-loop/](examples/agent-loop/README.md)) | Implemented |
| Conformance suite L0–L2 with fault injection and JSON evidence reports (`openmhs-conformance`, [spec/conformance.md](spec/conformance.md)); L3 defined in M9 | Implemented |
| `openmhs adapter new`: a working adapter crate with a fake device and conformance tests ([adapter guide](docs/adapters/README.md)) | Implemented |
| Protocol adapters for Modbus TCP, MQTT, SCPI (TCP/serial) and HTTP, driven by JSON device profiles, each L2 against an in-repo simulator ([profiles](docs/adapters/profiles.md)) | Implemented; untested on physical hardware |
| `openmhs profile check`, `serve --profile` / `--profile-sim`, `conformance --profile` | Implemented |
| Dependency policy: licences, advisories and sources checked by cargo-deny ([deny.toml](deny.toml)) | Implemented |
| OPC UA, ROS 2, out-of-process adapters | Planned (M7) |

## Try it

Requires a Rust toolchain (1.89+). Everything below uses simulated devices only.

```sh
cargo run -p openmhs-cli --bin openmhs -- demo     # guided tour, about 10 seconds
```

The tour runs a real gateway and drives it over its API. It shows:

- deny-by-default, and an agent refused when it tries to grant itself access
- range, unit, slew-limit and stale-telemetry denials
- verified writes, and a safe retry with an idempotency key
- human approval for a hazardous move
- a lost acknowledgement becoming `UNKNOWN_OUTCOME`, then resolved by readback
- an adapter crash contained by the gateway
- audit-log verification

Watch an agent hit a limit, read why, and replan, over MCP with no model and no key:

```sh
cargo build -p openmhs-cli
python examples/agent-loop/mock_agent.py
```

To connect Claude Code or another MCP host, see [docs/mcp.md](docs/mcp.md).

Check the simulators against their declared conformance levels, or start an adapter of
your own:

```sh
openmhs conformance --report-dir reports         # about 15 seconds; a JSON report per device
openmhs adapter new bench-lamp --openmhs-path .  # then follow docs/adapters/README.md
```

Serve Modbus, MQTT, SCPI and HTTP devices from their profiles, each with its simulator
running in the gateway process:

```sh
openmhs serve --profile-sim adapters/openmhs-modbus/profiles/climate-chamber.json \
              --profile-sim adapters/openmhs-mqtt/profiles/greenhouse-node.json
openmhs conformance --profile adapters/openmhs-scpi/profiles/bench-psu.json
```

To connect a real device, write a profile for it and read [docs/hardware/](docs/hardware/README.md)
first.

Or run the gateway yourself:

```sh
openmhs serve --sim                                  # terminal 1
openmhs devices list                                 # terminal 2
openmhs grants add --id me-read --principal operator:local --device '*' --capability '*' --ops read --effect allow
openmhs read sim.thermo-01 temperature.ambient
```

## Tests

```sh
cargo test --workspace                          # spec fixtures, policy, engine, gateway, CLI, MCP
python tools/check_schemas.py                   # schemas behave the same in Python (pip install jsonschema)
python -m unittest discover -s python/tests     # Python SDK, examples, CLI/MCP/SDK parity (cargo build first)
```

## Known limitations

- **Single-user trust boundary.** Any process running as your OS user can read the
  operator token. Scoped, signed credentials come in M8.
- **Stop commands can't interrupt a move.** A stop waits for the in-flight command on
  the same device to finish (see the ADR-0010 amendment). Never rely on OpenMHS for
  emergency stops: those belong to the hardware.
- **Rate and slew limits reset on restart.** Their history is kept in memory.
- **Simulator state resets on restart.**
- **Conformance is not certification.** A level says the integration behaves as
  specified under the tested faults, not that a device is safe to operate. The suite
  really sends commands: run it against fixtures and simulators only.
- **MCP over stdio only.** No HTTP transport, MCP resources or approval notifications
  yet; an agent waiting for approval polls with `get_command`.
- **Protocol adapters are tested against simulators only.** A profile that passes
  conformance is consistent with itself, not proven against your device. There is no
  TLS for MQTT or HTTP yet, no Modbus RTU, and serial SCPI is untested on Windows. See
  each protocol's limits in [docs/adapters/](docs/adapters/profiles.md).

## Documentation

- [Specification](spec/README.md) and the [local API](spec/api.md)
- [CLI reference](docs/cli.md)
- [Connecting an agent over MCP](docs/mcp.md)
- [Writing an adapter](docs/adapters/README.md) and [conformance](spec/conformance.md)
- [Device profiles](docs/adapters/profiles.md): [Modbus TCP](docs/adapters/modbus.md),
  [MQTT](docs/adapters/mqtt.md), [SCPI](docs/adapters/scpi.md), [HTTP](docs/adapters/http.md)
- [Trying real hardware](docs/hardware/README.md) (unverified guides)
- [Python SDK](python/README.md)
- [Landscape research](docs/research/landscape.md)
- [Architecture decisions](docs/adr/README.md)
- [Roadmap](docs/roadmap.md)

## License

Apache-2.0. See [LICENSE](LICENSE).
