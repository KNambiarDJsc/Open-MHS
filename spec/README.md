# OpenMHS Specification v0.1 (draft)

This directory is the source of truth for OpenMHS. Implementations — including the
Rust reference implementation in `crates/` — follow it, and tests keep them in sync.

> Draft status: v0.1 may change incompatibly until v1.0. OpenMHS is independent and
> not affiliated with Anthropic; it makes no claim of compatibility with Anthropic's
> Model Hardware Standard.

| Document | Defines |
|---|---|
| [descriptor.md](descriptor.md) | Device descriptors, capabilities, value specs, constraints, preconditions, verification, validation rules |
| [units.md](units.md) | Quantity kinds, UCUM unit table, conversion rules |
| [lifecycle.md](lifecycle.md) | Command states, transitions, outcome classes, idempotency, recovery, receipts |
| [authorization.md](authorization.md) | Principals, grants, approval, defaults, what descriptors may not do |
| [errors.md](errors.md) | Error object and the registry of stable error codes |
| [conformance.md](conformance.md) | Conformance levels L0–L3 |
| [api.md](api.md) | Local gateway API: transport, authentication, roles, methods |
| [adapter-protocol.md](adapter-protocol.md) | Adapter-host protocol: JSON-RPC over stdio, lifecycle, errors, supervision |
| [permission-manifest.md](permission-manifest.md) | Adapter permission manifest: what is host-enforced, cooperative, OS-enforced |

Machine-readable contracts (JSON Schema Draft 2020-12) live in [`schemas/`](schemas/):

| Schema | `$id` |
|---|---|
| `common.schema.json` | `urn:openmhs:schema:0.1:common` |
| `descriptor.schema.json` | `urn:openmhs:schema:0.1:descriptor` |
| `command.schema.json` | `urn:openmhs:schema:0.1:command` |
| `receipt.schema.json` | `urn:openmhs:schema:0.1:receipt` |
| `event.schema.json` | `urn:openmhs:schema:0.1:event` |
| `error.schema.json` | `urn:openmhs:schema:0.1:error` |
| `adapter-manifest.schema.json` | `urn:openmhs:schema:0.1:adapter-manifest` |

Schema IDs are URNs because the project does not own a domain; they are identifiers,
not URLs to fetch.

Examples in [`examples/`](examples/):

- `valid/` — three dissimilar devices (temperature sensor, power supply, linear
  actuator) plus the base used to derive invalid fixtures.
- `invalid/` — descriptors that each break exactly one rule; `expected.json` maps each
  file to the error code it must produce. Regenerate with
  `python tools/gen_invalid_fixtures.py spec/examples/invalid`.
- `commands/`, `receipts/`, `events/` — wire examples for the other schemas.

## Conventions

- The key words MUST, MUST NOT, SHOULD and MAY are used as in RFC 2119.
- Identifiers (devices, adapters, components, rule IDs) are lowercase:
  `^[a-z0-9]([a-z0-9._-]*[a-z0-9])?$`, max 128 characters.
- Capability names are dotted lowercase segments (`power.output.voltage.setpoint`).
  Vendor-specific names MUST be prefixed `x-<vendor>.` (`x-acme.turbo.mode`).
- Timestamps are RFC 3339 in UTC.

## Versioning

- The spec version is `MAJOR.MINOR` and appears in every descriptor (`"openmhs": "0.1"`).
- Before 1.0, any minor version may break compatibility. From 1.0, minor versions only
  add optional fields and new codes.
- Capabilities MAY carry their own `version` so a capability's semantics can evolve
  independently of the spec.
- Implementations MUST reject descriptors with a spec version they do not support.
