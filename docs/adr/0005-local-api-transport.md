# ADR-0005: Local API is JSON-RPC 2.0 over HTTP on loopback, token-authenticated

- Status: Accepted
- Date: 2026-10-23

## Context

Clients (CLI, MCP server, Python SDK) talk to the gateway (ADR-0001). The transport
must work identically on Windows, macOS and Linux, be easy to call from Python with
no build toolchain, and not be reachable from the network by default.

## Decision

- JSON-RPC 2.0 over HTTP, served with `axum`, bound to `127.0.0.1` on a configurable
  port. Binding to any non-loopback address requires an explicit flag and is logged
  as a warning.
- On start the gateway writes a random bearer token and its address to a runtime
  file in the user's config directory, readable only by the current user (POSIX
  `0600`; on Windows the file lives under the user profile, and an ACL restriction
  is tracked as a hardening item). Clients read the file; requests without the
  token are rejected.
- Request/response types are defined once in a shared crate and mirrored by the
  JSON Schemas in `spec/`.

## Alternatives considered

- **Unix sockets / Windows named pipes.** Better OS-level access control, but two
  code paths and weaker tooling on Windows. Possible later as an additional
  transport.
- **gRPC.** Strong typing and streaming, but needs a protobuf toolchain for every
  client language and is heavier for a local tool.
- **Plain REST.** Fine too; JSON-RPC matches MCP's style and maps one-to-one to
  operations.

## Consequences

- Any process running as the same user can read the token. That is the trust
  boundary for v0.1 and is documented in the threat model (M8 adds per-principal
  credentials).
- Telemetry streaming will need server-sent events or WebSocket next to JSON-RPC
  (M3/M4 decision).
