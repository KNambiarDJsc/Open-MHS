# Kernel hardening validation — 2026-10-30

Branch: `codex/execution-kernel-hardening`. This continues the agent-facing correctness
milestone (`e629cb6`) with durable command receipt evidence and recovery validation.

## Implemented behavior

- New command creation/transition audit events bind a canonical receipt digest in the
  same transaction as the receipt row. Status, listings, duplicate-key replay and later
  transitions check available digests. Recovery detects an indexed state that differs
  from the receipt rather than silently skipping the command.
- Operator-only `audit.export` takes a consistent full audit/receipt snapshot. The
  `audit verify-file` CLI verifies the portable file independently of the gateway and
  optionally compares an independently retained checkpoint.
- Verification reports missing/duplicate receipts, altered receipt fields, pointer
  mismatches, chain damage and incomplete legacy coverage. Export refuses file overwrites.
- Persisted-state restart tests cover CREATED, VALIDATING, APPROVED, DISPATCHED,
  ACCEPTED, TIMED_OUT, APPLIED and UNKNOWN_OUTCOME, recovering twice without device
  execution. Accepted native invocation references survive reopen/recovery.
- Generated lifecycle histories check that a command cannot redispatch after uncertainty
  or reach APPLIED/VERIFIED without the preceding acknowledgement/dispatch states.

## Checks executed

| Command | Final result |
|---|---|
| `cargo test --workspace` | 456 passed, 0 failed, including doc tests |
| `cargo clippy --workspace --all-targets -- -D warnings` | passed |
| `cargo fmt --check` | passed |
| `git diff --check` | passed |
| `python -m pytest python/tests -q` | 55 passed |
| `python -m unittest adapters/python/wot/test_td_import.py` | 25 passed |

Gateway integration tests ran with localhost networking permitted. No external model API
or physical hardware was used. The full 225-run research experiment suite was not rerun;
its historical data remains unchanged. New tests establish only the scenarios they exercise.

Two test expectations changed deliberately: corrupt held receipts now return
`journal.receipt_integrity` instead of producing a fresh denial receipt; the Python example
tool-conversion test now includes the new read-only permissions tool (23 tools total).

## Compatibility and limits

Receipt wire schemas and existing chain hash bytes are preserved. Audit event JSON gains
a versioned `receipt_digest` field. SQLite adds an audit-subject index. Existing journals
remain readable, but receipts whose latest legacy audit event lacks a digest are reported
as incomplete coverage. They are not retroactively authenticated. `audit verify` retains
its original chain-only behavior; `audit verify-file` requires valid and complete coverage
for exit status 0.

Checkpoints are unsigned. Independent retention is an operator responsibility; the bundle's
own head cannot protect against a consistent full rewrite. A privileged actor controlling
both journal and retained checkpoints remains outside this mechanism's protection. Full
exports use memory proportional to journal contents. Signatures, automated external anchors,
physical evidence truth, cross-gateway fencing, and full filesystem/power-loss drills remain
unproven or unimplemented. The high-level SDK recovery API is covered only by the simulated-
gateway follow-up below. No production-readiness claim.

## Exact files in this milestone

- `CHANGELOG.md`, `Cargo.lock`
- `crates/openmhs-api/src/client.rs`, `crates/openmhs-api/src/lib.rs`
- `crates/openmhs-cli/src/evidence.rs`, `crates/openmhs-cli/src/lib.rs`, `crates/openmhs-cli/tests/cli.rs`
- `crates/openmhs-engine/src/journal.rs`, `crates/openmhs-engine/tests/approvals.rs`,
  `crates/openmhs-engine/tests/evidence.rs`, `crates/openmhs-engine/tests/recovery_windows.rs`
- `crates/openmhs-gateway/src/server.rs`
- `crates/openmhs-types/Cargo.toml`, `crates/openmhs-types/src/evidence.rs`,
  `crates/openmhs-types/src/lib.rs`, `crates/openmhs-types/src/lifecycle.rs`
- `docs/architecture/AUDIT_EVIDENCE.md`, `docs/architecture/DISTRIBUTED_MIGRATION_PLAN.md`,
  `docs/architecture/EXECUTION_KERNEL_V2.md`, `docs/architecture/FAILURE_INVARIANTS.md`,
  `docs/architecture/KERNEL_HARDENING_VALIDATION.md`
- `docs/cli.md`, `python/tests/test_examples.py`, `research/AUDIT_AND_PROVENANCE.md`,
  `spec/api.md`, `spec/errors.md`

## SDK recovery follow-up — 2026-10-30

The Python SDK now provides `Client.execute()` and an `Execution` handle. It submits once
with a generated or caller-supplied idempotency key, bounds the submit HTTP call, recovers a
lost response through principal-scoped `(device, idempotency_key)` lookup, supports bounded
status polling, and requires an explicit `reconcile()` call for `UNKNOWN_OUTCOME`. It does
not automatically resend or reconcile. Callers must persist a supplied key before
submission if they need recovery after their own process restarts.

The new `commands.get_by_key` API returns only the authenticated principal's receipt and
uses the existing opaque `command.unknown` response when no matching record is visible.
Validation: `cargo test -p openmhs-gateway` passed; `cargo clippy -p openmhs-api
-p openmhs-gateway --all-targets -- -D warnings` passed; Python SDK suite: 58 passed.
These tests exercise simulated gateway behavior only; physical recovery and resource
fencing remain unproven.

## Local resource serialization follow-up — 2026-10-30

Commands now hold their device lock and every topology-declared shared-resource lock from
live checks through verification. Multiple resource locks are taken in sorted order.
Registration and topology changes cannot alter the graph while a command holds that lock
set. Reconciliation and recovery verification use the same locks. Stops retain the
existing stop lane and bypass these locks. An unresolved command also refuses writes to
other devices in its shared-resource claim until reconciliation or operator resolution.
`resource_serialization.rs` verifies shared-resource exclusion, the unresolved hold, and
that unrelated devices still can execute concurrently.

This is a guarantee within one gateway process. It is not a fencing epoch and does not
stop another gateway process, a direct controller, or a stale network client outside that
process. The next ownership milestone is an explicit versioned owner epoch validated at
the controlling endpoint where supported, with a fail-closed local boundary where it is
not.

## Next implementation milestone

Add versioned ownership epochs and stale-owner checks before distributed transport. Claim
fencing only when the underlying controller can enforce it.
