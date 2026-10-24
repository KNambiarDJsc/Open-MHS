# M8 threat model: identity and trust

This covers what M8 adds: scoped credentials, single-use approvals, and signed adapter
manifests. It says what each defense is, which test exercises it, and what it does not
cover. Transport security for adapters is in [transport-security.md](transport-security.md);
the adapter containment model is in
[adapter-host.md](../architecture/adapter-host.md).

> **Status of the tests.** Every test named below passed on Windows on 2026-10-24
> ([IMPLEMENTATION_STATUS.md](../IMPLEMENTATION_STATUS.md) has the full gate). Linux and
> macOS runs are left to CI.

OpenMHS does not claim safety certification, and a signed or approved command is not a
safe command. These controls decide *who may ask* and *what code may run*; whether the
physical action is safe stays with the device policy, the constraints in the descriptor,
and the hardware's own limits.

## Assurances

M8 aims at four statements, and no more:

1. **Verified caller.** Every request is attributed to a principal proved by a credential
   the gateway checked on that request. Nothing in a request body names who is calling.
2. **Precisely scoped operation.** A call needs a scope on the credential *and* a grant in
   policy. Each can only narrow the other. Default is deny.
3. **Valid, non-reusable approval.** Where policy requires approval, one human approver's
   decision covers one exact command, expires, and is spent once.
4. **Trusted adapter artifact.** An adapter launches only if its manifest and its files are
   what a key the operator trusts signed.

## Trust boundaries

```text
   ┌───────────────────────────── same user, same machine ─────────────────────────────┐
   │                                                                                   │
   │  Agent / model        CLI        MCP server        Python SDK        other tools  │
   │      │                 │             │                 │                 │       │
   │      └─────────────────┴──────┬──────┴─────────────────┴─────────────────┘       │
   │                               │  JSON-RPC over loopback HTTP                     │
   │                               │  Authorization: Bearer <credential>              │
   │ ════════════ BOUNDARY 1: every request is authenticated here ════════════════════ │
   │                               ▼                                                   │
   │   ┌──────────────────────── gateway (trusted computing base) ──────────────────┐ │
   │   │ authenticate → Caller{principal, kind, credential, scopes}                  │ │
   │   │ engine: scope ceiling → grants → constraints → approval → dispatch          │ │
   │   │ journal + hash-chained audit (SQLite)   tokens.json  policy.json            │ │
   │   │ trusted_keys.json  gateway.json (operator token)                            │ │
   │   └───────────────┬─────────────────────────────────────────────────────────────┘ │
   │                   │ adapter trait (in process) or JSON-RPC over stdio             │
   │ ═══════ BOUNDARY 2: only signed, verified, staged packages are launched ══════════ │
   │                   ▼                                                               │
   │   adapter process (no principal in its protocol; capabilities limited by manifest) │
   │ ═══════ BOUNDARY 3: the OS sandbox and the device, not OpenMHS ═══════════════════ │
   └───────────────────┼───────────────────────────────────────────────────────────────┘
                       ▼
                 hardware controller (its own interlocks, limits and emergency stop)
```

| Guarantee | Enforced by OpenMHS | Left to the OS sandbox | Left to the hardware |
|---|---|---|---|
| Who called | credential verification, per request | file permissions on the home directory and runtime file | – |
| What they may reach | scope ceiling, grants, constraints | – | – |
| Human approval of a specific command | fingerprint-bound, single-use approvals | – | – |
| Which adapter code runs | signature, digests, verified staging copy | what the process can open or connect to (**not applied in M7/M8**) | – |
| What the adapter says it is | descriptor checked against the manifest | – | – |
| That an action is physically safe | declared constraints, readback verification | – | interlocks, limits, E-stop |
| Isolation between users on a host | – | OS accounts and ACLs | – |

## Threats

Each entry: the attack, the defense implemented, the test, and what remains.

### 1. Credential theft

- **Attack.** An attacker obtains a bearer token: from a log, a process listing, a backup
  of `tokens.json`, or the runtime file.
