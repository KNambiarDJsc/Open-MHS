# Execution kernel hardening: code audit and scope

Status: implementation notes for the first reliability milestone. This document describes
the checked-in Rust implementation; it is not a production-readiness claim.

## Current module boundaries

The dependency direction is layered. `openmhs-types` owns wire and domain types, schemas,
canonical encodings and the command transition table. `openmhs-policy` evaluates credential
scopes, grants, approvals and command constraints. `openmhs-engine` owns lifecycle execution,
device serialization, recovery and the SQLite journal. `openmhs-adapter` defines the device
boundary. `openmhs-host` supervises out-of-process adapters. `openmhs-gateway` authenticates
RPC requests and supplies the verified caller to policy and engine operations. `openmhs-api`
contains the shared RPC/client types, `openmhs-mcp` and `openmhs-cli` are clients, and
`openmhs-conformance`/`openmhs-sim` provide simulator validation.

The execution path remains in one gateway process:

```text
MCP / CLI / SDK -> API client -> authenticated gateway -> engine -> adapter/host -> device
                                                   |          |
                                                   |          +-> SQLite journal
                                                   +-> policy and scopes
```

## Existing guarantees found in code and specs

- Deny by default; a request needs both a credential scope and a matching policy grant.
- State-changing commands serialize per device from stateful checks through verification;
  declared stops use a separate interrupt lane.
- `(principal, device, idempotency_key)` is unique in SQLite. Reuse with a changed payload
  is rejected by request-hash comparison.
- `DISPATCHED`, `ACCEPTED`, and timed-out work can recover as `UNKNOWN_OUTCOME`; recovery
  does not resend it. An unresolved non-idempotent capability is held pending reconciliation.
- A verification or reconciliation reading must postdate dispatch (ADR-0020).
- Command rows contain serialized receipts. Command transitions update the row and append
  the corresponding audit record in one SQLite transaction. A separate hash-chained audit
  table records other events.
- Policy, devices, state cache, storage settings and per-device locks are process memory;
  plans, approvals, leases, commands and audit entries are journaled. Rate/slew history
  starts empty after restart, as documented in `spec/lifecycle.md`.

The policy is loaded from a local JSON file and changed through operator RPC. The current
implementation does not persist a monotonic policy revision number. This milestone reports
a content digest instead, so identical policy content has the same revision label.

## Changes in this milestone

The MCP plan schema no longer shows a concrete example device id. `get_insights` verifies
the requested device exists before it summarizes journal rows. New `access.allowed_operations`
RPC and MCP `get_allowed_operations` enumerate operations for the authenticated principal
after intersecting credential scopes with current policy. The response carries policy and
descriptor SHA-256 digests, generation/expiry timestamps, and an advisory marker. It is not
an authorization token; the engine checks every command at dispatch.

## Not implemented in this milestone

The lifecycle already has a versioned state enum and an exhaustive transition table; the
main gap is broader fault/property testing, not an absent state machine. Distributed
PostgreSQL, transport, edge replication, native fencing, external audit anchors, receipt
digests, signed checkpoints, OpenTelemetry and deployment automation remain later work.
They must be justified against the standalone journal and tested invariants before adoption.

The LabThings adapter currently reports external invocation references when available. Its
server-side invocation lookup can disappear on restart; recovery still treats an ambiguous
dispatch as unknown. This work does not claim that a native invocation can always be queried.
