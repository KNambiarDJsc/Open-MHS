# ADR-0003: Small typed authorization engine now; Cedar re-evaluated in M8

- Status: Accepted
- Date: 2026-10-23

## Context

Policy has two different jobs:

1. **Authorization** — may this principal perform this operation on this capability
   of this device? Possibly requiring approval.
2. **Constraint validation** — are the inputs physically acceptable: schema, units,
   range, rate, slew, preconditions over fresh state?

Constraint validation is domain logic over typed values and state; it belongs in
Rust code driven by the descriptor. Authorization could use a general policy
language. `cedar-policy` (4.14.0 on crates.io, actively maintained) is the leading
Rust option: analyzable, deny-by-default, with formal verification behind it.

## Decision

- v0.1 uses a small, typed authorization engine: grants of the form
  `principal × device pattern × capability pattern × operation → allow | require_approval`,
  deny by default, explicit deny overrides allow. Every decision carries the rule IDs
  that produced it.
- It sits behind an `Authorizer` trait so it can be swapped.
- Constraint validation is separate code and always runs after authorization; both
  must pass.
- Cedar is re-evaluated in M8 (identity, trust, approvals), when attribute-based
  rules (time windows, roles, side-effect classes) make a policy language pay off.

## Alternatives considered

- **Cedar from day one.** Strong, but adds a policy language users must learn before
  the first demo, and a significant dependency, for rules that are currently a table.
- **OPA/Rego.** Requires a sidecar or WASM runtime; heavier; not Rust-native.
- **LLM-evaluated policy.** Non-deterministic. Rejected permanently.

## Consequences

- Fast to build, easy to test exhaustively, easy to explain in denials.
- A migration to Cedar later must preserve rule-ID reporting; the trait keeps that
  contained.
