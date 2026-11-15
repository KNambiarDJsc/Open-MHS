# ADR-0027: Import first; an edge agent only when importing is not enough

- Status: Proposed
- Date: 2026-11-15
- Builds on: [ADR-0026](0026-deployment-intelligence-beside-execution.md)

## Context

Robot teams already record (rosbag2/MCAP), already have CI that knows what it deployed, and already run fleet or
log tools (Foxglove, Roboto, Formant, InOrbit). A new on-robot agent is a deployment, a security review, a resource
budget and a support burden. Nobody has asked Open-MHS for one. Open-MHS's own experiments show that a recording
plus run metadata is enough to rebuild task attempts, ROS goal outcomes and their timing
(`research/outcome-verifier/ros2-evaluation`).

## Decision

1. The first product path is **import**: an MCAP recording (or directory of them) plus a **deployment manifest** and
   optional operator notes become events in a local store. No process runs on the robot.
2. The second path is the **SDK writing to a local spool file**, which the same importer reads. Still no daemon.
3. A **collector** (a lightweight edge process with buffering and upload) is built only when a design partner needs
   live or continuous ingestion that files cannot give, and it reuses the journal's quota, reserve and sealed-segment
   patterns.

## Alternatives considered

- **Agent first** (the prompt's Layer 1): the most invasive path, built before anyone has used the analysis it feeds.
  Deferred, not rejected.
- **OpenTelemetry collector as the only ingestion path**: good for services, poor for ROS bag contents and deployment
  manifests. Kept as a later source (ADR-0031).

## Consequences

- Demo and design-partner trials need only files a team already has.
- Ingestion is batch and idempotent from day one, which the collector later reuses.
- Live alerting is out of scope until the collector exists.
