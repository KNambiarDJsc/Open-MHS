# ADR-0006: MCP via the official Rust SDK (`rmcp`), as a gateway client

- Status: Accepted; version pinned in [ADR-0012](0012-mcp-server-and-sdk.md)
- Date: 2026-10-23

## Context

The MCP server is the main agent-facing interface. It must not hold adapter access
or hardware credentials (ADR-0001).

## Decision

- Use `rmcp`, the official SDK from `modelcontextprotocol/rust-sdk`
  (crates.io: 3.6.0 stable, updated 2026-10-20; ~39M downloads). Pin the exact
  version when M4 starts and re-check the changelog; the project releases often and
  has shipped breaking majors.
- The MCP server is a separate binary/subcommand (`openmhs mcp`) that talks to the
  gateway's local API. It exposes a small, stable tool set:
  `list_devices`, `describe_device`, `read_state`, `submit_command`, `get_command`,
  `list_pending_approvals`. Denials are returned as structured results (rule IDs +
  reasons) so agents can replan, not as transport errors.
- Per-capability typed tools may be generated later as an option; the generic set
  stays the default to avoid tool explosion on large device fleets.

## Alternatives considered

- **Python MCP SDK.** Mature, but adds a Python runtime to the core install path.
- **Hand-rolled MCP.** Unnecessary risk of protocol drift.

## Consequences

- Rapid upstream releases mean periodic upgrade work; contained to one crate.
