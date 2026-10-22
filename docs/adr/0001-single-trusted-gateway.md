# ADR-0001: Single trusted gateway process; all interfaces are clients

- Status: Accepted
- Date: 2026-10-23

## Context

Agents will reach hardware through several interfaces: CLI, MCP, Python SDK, and
later HTTP/gRPC. If each interface embeds the engine as a library, each process has
its own registry, journal and policy state. Two processes could then dispatch
conflicting commands, idempotency could not be enforced across them, and "no
interface can bypass policy" would be a convention rather than a structural fact.

## Decision

One process — the gateway (`openmhsd`, also `openmhs serve`) — owns the registry,
command engine, journal, policy, state store, audit log and adapters. Every
interface is a client of the gateway's local API (ADR-0005). Client crates do not
depend on the engine or adapter crates; this is enforced by the workspace
dependency graph and checked in CI.

The engine is still a normal library crate, so it can be tested in-process and
embedded in tests and the gateway binary.

## Alternatives considered

- **Library-only, embedded per interface.** Simplest to start; breaks idempotency
  and single-writer guarantees as soon as two clients run. Rejected.
- **Gateway plus a "direct mode" for the CLI.** Convenient, but it is exactly the
  bypass path we promise does not exist. Rejected; `openmhs demo` starts a gateway.

## Consequences

- The CLI needs a running gateway for most commands; `openmhs demo` and
  `openmhs serve` make that one step.
- One process is a single point of failure; acceptable for a local supervisory
  gateway. Multi-host and failover are M11+ topics.
- Invariant 10 ("no client can bypass the gateway") becomes testable by
  inspecting the dependency graph plus end-to-end tests.