- **Defense.** Credentials are 256-bit random values. Only their SHA-256 is stored, compared
  in constant time (`subtle`). `Debug` output of credential types redacts the secret. The
  secret is shown once at creation. Credentials can expire (`--ttl`), be rotated, and be
  revoked, effective on the next request. Scopes bound what a stolen credential can reach.
  The audit log never contains secrets.
- **Tested by.** `tokens_are_random_hashed_and_revocable`, `ids_are_not_derived_from_the_secret`
  (gateway store unit tests); `secrets_are_never_stored_or_logged`,
  `a_revoked_credential_is_rejected_immediately`, `an_expired_credential_is_rejected`
  (`crates/openmhs-gateway/tests/credentials.rs`).
- **Remaining.** A bearer token is whoever holds it. The runtime file with the operator
  token is protected by `0600` on Unix and by the user profile's ACL on Windows (a
  dedicated ACL is a tracked item). Any process running as the same user can read it. The
  gateway API is plain HTTP on loopback, so a local process that can observe loopback traffic
  sees tokens. Tokens passed on a command line are visible to other local processes; prefer
  `OPENMHS_TOKEN`. Secrets are not zeroized in memory. A new credential defaults to
  no expiry; set `--ttl` for anything handed to a model.

### 2. Principal spoofing

- **Attack.** A caller claims to be someone else: an agent names `operator:local` in a
  request, asks for a credential carrying another identity's name, or an adapter speaks as a
  principal.
- **Defense.** The principal comes only from the credential's record. Request parameters
  and command bodies have no principal field, and the command schema rejects extra
  fields. `tokens.create` is operator-only, refuses the gateway operator's own principal,
  refuses the operator role for `agent:*` names, and keeps one role per principal. The
  principal kind (human, agent, service) is derived from the id, and agents cannot approve.
  The adapter protocol has no principal at all (threat 10). The CLI has no flag to choose an
  identity; it uses the token it is given or the runtime file's.
- **Tested by.** `an_agent_cannot_claim_another_identity_or_issue_credentials`,
  `issuing_refuses_impersonation_of_the_operator_and_role_mixing`,
  `only_a_human_approver_other_than_the_requester_can_decide`.
- **Remaining.** `Caller::in_process` exists for tests, conformance and embedding, and
  creates a caller with no credential and an open scope ceiling (grants still apply). Code
  linked into the gateway process is trusted by construction. Its audit trail says
  `auth_method: in_process`. The operator can create a credential for any non-reserved
  principal name.

### 3. Scope escalation

- **Attack.** A credential reaches a device, capability or operation beyond what it was
  issued for, or widens itself.
- **Defense.** The scope check runs first and before any device lookup, so an out-of-scope
  call is refused identically whether or not the device exists. Scopes cannot be changed
  after issue; rotation copies them. A credential with no scopes authorizes nothing, and
  credentials written before M8 have none. A scope never replaces a grant: both must allow.
  An approver's credential must reach what it approves. Only the operator role can issue,
  rotate or revoke credentials.
- **Tested by.** `a_credential_cannot_reach_a_device_outside_its_scope`,
  `a_credential_cannot_exceed_its_operations_or_capabilities`, `a_scope_never_replaces_a_grant`,
  `a_credential_without_scopes_authorizes_nothing`, `rotation_replaces_a_credential_without_widening_it`,
  `agent_credentials_cannot_administer`, `tokens_written_before_m8_load_but_authorize_nothing`.
- **Remaining.** Scopes use the same `*` patterns as grants; a broad pattern is a broad
  scope. The operator can issue `--any-scope`. There are no time windows or attribute
  conditions.

### 4. Expired and revoked credentials

- **Attack.** A credential keeps working after it should have stopped.
- **Defense.** Expiry and revocation are read on every request from the gateway's own state
  and clock; there is no cache and no token lifetime to wait out. Revocation is effective
  for the next request (**revocation latency: none beyond the request in flight**). A
  revoked credential stays on record so use of it is distinguishable from a guess, and the
  first refused use is audited once. Revoking withdraws that credential's open approvals and
  cancels the commands waiting on them. The check fails closed: if the token file cannot be
  read at start, the gateway does not start.
