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
physical evidence truth, cross-gateway fencing, full filesystem/power-loss drills, and the
high-level SDK recovery API remain unproven or unimplemented. No production-readiness claim.

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

## Next implementation milestone

Add a receipt-aware SDK API that submits once with a stable key, retrieves the durable
status even after a lost response, waits within a bounded deadline, and surfaces explicit
reconciliation or human action without retrying an ambiguous physical command. Follow
that with resource ownership/fencing semantics before adding distributed transport.
