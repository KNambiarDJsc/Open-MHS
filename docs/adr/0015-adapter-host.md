# ADR-0015: Out-of-process adapters and the adapter host

- Status: Accepted
- Date: 2026-10-24
- Builds on: [ADR-0008](0008-languages.md) (Python for adapters), [ADR-0013](0013-conformance-suite.md), [ADR-0014](0014-protocol-adapters.md)

## Context

OPC UA and ROS 2 have good Python libraries (`asyncua`, `rclpy`) and no mature Rust
ones, and `rclpy` only exists inside a ROS installation. ADR-0014 named out-of-process
adapters as the escape hatch. A separate process can crash, hang, print garbage, or be
written by someone we do not know, so the gateway must contain it without weakening the
guarantees the rest of the system makes: authorization, the command lifecycle and the
audit trail stay in the gateway.

## Decision

**The adapter host is a `HardwareAdapter`.** `ProcessAdapter` (`crates/openmhs-host`)
implements the existing trait by relaying calls to a child process. The engine, policy,
journal and conformance suite are unchanged.

**Protocol:** JSON-RPC 2.0, one message per line, over the child's stdin and stdout
([spec/adapter-protocol.md](../../spec/adapter-protocol.md)). Host-initiated requests;
adapter notifications only for progress and fixture counts. Version negotiation in
`initialize`; typed errors with the adapter error class in `data.kind`; deadlines on
every call. Chosen over gRPC, sockets or MCP because it needs no ports, no code
generation and no dependencies on either side, and works in a container with `-i`.

**Containment:**

- stdout carries protocol only; anything else kills the process. The Python SDK moves
  fd 1 aside so stray `print`s go to stderr.
- Heartbeats (`ping`); missed ones kill the process.
- A crash or kill fails every in-flight call as `adapter.disconnected`, so in-flight
  commands become `UNKNOWN_OUTCOME`. They are never resent. The host never retries a call.
- Restarts back off exponentially within a budget. Beyond it, or for faults a restart
  cannot fix (unsupported version, descriptor outside the manifest or changed, missing
  credential), the adapter is quarantined and `execute` returns `adapter.not_sent`.
- The child gets a cleared environment plus an OS allow-list, the manifest's `env`, and
  named `credentials` (redacted from forwarded stderr).

**Not a second policy engine.** The host forwards only commands policy has already
allowed and validated. The manifest's capability allow-list is an extra host-enforced
ceiling on what a descriptor may declare, never a way to grant more.

**Permission manifest** ([spec/permission-manifest.md](../../spec/permission-manifest.md)):
every field is labelled host-enforced, cooperative (checked by the SDK), or OS-enforced.
In M7 no OS-level sandbox is applied, and the documents say so. Network, serial and
filesystem entries are cooperative: they catch mistakes and document intent, they do not
contain a hostile adapter. Running the command in a container (as the ROS 2 manifest
does) is the supported way to get real isolation today.

**Conformance:** SDK adapters pass the same suite through the host. The Python PSU
matches the Rust PSU check for check.

## Alternatives considered

- **Embed Python (PyO3).** Rejected: a crash takes the gateway down, and rclpy needs its
  own environment.
- **A Rust OPC UA crate.** Rejected for now; revisit when one matches asyncua on
  security and reconnection.
- **Claim a sandbox with seccomp/Landlock/job objects now.** Rejected: unfinished,
  platform-specific restrictions would read as a guarantee. Planned work, documented as
  such.

## Consequences

- One process per adapter. No subscriptions across the protocol (the adapter contract
  has none).
- Quarantine is cleared only by restarting the gateway.
- Supervising `docker run` supervises the client; a container hung beyond Python's reach
  may outlive a kill (documented).
- A Python adapter is trusted with its device's credentials. The manifest limits what
  the gateway hands over, not what the adapter does with it.
