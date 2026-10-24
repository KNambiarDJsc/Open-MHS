# Good first issues

Candidates for people new to the project. They are not filed as GitHub issues yet; the
maintainer will file them with the `good first issue` label when the repository opens.
Each is small, testable without hardware, and does not touch the safety model. Ask on
the issue before you start so work is not duplicated.

| Idea | Where | Notes |
|---|---|---|
| Run the quickstart on macOS (arm64 or x86_64) and report what differs | [README](../README.md) | macOS x86_64 has never been tested; arm64 only in CI |
| Run the quickstart on a Linux distribution other than Ubuntu | README | Report glibc and package problems with the release archive |
| Add an example descriptor for a fourth kind of device (for instance a pump or a relay board) with valid and invalid fixtures | `spec/examples/` | `python tools/check_schemas.py` must pass |
| Write a device profile for a common instrument against an existing protocol simulator | `adapters/*/profiles/` | Must pass `openmhs conformance --profile`; label it simulated, not validated |
| Improve an error message that does not say what to do next | `crates/openmhs-cli`, `crates/openmhs-mcp` | Add a test that asserts the text |
| Tighten a doc page: remove stale statements, add a missing command example, fix a broken link | `docs/` | `lychee` runs in CI |
| Add a shell completion command to the CLI | `crates/openmhs-cli` | `clap_complete` is the usual route; check `cargo deny` first |
| Try serial SCPI on Windows with a USB-serial loopback and report | [docs/adapters/scpi.md](adapters/scpi.md) | Currently untested on Windows; a report is a contribution |
| Write a second example agent loop in a language other than Python, using the MCP server | `examples/` | Must run with no API key |
| Port the adapter-author tutorial to your adapter language and open a docs PR | [docs/adapters/README.md](adapters/README.md) | The Python adapter SDK already exists |

Larger projects need an [RFC](rfcs/0000-template.md) first. See
[CONTRIBUTING.md](../CONTRIBUTING.md).
