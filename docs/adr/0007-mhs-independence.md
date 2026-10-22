# ADR-0007: Independent of Anthropic MHS; compatibility via a future façade

- Status: Accepted
- Date: 2026-10-23

## Context

Anthropic's Model Hardware Standard is a closed research preview with no published
specification (see `docs/research/landscape.md`). Anthropic states it intends to
open-source it later. Many community projects call themselves "open MHS".

## Decision

- OpenMHS defines its own specification and makes no claim of MHS compatibility.
- All docs state the project is independent and unaffiliated with Anthropic.
- The core model (descriptors, capabilities, lifecycle) is not shaped around guesses
  of the MHS wire format. If MHS is published, compatibility is added as:
  - a **front-end façade** (speak MHS to agents, translate to the core), and/or
  - a **descriptor importer** (MHS driver metadata → OpenMHS descriptors),
  each written against the published spec only.

## Consequences

- No rework of the core if MHS publishes something different from press coverage.
- Positioning is "runtime correctness + conformance + real adapters", which stays
  valuable whether or not MHS becomes dominant (see the naming recommendation in
  `docs/research/naming.md`).
