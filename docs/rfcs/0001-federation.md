# RFC 0001: Federation: several gateways, one lab, local authority

- **Status:** draft
- **Author:** OpenMHS maintainers (drafted with Claude Code)
- **Date:** 2026-10-28
- **Touches:** API / policy / security model / spec

## Summary

Let gateways on different computers see each other's devices and forward commands to one
another, while **command authority never leaves the gateway that owns a device**. A
gateway can reach a peer's device only by asking the peer, and the peer applies its own
policy to a principal it knows. Nothing here is built. The roadmap gates building it on
three independent sites running single gateways (M20).

## Motivation

A lab spans machines: a supply on the bench PC, a robot arm on its controller's host, a
chamber on a Raspberry Pi. Today an agent needs one MCP connection per gateway, sees no
cross-gateway capability graph, and cannot run one plan that touches devices on two hosts.
Without federation, users connect everything to one host (long serial cables, one point
of failure) or write their own glue, which will not keep the guarantees.

## Design

### Identities and trust

- Each gateway has an Ed25519 identity, generated at first start in its home, plus a
  human-readable site name.
- A peer is added by an operator on both sides (`openmhs peers add <name> <public key>
  <url>`), never discovered into trust. Removing a peer is immediate on that side.
- Transport is mutually authenticated TLS between the two identities (ADR-0016 rules:
  verified, no insecure mode). Loopback-only gateways cannot federate.

### What crosses between gateways

| Item | Direction | Notes |
|---|---|---|
| Descriptors and availability | Owner to peer, signed by the owner | Shown as `<site>/<device>`; read-only on the peer |
| Events | Owner to peer, pulled with `events.poll` | Filtered by the owner, against what the remote principal may `subscribe` to on the owner |
| Commands and plans | Peer to owner, forwarded | Authorized by the owner's policy, against the remote principal |
| Audit | Each gateway keeps its own chain | Peers exchange checkpoint hashes (storage RFC §6) so a rewrite is detectable |

A forwarded call carries the original principal, as `site:<name>/agent:<x>`, and the
forwarding gateway's identity. The owner's grants name remote principals explicitly. A
remote principal has no rights until the owner's operator grants them.

### Commands across a partition

- A forwarded command is submitted to the owner with an idempotency key the peer derives
  from its own command id. Resending after a lost answer returns the same receipt.
- If the peer cannot reach the owner, the command is refused (`peer.unreachable`) with
  nothing sent. No command is queued across a partition.
- If the request may have reached the owner and the answer was lost, the peer records
  `UNKNOWN_OUTCOME` and the owner's receipt is the truth. Reconciliation always asks the
  owner, never the device directly.
- During a partition, the peer shows remote devices as `UNREACHABLE` (a new availability
  value, distinct from `OFFLINE`, which is what the owner says).

### Leases and fencing

- A lease on a remote device is taken from the owner, which issues a **fencing token**: a
  number that only increases, per device.
- Every forwarded command carries the token. The owner refuses a token older than the
  newest it has issued for that device (`lease.fenced`). So a peer that lost its lease
  during a partition cannot act on the strength of it after the partition heals.
- A plan that spans gateways holds one lease per owner. If any owner becomes unreachable,
  the plan stops at the next step boundary and waits for a person (`AWAITING_DECISION`).
  It never compensates across a partition by itself.

### Stops

A stop to a remote device is forwarded like any command, with priority. A stop that cannot
reach the owner is `UNKNOWN_OUTCOME` with the stop guidance of ADR-0019. The equipment's
own emergency stop remains authoritative. Federation adds no stop path.

## Safety and security

- **No new authority.** Every physical command is still authorized, checked, journaled and
  verified by exactly one gateway, the owner, with its own policy. A compromised peer can
  ask; it cannot decide.
- **Explicit trust only.** Peers are pinned by key on both sides. There is no auto-join and
  no transitive trust: A trusting B and B trusting C does not let A reach C.
- **Partitions fail closed.** Nothing is queued, a lost answer is `UNKNOWN_OUTCOME`, stale
  leases are fenced, and cross-gateway plans stop for a person.
- **Local-first holds.** A gateway never needs a peer, or a cloud, to stop or operate its
  own devices.
- **New attack surface:** a network listener for peers. It needs a threat-model update (M8)
  before implementation: replayed forwarded requests (covered by idempotency keys and
  fencing), a peer impersonating principals (the owner grants remote principals
  explicitly, with the site name), and event-stream volume (bounded by the owner's ring).

## Compatibility

Additive. Single gateways are unchanged. New fields:
- `site` on devices seen remotely;
- an availability value `UNREACHABLE`;
- error codes `peer.unreachable` and `lease.fenced`;
- audit kinds `peer.added`, `peer.removed` and `peer.forwarded`.

## Alternatives considered

- **A central coordinator that holds authority.** Rejected: it violates local-first, is one
  point of failure, and puts a network hop inside every safety decision.
- **Replicated policy, where any gateway authorizes any device.** Rejected: two
  authorities for one device can disagree during a partition.
- **Consensus (Raft) for a shared journal.** Rejected for now: heavy, and physical effects
  are not log entries; the owner's receipt is already the single source of truth.
- **Doing nothing**, and keeping one MCP connection per gateway. It is a valid interim
  answer, and is what users should do until this is built.

## How it will be tested

- Two gateways in one test process on loopback with mutual TLS.
- A forwarded command verifies on the owner and the receipt matches on both sides.
- Partition injection (drop the peer link mid-command) yields `UNKNOWN_OUTCOME` on the peer
  and exactly one effect on the device. Reconciling from the peer asks the owner.
- A lease fenced after a partition is refused by the owner.
- An unknown peer key is rejected at TLS.
- A remote principal without an owner grant is refused.

## Open questions

- Should events be pushed instead of pulled between gateways, once push exists for
  clients?
- How long may a forwarded plan wait for an unreachable owner before it must be decided?
- Key rotation for gateway identities: overlap window, and how peers learn the new key.
- Do we need per-site rate limits on forwarded commands?
