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
| M8 | **Built on branch `m8`; tests written; not run** | **No.** See below |

## M8: identity and trust

| Component | Built | Tests written | Tests run |
|---|---|---|---|
| A. Scoped credentials (`openmhs-policy::auth`, gateway token store) | Yes | Yes | Credential store unit tests and the credentials integration tests were run once, partway through the milestone (workspace: 238 passed, 0 failed, with `OPENMHS_PYTHON` set). Not rerun since the approvals and manifest work |
| B. Approval workflow (`openmhs-types::approval`, engine, journal) | Yes | Yes | **No** |
| C. Signed adapter manifests (`openmhs-host::trust`) | Yes | Yes | **No** |
| D. Shared authenticated execution context (`Caller` through engine, gateway, CLI, MCP, SDK) | Yes | Yes | **No** |
| E. Threat model ([M8_THREAT_MODEL.md](security/M8_THREAT_MODEL.md)) | Yes | n/a | n/a |

Also written: [ADR-0017](adr/0017-identity-and-trust.md); updates to
`spec/authorization.md` (sections 5 and 7), `spec/permission-manifest.md` (Signatures),
`docs/cli.md`, `docs/adapters/python-sdk.md`, `docs/mcp.md`.

### Not run since the M8 changes

- `cargo test --workspace` (the approvals, manifest-signing, impersonation and gateway
  bypass tests have therefore not passed or failed; they compile)
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo deny` (licences and advisories for `ed25519-dalek` 3 and its dependencies)
- `tools/check_schemas.py` (the manifest schema gained a `signature` property)
- The Python unit suites, `openmhs demo`, and the conformance suite, including ROS 2

Until these run, M8 cannot be called complete, and its security properties are design
intent backed by tests that exist, not by tests that have passed. A failure in an
authorization test should be treated as a possible vulnerability, not as a test to adjust.

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
| M1–M7 behavior still works | The existing suites, updated for `&Caller`; **not rerun** |

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
- `Client::create_token` takes `CreateTokenParams`.
- `serve --adapter-manifest` refuses unsigned manifests unless `--allow-unsigned-adapters`.
  `HostOptions::default()` requires signatures.

### Database changes

One new table, `approvals`, with indexes, created with `CREATE TABLE IF NOT EXISTS` when
the journal opens, like the existing tables (the project has no migration framework). The
command table gains no columns. Existing journals open unchanged. Credentials remain in
`tokens.json`; files written before M8 load, with no scopes. A new registry file,
`trusted_keys.json`, appears in the gateway home when the first key is trusted.
