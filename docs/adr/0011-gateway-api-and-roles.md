# ADR-0011: Gateway API, roles and home directory

- Status: Accepted
- Date: 2026-10-23
- Builds on: [ADR-0001](0001-single-trusted-gateway.md), [ADR-0005](0005-local-api-transport.md)

## Context

M3 turns the engine into a running gateway with a local API and a CLI. ADR-0005 fixed
the transport (JSON-RPC 2.0 over loopback HTTP with a bearer token). Several things it
left open had to be settled before the MCP server (M4) becomes a client:

- One shared token would let an agent with API access change its own permissions.
- A client that disconnects in the middle of a command must not cancel the command
  half-way through its lifecycle.
- Grants must survive restarts, and two gateways must never share one journal.

## Decision

1. **Two roles, token per principal.**
   - The gateway writes a fresh *operator* token to the runtime file on every start.
   - Operators create further tokens for named principals with role `operator` or
     `agent`. Tokens are 256 random bits, shown once, and stored only as SHA-256
     hashes in `tokens.json`.
   - Agents can read devices, submit commands, and see, cancel or reconcile only their
     own commands.
   - Policy, tokens, approvals, the audit log and fault injection are operator-only
     (`gateway.forbidden` otherwise).
   - Roles never bypass grants. An operator's reads and commands are authorized like
     anyone else's.
2. **Commands run detached from the HTTP request.** `commands.submit`, `approve` and
   `reconcile` run in their own task, so a client timeout or disconnect cannot drop the
   engine's future mid-lifecycle. A test proves the command still reaches a defined
   state.
3. **Home directory.** `OPENMHS_HOME`, else the per-user local data directory + `openmhs`.
   It holds:
   - `journal.sqlite`
   - `policy.json` (grants and approvers; created on first start with no grants and the
     operator as the only approver)
   - `tokens.json`
   - `gateway.json` (the runtime file)
   - `gateway.lock`

   Policy and token files are rewritten atomically.
4. **One gateway per home.** The gateway holds an exclusive OS file lock on
   `gateway.lock`; a second gateway on the same home refuses to start. The OS releases
   the lock if the process dies, so a crash never blocks a restart.
5. **Requests must be `application/json`.** This forces a CORS preflight, which the
   gateway never answers, so a web page in the user's browser cannot drive it.
6. **MSRV raised to Rust 1.89**, for the standard library's file locking (`File::try_lock`).
7. **Simulators are opt-in** (`serve --sim`). Fault injection (`sim.fault`) exists only
   in simulation mode and every injected fault is written to the audit log.

## Alternatives considered

- **Single shared token (ADR-0005 as written).** Simplest, but any client could edit
  policy. Rejected; this ADR refines ADR-0005.
- **Policy in SQLite.** Keeps one store. Rejected for v0.1: a JSON policy file can be
  reviewed, diffed and edited by hand, which matters for a safety-relevant config. The
  audit log still records every change made through the API.
- **Cancelling commands when the client disconnects.** Would leave commands in
  `DISPATCHED` until restart, contradicting the lifecycle spec.

## Consequences

- Any process running as the same OS user can read the runtime file and act as the
  operator. That is the v0.1 trust boundary. Signed, scoped credentials come in M8.
- On Windows the runtime and token files rely on the per-user profile's default ACL; a
  dedicated ACL is a tracked hardening item.
- Edits made to `policy.json` by hand take effect on the next start and are not
  audited. Only API edits are audited.
- Simulator state is not persisted, so a gateway restart is like power-cycling every
  simulated device. Recovery still behaves correctly: in-flight commands become
  `UNKNOWN_OUTCOME` and reconcile against the device as it is now.
