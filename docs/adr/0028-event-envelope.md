# ADR-0028: A small versioned event envelope; relationships, not one trace; no universal ontology

- Status: Accepted (reviewed 2026-11-15/16)
- Date: 2026-11-15
- Specification: [RFC 0005](../rfcs/0005-deployment-events.md)

## Context

Robot stacks disagree on almost everything: task models (behaviour trees, RMF phases, VLA rollouts), identifiers,
clocks and message types. A common ontology of robot behaviour would be wrong for most of them and obsolete within a
year for learning-based systems. Yet correlation needs a few fields that mean the same thing everywhere.

## Decision

1. One envelope, `openmhs.event/0.1`, with a **small closed set of normalized fields**: identity, source and ordering,
   time with clock domain and uncertainty, scope (tenant, site, robot, workcell), subject ids (task, episode, action,
   goal), trace context, deployment reference, event type, outcome with provenance, artifact references, and typed
   relations.
2. Everything else is preserved verbatim in **namespaced extensions** (`ext.ros2`, `ext.lerobot`, `ext.vendor.<name>`),
   never interpreted by the core.
3. Events relate through **typed edges** (`part_of`, `follows`, `retry_of`, `responds_to`, `derived_from`,
   `same_as`), so one action can belong to a task and an intervention at once. There is no `caused_by`.
4. **Deployment manifests are separate, content-addressed records**; events carry the manifest digest.
5. The schema evolves additively within a major version; consumers ignore unknown fields; a major version change gets
   a converter.

## Alternatives considered

- **Adopt OpenTelemetry spans as the model**: spans assume one tree per trace and a known duration; robot tasks are
  graphs with uncertain ends. Trace ids are carried as fields instead.
- **Adopt a robotics ontology (e.g. a full ISO 21423 mapping)**: covers mobile-robot fleet interop, not manipulation,
  learning or releases. Map to it at the edge when a user needs it.
- **Free-form JSON only**: no correlation possible. Rejected.

## Consequences

- The core can be small and stable; integrations carry the source-specific detail.
- Some questions cannot be answered without the source's own ids (see the critical review, question 8).
