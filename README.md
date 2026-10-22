# OpenMHS (working name)

An open, model-agnostic, supervisory interface between AI agents and programmable
hardware.

> **Status: pre-alpha (M1 — specification).** There is no gateway, CLI or hardware
> support yet. What exists: the v0.1 specification, its JSON Schemas, and a Rust crate
> that validates descriptors and encodes the command lifecycle.
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
| Command lifecycle state machine (`openmhs-types::lifecycle`) | Implemented as data + tests; no engine runs it yet |
| Gateway, policy engine, adapters, simulators, CLI, MCP, Python SDK | Planned (M2–M4) |

```sh
cargo test --workspace          # Rust: schema + semantic validation, lifecycle, units
python tools/check_schemas.py   # Python: schemas behave the same (pip install jsonschema)
```

## Documentation

- [Specification](spec/README.md)
- [Landscape research](docs/research/landscape.md)
- [Architecture decisions](docs/adr/README.md)
- [Roadmap](docs/roadmap.md)

## License

Apache-2.0. See [LICENSE](LICENSE).
