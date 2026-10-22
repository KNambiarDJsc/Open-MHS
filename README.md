# OpenMHS

An open, model-agnostic, supervisory interface between AI agents and programmable
hardware.

> **Status: pre-alpha (M3: gateway and CLI).** A local gateway serves three simulated
> devices through an authenticated API and a CLI. There is no MCP server and no
> real-hardware adapter yet: it controls simulators only, and an AI agent can't
> connect to it directly until M4.
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
| Adapter contract (`openmhs-adapter`) | Implemented; may change before M5 |
| Virtual lab: temperature sensor, power supply, linear stage, with fault injection (`openmhs-sim`) | Implemented |
| Gateway process with local JSON-RPC API, per-principal tokens, operator/agent roles (`openmhs-gateway`, [spec/api.md](spec/api.md)) | Implemented |
| CLI and guided demo (`openmhs`, `openmhsd`; [docs/cli.md](docs/cli.md)) | Implemented |
| MCP server, Python SDK | Planned (M4) |
| Real protocol adapters (Modbus, MQTT, SCPI, …) | Planned (M6+) |

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

Or run the gateway yourself:

```sh
openmhs serve --sim                                  # terminal 1
openmhs devices list                                 # terminal 2
openmhs grants add --id me-read --principal operator:local --device '*' --capability '*' --ops read --effect allow
openmhs read sim.thermo-01 temperature.ambient
```

## Tests

```sh
cargo test --workspace          # spec fixtures, policy, engine invariants, gateway, CLI end to end
python tools/check_schemas.py   # Python: schemas behave the same (pip install jsonschema)
```

## Known limitations

- **Single-user trust boundary.** Any process running as your OS user can read the
  operator token. Scoped, signed credentials come in M8.
- **Stop commands can't interrupt a move.** A stop waits for the in-flight command on
  the same device to finish (see the ADR-0010 amendment). Never rely on OpenMHS for
  emergency stops: those belong to the hardware.
- **Rate and slew limits reset on restart.** Their history is kept in memory.
- **Simulator state resets on restart.**

## Documentation

- [Specification](spec/README.md) and the [local API](spec/api.md)
- [CLI reference](docs/cli.md)
- [Landscape research](docs/research/landscape.md)
- [Architecture decisions](docs/adr/README.md)
- [Roadmap](docs/roadmap.md)

## License

Apache-2.0. See [LICENSE](LICENSE).
