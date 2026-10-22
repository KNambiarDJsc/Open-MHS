# OpenMHS (working name)

An open, model-agnostic, supervisory interface between AI agents and programmable
hardware.

> **Status: pre-alpha, design stage (M0).** Nothing is implemented yet. This
> repository currently contains research and architecture decisions only.
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

- [Landscape research](docs/research/landscape.md)
- [Architecture decisions](docs/adr/README.md)
- [Roadmap](docs/roadmap.md)

## License

Apache-2.0. See [LICENSE](LICENSE).
