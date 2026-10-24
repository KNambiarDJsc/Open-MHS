# Versions and compatibility of formats

OpenMHS versions several things separately. Everything is **0.1** and pre-alpha: until
1.0, any of them can change in a minor release, and the [changelog](../CHANGELOG.md) says
when.

| What | Version | Where it appears |
|---|---|---|
| Software (`openmhs`, `openmhsd`, Rust workspace) | 0.1.0 | `openmhs --version`; `Cargo.toml` |
| Python client | 0.1.0 | `openmhs.__version__` (kept equal to the workspace by `tools/check_release_versions.py`) |
| Specification (descriptors, commands, receipts) | 0.1 | `"openmhs": "0.1"` in a descriptor; `openmhs info` prints `spec 0.1` |
| Out-of-process adapter protocol | 0.1 | `protocol` in a [permission manifest](../spec/permission-manifest.md); negotiated at start ([spec/adapter-protocol.md](../spec/adapter-protocol.md)) |
| Permission manifest | 0.1 | `"openmhs_manifest": "0.1"` |
| Topology | 0.1 | `"openmhs_topology"` in a topology file |
| Conformance report | 0.1 | `"openmhs_conformance": "0.1"`; `suite_version` is the suite's own version |
| Adapter registry entry | 0.1 | `"openmhs_registry": "0.1"` |
| Minimum Rust | 1.89 | `rust-version` in `Cargo.toml` |

A gateway refuses a document whose version it does not support, with a stable error code,
rather than guessing. MCP's own protocol version is negotiated by the MCP SDK the server
uses ([ADR-0012](adr/0012-mcp-server-and-sdk.md)).

## How the documentation is versioned

This site is built from the repository, so every page is the version of the docs in the
commit it was built from. The plan once releases exist:

- **`latest`** is built from `main` and may describe unreleased work. It says so at the top.
- **A version** (for example `v0.1.0`) is built once from its release tag and then never
  changes, except for a correction to a broken link or a factual error, which is
  re-published from a docs-fix commit on a release branch.
- The site root redirects to the newest release; a version switcher links the others.
- Pages that describe what is *tested* (the [compatibility matrix](compatibility.md),
  [limitations](limitations.md)) always name the report or commit they were generated from.

Until the first release only `latest` exists. See
[docs/release/RELEASING.md](release/RELEASING.md).