- **Tested by.** `an_expired_credential_is_rejected`, `a_revoked_credential_is_rejected_immediately`,
  `revoking_a_credential_withdraws_its_open_approvals`, `credentials_expire_exactly_at_their_time`,
  `revoking_twice_is_harmless_and_reported`.
- **Remaining.** A command already dispatched is not recalled by revoking the credential
  that sent it. The credential rejected response distinguishes expired and revoked from
  unknown, to anyone holding the secret. The clock is the host's; a changed system clock
  changes expiry.

### 5. Approval replay

- **Attack.** An approval is used twice, or concurrently, to run a command more than once.
- **Defense.** Spending an approval is one SQLite transaction that checks `APPROVED`,
  expiry, requester, binding and fingerprint, marks it `CONSUMED`, and moves the command to
  `APPROVED` in the same commit. A second attempt sees `CONSUMED`. The device lock and the
  journal's single connection serialize concurrent attempts.
- **Tested by.** `an_approval_cannot_be_used_twice`, `a_spent_approval_is_not_reusable`,
  `concurrent_consumption_runs_one_command`, `concurrent_approvals_do_not_duplicate_execution`
  (engine); the state machine itself by property tests in `openmhs-types`.
- **Remaining.** A spent approval whose command then fails (device error, timeout) is not
  restored; the requester asks again. Approvals are rows in the same database as the
  commands; they are not independently signed.

### 6. Approval modification

- **Attack.** A command is changed after a human approved it, or an approval for one command
  is presented for another or by someone else.
- **Defense.** An approval carries the SHA-256 of the canonical command: requester, device,
  capability, operation, value and parameters. At spending, the fingerprint is recomputed
  from the command as stored at that moment and must match; the requester must be the one
  who asked; the approval must not be bound to a different command. A mismatch spends nothing
  and, for a held command, denies it and revokes the approval. Policy is evaluated again at
  the moment of use, so an approval does not outlive the grant.
- **Tested by.** `a_modified_command_is_rejected`, `another_principal_cannot_spend_an_approval`,
  `tampering_with_a_held_command_is_caught_at_approval`, `policy_still_applies_after_approval`,
  `an_approval_requested_ahead_runs_exactly_its_command`, `approvals_are_only_issued_for_commands_that_need_one`.
- **Remaining.** The fingerprint does not cover idempotency key, correlation id or
  deadline (they affect delivery, not meaning). Someone who can write the database can
  rewrite the approvals table and the audit chain together; the audit chain detects edits
  in the middle, not truncation of its tail (known since M2).

### 7. Malicious manifests

- **Attack.** An adapter package ships a manifest that widens its permissions, or someone
  edits a vetted manifest.
- **Defense.** A manifest carries an Ed25519 signature over its canonical form (including the
  digest of every file). Any change to any field invalidates it. The signer must be in the
  operator's registry, unrevoked and allowed for that adapter id. Signed or not, the manifest's
  own limits are enforced: capabilities are checked against the descriptor at every start. A
  valid signature never widens them.
- **Tested by.** `a_tampered_manifest_is_rejected`, `a_malformed_signature_is_rejected`,
  `an_unsigned_manifest_is_refused_by_default`,
  `a_valid_signature_does_not_override_the_manifests_permissions`,
  `a_correctly_signed_manifest_is_accepted` (`crates/openmhs-host/tests/trust.rs`).
- **Remaining.** A trusted signer can sign something harmful; the signature says who, not
  whether it is safe. Endpoint and filesystem entries are still cooperative (checked by the
  SDK), and no OS sandbox is applied, so a signed adapter that ignores them is not stopped
  (M7 limitation, unchanged).

### 8. Adapter binary replacement

- **Attack.** The adapter's program is swapped after review, or between the check and the
  launch.
- **Defense.** Every file in the signature's artifact list is hashed from the bytes that are
  read, compared to the signed digest, and copied to a private staging directory (`0700` on
  Unix, exclusive create, files `0500`). The process is launched from the copies, and
  restarts use the same copies, so replacing the original after verification changes
  nothing. The launch command may not name a file inside the package that the signature does
  not cover. Links and non-regular files are refused.
