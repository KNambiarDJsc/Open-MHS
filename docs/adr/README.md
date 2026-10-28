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
| [0009](0009-descriptor-model.md) | Descriptor model choices for spec v0.1 | Accepted |
| [0010](0010-engine-execution-model.md) | Engine execution model (per-device serialization, fresh reads, isolation) | Accepted |
| [0011](0011-gateway-api-and-roles.md) | Gateway API, roles, tokens and home directory (refines 0005) | Accepted |
| [0012](0012-mcp-server-and-sdk.md) | MCP server design, agent guidance, unattested text, Python SDK (refines 0006) | Accepted |
| [0013](0013-conformance-suite.md) | Conformance suite: fixtures, fault injection, evidence reports; adapter template | Accepted |
| [0014](0014-protocol-adapters.md) | Protocol adapters driven by device profiles; MQTT acknowledgements; dependency policy | Accepted |
| [0015](0015-adapter-host.md) | Out-of-process adapters: JSON-RPC over stdio, supervision, permission manifests | Accepted |
| [0016](0016-transport-security.md) | Transport security: verified TLS, no insecure mode, plaintext refused beyond loopback | Accepted |
| [0017](0017-identity-and-trust.md) | Identity and trust: scoped credentials, approval records, signed adapter manifests, the `Caller` context | Accepted |
| [0018](0018-orchestration.md) | Capability graph, leases, stop lane, plans with twin preflight | Accepted |
| [0019](0019-device-availability.md) | Device availability: offline devices at start, probes, `device.unavailable`, `devices.status` | Accepted |
| [0020](0020-readback-must-follow-dispatch.md) | Readback must be measured after the command was sent | Accepted |
| [0021](0021-journal-quota-and-reserve.md) | Journal size limit and critical reserve | Accepted |
| [0022](0022-insights-from-the-journal.md) | Insights: an agent learns from what the journal recorded, never from a back door | Accepted |

Format: Context → Decision → Alternatives considered → Consequences. Supersede an
ADR with a new one rather than rewriting history.
