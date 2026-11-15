# ADR-0031: Ops API: HTTP and JSON with idempotent batch ingestion; OTLP and gRPC later

- Status: Accepted (reviewed 2026-11-15/16)
- Date: 2026-11-15
- Refines for the new layer: [ADR-0005](0005-local-api-transport.md), [ADR-0011](0011-gateway-api-and-roles.md)

## Context

The gateway uses JSON-RPC 2.0 on one loopback route, which suits an agent session. Event ingestion comes from CI
pipelines (a `curl` after a deploy), robot SDKs in several languages, importers and, later, collectors over unreliable
links. Queries come from a CLI, a web page and MCP tools.

## Decision

1. The ops service exposes **HTTP/JSON resources** under `/v1/`: `POST /v1/events:batch`, `POST /v1/deployments`,
   `GET /v1/incidents`, `GET /v1/incidents/{id}/timeline`, `GET /v1/releases/compare`, `POST /v1/incidents/{id}/handoff`
   (contracts in [05-api.md](../architecture/deployment-intelligence/05-api.md)).
2. **Ingestion is idempotent per event id**: a repeated event is `duplicate`; the same id with different content is a
   recorded `conflict`, never an overwrite. A batch answers per event.
3. Authentication reuses the gateway's hashed-token model for Stage 1 (one host). Tokens carry tenant/site/robot
   scopes. mTLS or OIDC arrive with Stage 2.
4. **The CLI works without the service**, directly on `ops.sqlite`, for local investigation.
5. OTLP (logs and traces) is a later ingestion source; gRPC only if a client needs streaming at Stage 3.

## Alternatives considered

- **Reuse the gateway's JSON-RPC**: consistent, but awkward for CI and per-event batch results, and couples the
  observation product to the actuation process. Rejected.
- **gRPC first**: code generation burden on Python and robot teams for no measured gain. Deferred.

## Consequences

- Two API styles in one repository, kept in separate crates and documented as such.
- MCP tools for incidents call the ops API, read-only.