- **Tested by.** `a_modified_executable_is_rejected`, `a_missing_artifact_is_rejected`,
  `a_launch_command_outside_the_signature_is_rejected`, `the_adapter_runs_from_the_verified_copy`.
- **Remaining.** Only listed files are covered. The interpreter (`python`), standard
  library, installed packages, native libraries, the Python SDK reached through
  `PYTHONPATH`, and anything the adapter reads at run time are outside the signature. A
  package that runs `python -m module` imports from the working directory, not from the
  staged copy; sign script-style packages. A same-user attacker can modify the staging
  directory on Windows, where `0700` has no equivalent here.

### 9. Untrusted signing keys

- **Attack.** An unknown key signs an adapter, a trusted key's id is forged, a compromised key
  keeps signing, or a key meant for one vendor signs another's adapter.
- **Defense.** Trust is a local registry (`trusted_keys.json`); the API cannot change it. A
  key id is the SHA-256 of the public key, so it cannot name another key, and it is part of
  the signed bytes. Keys carry the adapter ids they may sign. Revoked keys stay listed and
  are refused (`signer.revoked`).
- **Tested by.** `unknown_revoked_and_out_of_scope_signers_are_rejected`,
  `a_forged_key_id_does_not_borrow_trust`, and the registry unit tests.
- **Remaining.** **Revocation latency for a running adapter is until the gateway restarts:**
  the registry is read at start, and a running adapter is not stopped when its key is
  revoked. Anyone who can edit the gateway home can edit the registry. There is no key
  expiry, no transparency log, and no signed timestamp, so a signature does not say when
  it was made.

### 10. Adapter impersonation

- **Attack.** An adapter process sends requests as a principal, submits commands, or
  pretends to be another device.
- **Defense.** The adapter protocol has no principal and no submit method. The host treats a
  request from the adapter as a protocol violation and does not act on it. Commands exist only
  through `Engine::submit`, which takes a `Caller` built by the gateway. An adapter's
  descriptor is checked against the manifest on every start and must not change across
  restarts.
- **Tested by.** `an_adapter_cannot_submit_commands_or_name_a_principal`,
  `requests_from_the_adapter_are_refused_without_harm`.
- **Remaining.** An adapter is trusted for what its device says: it can report false readings
  or a false outcome. It runs with the user's privileges and can read the files and
  environment that user can (no OS sandbox).

### 11. Authorization bypass through public interfaces

- **Attack.** A path to the engine that skips authentication or authorization: an
  unauthenticated method, a client that picks its own identity, or an administrative call
  an agent can reach.
- **Defense.** The gateway is the only door (ADR-0001): the CLI, MCP server and SDKs are
  clients of its API. Every request needs a verified bearer credential before any method
  runs; requests without JSON content type are refused before authentication (a browser
  cannot drive the gateway). The engine's entry points take a `Caller`, not a string.
  Administration requires the operator role. `commands.get/list/cancel/reconcile` show an
  agent only its own commands, and approvals are visible to their requester and to
  approvers.
- **Tested by.** `every_method_requires_authentication`, `agent_credentials_cannot_administer`,
  `missing_and_invalid_credentials_are_rejected`, `request_approve_submit_over_the_api`.
- **Remaining.** The MCP server refuses an operator token unless told otherwise, but a user
  can pass one. Anything that can reach the loopback port and holds a token can act as it.
  Listening beyond loopback (`--allow-remote`) is unauthenticated plaintext HTTP apart from
  the token; do not.

## Not mitigated by M8

- A compromised operator account, or any process running as the operator's OS user.
- A malicious or compromised signer, or a compromised build pipeline that signs.
- An adapter that ignores its manifest's endpoint and filesystem declarations (no sandbox).
- Tampering by someone who can write the gateway home: policy, tokens, trusted keys and the
  database. The audit chain detects edits in the middle but not truncation of its tail.
- A stolen bearer token used from the same machine within its lifetime.
- TLS for the gateway's own API (loopback plaintext).
- Signed audit export and signed device descriptors: **not implemented in this milestone**.
- Physical safety, which stays with constraints, readback verification and the hardware.
