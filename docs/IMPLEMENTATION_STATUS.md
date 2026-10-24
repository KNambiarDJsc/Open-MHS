# Implementation status

What exists, and how much of it has been checked. "Built" means the code compiles in the
workspace. "Verified" means the named tests were run and passed. This page is updated at
the end of each milestone and says plainly when something has not been run.

Last updated: 2026-10-24, on branch `m8` (not pushed, not merged).

## Milestones

| # | Status | Verified |
|---|---|---|
| M0–M6 | Done, on `main` | Yes, at their milestones |
| M7 | Done on branch `m7` (`a30aa63`), awaiting review and a push decision | Full gate run at the milestone, except the ROS 2 conformance (Docker was unavailable) |
| M8 | Built on branch `m8`; full gate run on Windows except ROS 2 | Yes, see below |

## M8: identity and trust

| Component | Built | Tests |
|---|---|---|
| A. Scoped credentials (`openmhs-policy::auth`, gateway token store) | Yes | Pass |
| B. Approval workflow (`openmhs-types::approval`, engine, journal) | Yes | Pass |
| C. Signed adapter manifests (`openmhs-host::trust`) | Yes | Pass |
| D. Shared authenticated execution context (`Caller` through engine, gateway, CLI, MCP, Python SDK) | Yes | Pass |
| E. Threat model ([M8_THREAT_MODEL.md](security/M8_THREAT_MODEL.md)) | Yes | n/a |

Also written: [ADR-0017](adr/0017-identity-and-trust.md); updates to
`spec/authorization.md` (sections 5 and 7), `spec/permission-manifest.md` (Signatures),
`docs/cli.md`, `docs/adapters/python-sdk.md`, `docs/mcp.md`. The Python SDK gained
scoped `create_token`, `rotate_token`, the approvals methods and `submit(approval_id=)`.

### Gate results (Windows 11, 2026-10-24)

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass |
| `cargo test --workspace` | 294 passed, 0 failed (after two test fixes, below) |
| `cargo deny check` | advisories, bans, licences, sources ok |
| `tools/check_schemas.py` | 57 documents against 8 schemas ok |
| Python SDK, examples and CLI/MCP/SDK parity (`python/tests`) | 38 passed, 0 skipped (Python 3.11) |
| OPC UA adapter unit tests | 11 passed |
| `openmhs demo` | every step behaved as expected |
| Conformance: built-in simulators, four protocol profiles, Python PSU, OPC UA | all reached their declared levels |
| ROS 2 conformance | **Not run**: Docker Desktop not running on this machine (also not rerun at M7) |
| Python 3.10, Linux, macOS | **Not run** locally; CI covers them once the branch is pushed |

Failures found by the first run and how they were resolved:

- `a_modified_command_is_rejected` (test bug). It sent `params` on a write, which the
  command schema forbids, so the request was refused before approvals were consulted. The
  test now asserts that refusal and checks directly that the fingerprint covers `params`.
- `inv08_core_has_no_device_specific_code` (test bug, caused by M8). It read
  `src/engine/` as a file. It now walks subdirectories, so the new approvals module is
  checked too.
- Python suites: 8 failures and 2 errors, from the documented breaking change. The SDK's
  `create_token` could not send scopes, so every credential it made authorized nothing
  (`auth.scope.denied`, the intended default). Fixed in the SDK; tests issue
  `any_scope=True` credentials, as the Rust tests do. The parity test's credential-id
  normalizer was updated for M8's random 16-digit ids.

No security test was weakened, and no security failure was found.

### Required end-to-end tests and where they are

| Required test | Test |
|---|---|
| Valid credential can read | `a_valid_scoped_credential_can_read` |
| Missing or invalid credential rejected | `missing_and_invalid_credentials_are_rejected`, `every_method_requires_authentication` |
| Expired credential rejected | `an_expired_credential_is_rejected` |
| Revoked credential rejected | `a_revoked_credential_is_rejected_immediately` |
| Wrong device scope rejected | `a_credential_cannot_reach_a_device_outside_its_scope` |
| Wrong operation scope rejected | `a_credential_cannot_exceed_its_operations_or_capabilities` |
| Approval-required command cannot dispatch without approval | `a_held_command_gets_a_pending_approval_bound_to_its_fingerprint` |
| Approved command executes | `approved_commands_run_and_spend_their_approval` |
| Modified approved command rejected | `a_modified_command_is_rejected`, `tampering_with_a_held_command_is_caught_at_approval` |
| Expired approval rejected | `an_expired_approval_is_rejected` |
| Revoked approval rejected | `a_revoked_approval_is_rejected` |
| Approval cannot be consumed twice | `an_approval_cannot_be_used_twice` |
| Concurrent consumption does not duplicate | `concurrent_consumption_runs_one_command` |
| Correctly signed manifest accepted | `a_correctly_signed_manifest_is_accepted` |
| Tampered manifest rejected | `a_tampered_manifest_is_rejected` |
| Modified executable rejected | `a_modified_executable_is_rejected` |
| Unknown or revoked signer rejected | `unknown_revoked_and_out_of_scope_signers_are_rejected` |
| Adapter IPC cannot bypass the gateway | `an_adapter_cannot_submit_commands_or_name_a_principal` |
| M1–M7 behavior still works | The existing Rust, Python, demo and conformance suites; pass |

### Known limitations

See the threat model's "Not mitigated" list. In short: same-user attackers, a compromised
signer, adapters without an OS sandbox, plaintext loopback HTTP, no zeroization of secrets
in memory, no signed audit export or signed descriptors, and the interpreter and libraries
of a signed adapter are not covered by its signature.

### Breaking changes

- Credentials without scopes authorize nothing. `tokens create` requires `--scope` or
  `--any-scope`.
- Engine entry points take `&Caller`. `Journal::insert_command` takes an origin.
  `Resolved` has a `dir` field.
- `Client::create_token` takes `CreateTokenParams`. The Python `Client.create_token`
  requires `scopes=[...]` or `any_scope=True`.
- `serve --adapter-manifest` refuses unsigned manifests unless `--allow-unsigned-adapters`.
  `HostOptions::default()` requires signatures.

### Database changes

One new table, `approvals`, with indexes, created with `CREATE TABLE IF NOT EXISTS` when
the journal opens, like the existing tables (the project has no migration framework). The
command table gains no columns. Existing journals open unchanged. Credentials remain in
`tokens.json`; files written before M8 load, with no scopes. A new registry file,
`trusted_keys.json`, appears in the gateway home when the first key is trusted.
