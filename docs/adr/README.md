# Architecture Decision Records

| ADR | Title | Status |
|---|---|---|
| [0001](0001-single-trusted-gateway.md) | Single trusted gateway; all interfaces are clients | Accepted |
| [0002](0002-command-lifecycle.md) | Canonical command lifecycle with explicit uncertainty | Accepted |
| [0003](0003-authorization-engine.md) | Small typed authorization engine; Cedar re-evaluated in M8 | Accepted |
| [0004](0004-units.md) | UCUM units; exact match or explicit conversion | Accepted |
| [0005](0005-local-api-transport.md) | JSON-RPC 2.0 over loopback HTTP with token | Accepted |
| [0006](0006-mcp-sdk.md) | MCP via `rmcp`, as a gateway client | Accepted |
| [0007](0007-mhs-independence.md) | Independent of Anthropic MHS; façade later | Accepted |
| [0008](0008-languages.md) | Rust core, Python clients and adapters | Accepted |

Format: Context → Decision → Alternatives considered → Consequences. Supersede an
ADR with a new one rather than rewriting history.
