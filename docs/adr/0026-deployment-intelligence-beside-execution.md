# ADR-0026: Deployment intelligence sits beside execution; the gateway is one optional source

- Status: Proposed
- Date: 2026-11-15
- Builds on: [ADR-0001](0001-single-trusted-gateway.md), [ADR-0002](0002-command-lifecycle.md)
- Design: [docs/architecture/deployment-intelligence](../architecture/deployment-intelligence/README.md)

## Context

Open-MHS is a governed command gateway: it is in the actuation path of every command it mediates. The proposed product
answers different questions (why did this task fail, which release was involved, did a newer policy help) for robots
whose commands never pass through Open-MHS: ROS 2 action servers, VLA policies, vendor controllers. Teams will not
route their control traffic through a new process to get incident reports, and a failure of Open-MHS must never stop
a robot.

## Decision

1. The deployment-intelligence layer is **observation-only and out of band**. It reads events, recordings and
   deployment metadata. It sends nothing to robots and is never consulted before an action.
2. Robots keep running if every Open-MHS component is down, full or unreachable. Instrumentation (SDK, collector)
   drops and counts rather than blocks.
3. The existing gateway stays a supported product for governed execution of lab and bench devices, with its feature
   work frozen. It becomes **one event source** through an exporter that reads its audit chain (not the ephemeral
   state fabric, ADR-0023).
4. The uncertainty discipline of ADR-0002 carries over: a timeout, a missing acknowledgement, a stale observation and
   a verified result stay different things in the event model (ADR-0033).

## Alternatives considered

- **Grow the gateway into the platform**: it would put the product in the control path and demand that teams adopt
  Open-MHS for actuation first. Rejected.
- **Retire the gateway**: throws away tested work some users may want, gains nothing. Rejected; freeze instead.
- **Observability only, drop the uncertainty model**: loses the one property that distinguishes this work. Rejected.

## Consequences

- New crates and storage, separate from the journal (ADR-0029, ADR-0034).
- The gateway's adapters, conformance suite, plans and procedures receive fixes only.
- Every claim of the new layer is about what was observed or reported, never a guarantee about the robot.
