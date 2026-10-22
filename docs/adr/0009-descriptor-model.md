# ADR-0009: Descriptor model choices for spec v0.1

- Status: Accepted
- Date: 2026-10-23

## Context

M1 defines the device descriptor. Several choices were not settled by earlier ADRs.

## Decision

1. **JSON only in v0.1.** Descriptors are JSON validated by JSON Schema 2020-12. YAML
   is friendlier to write, but the main Rust YAML crate (`serde_yaml`) is unmaintained
   and the forks are young. YAML input can be added later as a front-end that converts
   to JSON before validation.
2. **Three capability kinds: property, action, event.** The master plan listed a fourth
   kind, `stream`. Telemetry streams are modelled as properties with the `subscribe`
   operation instead, which avoids two ways of saying the same thing.
3. **Descriptors cannot grant permissions.** A descriptor is a claim by its author. If
   it could set "default: read-write", writing a descriptor would be a way to grant
   yourself access. Permissions come only from operator policy (`spec/authorization.md`);
   the schema rejects unknown top-level fields such as `permissions`.
4. **Preconditions are structured comparisons, not an expression language.** Each is
   `{capability, op, value}` combined with AND. No parser, no variables, nothing to
   sandbox. OR and richer logic can be added when a real device needs them.
5. **Verification is mandatory for state-changing capabilities.** A writable or
   invokable capability must declare `readback` or `none` with a reason, so "we never
   checked" is always visible.
6. **Every number carries its unit on the wire.** `{"value": 5, "unit": "V"}`. See
   ADR-0004.
7. **Schema IDs are URNs** (`urn:openmhs:schema:0.1:…`) because the project owns no
   domain.

## Consequences

- Cross-field rules (units, references, kind/operation compatibility) can't be
  expressed in JSON Schema, so they live in semantic validation with stable error
  codes (`spec/errors.md`). Other implementations must reimplement them; the invalid
  fixtures in `spec/examples/invalid/` are the shared test set.
- A bare integer like `5` is structurally valid (integers are allowed for `integer`
  capabilities), so the "numbers need units" rule for `number` capabilities is
  enforced when a command is checked against its descriptor (M2), not by the command
  schema alone.
