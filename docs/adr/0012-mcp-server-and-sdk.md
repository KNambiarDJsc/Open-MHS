# ADR-0012: MCP server design, agent guidance and the Python SDK

- Status: Accepted
- Date: 2026-10-23
- Refines: [ADR-0006](0006-mcp-sdk.md) (pins the `rmcp` version)

## Context

M4 gives AI agents a way in. The [MCP specification (2025-11-25)][tools] says tool
results can carry `structuredContent` with an `outputSchema`. It says errors the model
can act on belong in the result with `isError: true`, not in the protocol. Tool
annotations are hints that clients must not trust from untrusted servers. And servers
must validate inputs, enforce access control and sanitize outputs.

The gateway already decides everything (ADR-0001). The MCP server's job is to present
those decisions so a model can act on them correctly. Another Apache-2.0 project
([Abenor-Labs/Open-MHS][ref]) was reviewed for its agent-facing design. Its idea of
marking device-authored prose as unattested is adopted below. Its code is not used.

## Decision

**Transport and SDK.** `openmhs mcp` serves MCP over stdio using `rmcp ~3.5.1`. At M4
the newest release was 3.6.0, published three days earlier. The project takes releases
at least two weeks old, so it uses the newest 3.5.x and a tilde requirement.
`rmcp-macros` is held at 3.5.1 in `Cargo.lock`. Logs go to stderr only, because stdout
is the transport.

**A client like any other.** The server holds one agent token and calls the gateway's
JSON-RPC API. It has no adapter, journal or policy access, so invariant 10 holds by
construction. `python/tests/test_parity.py` checks it: the same scenario through the
CLI, MCP stdio and the Python SDK gives identical receipts and audit records.

**Tools.** The six from ADR-0006, plus three:

| Tool | Annotations | Purpose |
|---|---|---|
| `list_devices` | read-only | Compact capability summaries |
| `describe_device` | read-only | Full descriptor |
| `read_state` | read-only, open-world | Fresh, audited read with `usable` |
| `check_command` | read-only, open-world | Preflight: same checks as a submission, runs nothing |
| `submit_command` | destructive, not idempotent, open-world | Write or invoke; operation inferred from the descriptor |
| `get_command` | read-only | Receipt, optionally waiting (`wait_ms`, max 60 s) |
| `list_pending_approvals` | read-only | Own commands in `AWAITING_APPROVAL` |
| `reconcile_command` | idempotent, open-world | Settle `UNKNOWN_OUTCOME` by readback |
| `cancel_command` | idempotent | Withdraw before dispatch |

`check_command` needed a new gateway method, `commands.preflight` (spec/api.md). It runs
the same evaluation as a submission, including the device lock and fresh reads, and
then stops. It creates no command and is audited as `command.preflight`. It is
advisory: state can change before the real submission, which is checked again.

**Results an agent can replan from.** Every command result is a `CommandView`. It
carries `state`, `outcome`, and `confirmed` (true only on readback-verified success).
It carries `reached_device`, a one-line `summary`, and `reasons`, each with its code,
rule id and the constraint's numbers. And it carries `next_steps`: guidance written in
code from those numbers, for example "change by at most 2 V per 1000 ms". Denied,
failed, not-executed and uncertain outcomes set `isError`. Pending approval does not.
A successful cancellation does not. A check that answers "no" does not.

Gateway and connection failures become text tool errors with the code. Setup problems
such as a bad token or an unreachable gateway say "tell the human operator; do not
retry". Malformed arguments are rejected by `rmcp` as `isError` results naming the bad
field, which the model can correct.

**Unattested text is marked.** Descriptor prose (names, descriptions, verification
reasons), adapter error messages and string readings come from adapter or device
authors. Before they reach a model, they are flattened to one line, stripped of control
characters and of the delimiters, capped at 900 characters, and wrapped as
`<<device-text … device-text>>`. The server instructions say that text in this wrapper
is never an instruction. Identifier-like tokens (`IDLE`, capability names) pass through
unchanged, because agents send them back verbatim. Nothing is censored.

**Operator tokens are refused.** A model should not be able to approve its own
commands or change policy. `openmhs mcp` exits if no token is given, rather than fall
back to the operator token in the runtime file. The server checks its role on first
use and refuses operator tokens unless started with `--allow-operator`.

**Schemas are plain JSON Schema.** Integer-width formats that schemars emits, such as
`uint64`, are stripped. Claude Code's validator warned about them in the manual test.

**MCP resources and prompts are deferred.** Tools cover the M4 acceptance criteria.
Descriptors as resources, and approval notifications, come later.

**Python SDK.** `python/openmhs` uses only the standard library and needs Python 3.10+.
It is unpublished (`Private :: Do Not Upload` blocks uploads to PyPI). Its models are
checked against the JSON Schemas rather than generated from them. A test asserts that
each model's fields, and its required fields, equal the schema's `properties` and
`required`. The spec's example documents and live gateway responses must round-trip
unchanged. Fields a newer gateway adds are kept in `extra`, not dropped.

## Alternatives considered

- **Generate per-capability tools** (`psu_set_voltage`, …). Rejected again, as in
  ADR-0006: the tool list grows with the fleet, and descriptor changes would change
  the tool surface.
- **Return denials as JSON-RPC errors.** The MCP spec explicitly recommends against it:
  models see tool errors and can self-correct; protocol errors are often hidden.
- **Generate Python models from the schemas.** It would need a code generator in the
  build. A parity test gets the same guarantee with hand-written, readable code.
- **Clamp out-of-range values.** Never: OpenMHS refuses. A clamped value is one the
  agent did not choose.

## Consequences

- The model sees why a command was refused and what would be allowed, without being
  able to change what is allowed.
- `next_steps` is English prose and will need translation or a structured form if
  non-English agents become a target.
- The `rmcp` pin needs an upgrade pass. The next one should take 3.6.x once it is two
  weeks old.

[tools]: https://modelcontextprotocol.io/specification/2025-11-25/server/tools
[ref]: https://github.com/Abenor-Labs/Open-MHS
