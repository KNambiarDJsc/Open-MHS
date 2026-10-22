# OpenMHS (working name)

An open, model-agnostic, supervisory interface between AI agents and programmable
hardware.

> **Status: pre-alpha (M2 — core engine).** The engine runs commands end to end against
> a simulated power supply, as a Rust library. There is no gateway process, CLI, MCP
> server or real-hardware adapter yet, so it is not usable by an agent today.
>
> OpenMHS is an independent project. It is not affiliated with or endorsed by
> Anthropic, and it does not claim compatibility with Anthropic's Model Hardware
> Standard, whose specification is not public.
>
> "OpenMHS" is a provisional name; see [docs/research/naming.md](docs/research/naming.md).

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
| Command engine: full lifecycle, idempotency, timeouts → `UNKNOWN_OUTCOME`, readback verification, reconciliation, crash recovery (`openmhs-engine`) | Implemented, library only |
| SQLite journal and hash-chained, tamper-evident audit log | Implemented |
| Adapter contract (`openmhs-adapter`) | Implemented; may change before M5 |
| Simulated power supply with fault injection (`openmhs-sim`) | Implemented |
| Simulated sensor and actuator, gateway process, CLI | Planned (M3) |
| MCP server, Python SDK | Planned (M4) |
| Real protocol adapters (Modbus, MQTT, SCPI, …) | Planned (M6+) |

```sh
cargo test --workspace          # spec fixtures, policy, engine invariants and scenarios
python tools/check_schemas.py   # Python: schemas behave the same (pip install jsonschema)
```

## Documentation

- [Specification](spec/README.md)
- [Landscape research](docs/research/landscape.md)
- [Architecture decisions](docs/adr/README.md)
- [Roadmap](docs/roadmap.md)

## License

Apache-2.0. See [LICENSE](LICENSE).
