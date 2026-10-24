# ADR-0017: Identity and trust

- Status: Accepted
- Date: 2026-10-24
- Builds on: [ADR-0001](0001-single-trusted-gateway.md), [ADR-0003](0003-authorization-engine.md),
  [ADR-0011](0011-gateway-api-and-roles.md), [ADR-0015](0015-adapter-host.md)

## Context

Before M8 the gateway authenticated callers with hashed bearer tokens and decided with
deny-by-default grants. Three things were missing for use beyond one trusted operator:

- A token carried a principal and a role, but no limit on *what* it could reach, no expiry,
  and a revocation that was a flag with no history.
- An approval was a state of a command (`AWAITING_APPROVAL`) with an approver name in the
  receipt. It could not be requested ahead of time, was not tied to what the command said
  when it was approved, and could not expire on its own clock.
- The adapter host (M7) bounded what an adapter declared, but nothing said *whose* adapter
  it was. A modified manifest or program started like any other.

The constraint is not to build a second system. The gateway is the single door; the engine,
journal and audit chain exist; the permission manifest exists.

## Decision

**One verified identity, passed to the engine as a value.** Authentication produces a
`Caller` (`openmhs-policy::auth`): principal, kind, credential id, method and scopes. The
engine's entry points take `&Caller` instead of a principal string, so there is no engine
call that can be made without one. Code in the gateway's own process uses
`Caller::in_process`, which is audited as such. The adapter protocol has no principal and
no submit method, so an adapter cannot construct a `Caller`.

**Credentials stay opaque bearer tokens.** `omhs_` + 256 random bits, SHA-256 stored,
constant-time compare, random ids, expiry, rotation, revocation retained on the record. No
JWT: a stateless token cannot be revoked before it expires without keeping state, and we
want revocation to take effect on the next request. The gateway already keeps the state.

**Scopes are a ceiling, not a grant.** A scope (`device:capability:operations`) limits a
credential; a policy grant allows a principal. A call needs both. This keeps one policy
engine and makes a stolen or over-shared credential less valuable than its principal. A
credential with no scopes authorizes nothing, so the safe state is the default.

**Approvals are durable records in the command journal.** A SQLite table beside the
commands, changed in the same transaction as the command and its audit record. States
`PENDING → APPROVED → CONSUMED`, with `DENIED`, `EXPIRED` and `REVOKED` as other ends. The
approval holds a fingerprint (SHA-256 of canonical JSON of requester, target, operation,
value, parameters) recomputed from the stored command when the approval is spent; spending
is one transaction, so a second or concurrent attempt sees `CONSUMED`. Two ways in, one
record: policy holds a command and an approval is bound to it; or the requester asks first
and submits with the approval id. Policy is evaluated again at spending, so an approval
never overrides a grant, a scope or a constraint.

**Manifests are signed with Ed25519, verified before launch, and launched from a
verified copy.** The signature is inside the manifest and covers its canonical form,
including the digest of each adapter file. The host reads each file once, hashes the bytes,
compares to the signed digest, and copies those bytes to a private directory from which the
adapter runs and restarts; there is no gap between checking and executing. The signer must
be in a local registry (`trusted_keys.json`, operator-managed, not in the API), unrevoked
and allowed for the adapter id. A signature never relaxes the manifest's own limits. The
default is to require signatures; `--allow-unsigned-adapters` is an explicit development
switch. Conformance fixtures use an unsigned mode that is only reachable for simulators.

**Libraries.** `ed25519-dalek` (signatures, `zeroize` for key material), `sha2`, `subtle`
(constant-time compare), `getrandom`. No custom cryptography, and no new service.

## Alternatives considered

- **JWT or macaroons.** Self-contained scoping, but revocation needs a deny list, which is
  the state we already have; and parsing attacker-supplied structure on the door adds
  surface. Rejected.
- **Scopes as grants.** Letting a credential carry its own allowances would make a second
  policy language. Rejected: scopes only narrow.
- **A separate approvals service or table store.** A second durable store cannot be changed
  atomically with the command. Rejected.
- **Signature outside the manifest (detached `.sig`).** Easier to drop and to forget.
  Rejected; the manifest is self-describing.
- **X.509 or Sigstore for adapter signing.** Needs a CA or online service, against the
  local-first goal. Candidates for a later milestone if adapters are distributed publicly.
- **Verifying then launching the original files.** Leaves a window to swap them. Rejected
  for the staged copy.
- **Cedar for policy.** Considered at M3. M8 needs scopes and approvals, not a new
  language; revisit with time windows and attributes.

## Consequences

- **Breaking.** Tokens without scopes authorize nothing; `tokens create` requires `--scope`
  or `--any-scope`. Engine methods take `&Caller`. `Client::create_token` takes a params
  struct. `serve --adapter-manifest` requires signed manifests unless
  `--allow-unsigned-adapters`; `HostOptions::default()` requires signatures.
- **Revocation latency.** Credentials: the next request. Approvals: immediately, and a
  credential's revocation withdraws its open approvals. Signing keys: the next gateway
  start; a running adapter is not stopped.
- **Fail closed.** An unreadable token file, a registry that does not parse, an unknown
  signer, a digest mismatch, and an expired or spent approval all refuse.
- **Not solved here.** A same-user attacker, a compromised signer or build, adapters
  without an OS sandbox, TLS on the gateway's own API, signed audit export and signed
  descriptors. The threat model lists what is and is not defended
  ([M8_THREAT_MODEL.md](../security/M8_THREAT_MODEL.md)).
