# Contributing to OpenMHS

Thanks for looking. OpenMHS is pre-alpha, maintained by one person, and moves in
milestones ([docs/roadmap.md](docs/roadmap.md)). Small, focused changes are the easiest to
review.

> **Want to connect a new device? Build an adapter.** That is the most useful
> contribution. Start with [docs/adapters/README.md](docs/adapters/README.md), run
> `openmhs adapter new <name>`, and make the [conformance suite](spec/conformance.md)
> pass. Then list it in the [adapter registry](registry/README.md).

## Ground rules

- **Safety claims need evidence.** Do not write that something is safe, certified or
  validated unless a test or a report in the repository shows it. Simulated is not
  physical; say which one you tested.
- **No real hardware in CI or tests.** Tests run against simulators and in-process
  fixtures. Anything that could move or power a real device must be opt-in and say so
  loudly.
- **Do not weaken security tests** to make a change pass. If a test is wrong, say why in
  the pull request.
- **Be honest about scope.** OpenMHS is independent and unaffiliated with Anthropic. Do
  not claim compatibility with Anthropic's Model Hardware Standard; its specification is
  not public ([ADR-0007](docs/adr/0007-mhs-independence.md)).

## Setting up

You need Rust 1.89 or newer. Python 3.10+ is needed for the Python SDK, the schema
check and some examples.

```sh
git clone https://github.com/KNambiarDJsc/Open-MHS
cd Open-MHS
cargo build -p openmhs-cli
cargo run -p openmhs-cli --bin openmhs -- demo
```

## Before you open a pull request

Run what CI runs:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check                                # https://github.com/EmbarkStudios/cargo-deny
python tools/check_schemas.py                   # pip install jsonschema
(cd python/tests && PYTHONPATH=.. python -m unittest discover -s . -v)
python tools/check_registry.py                  # adapter registry
python tools/gen_matrix.py --check              # compatibility matrix is current
```

While you work, test only what you touched. The whole workspace takes about ten minutes to
build from cold and needs a few GB of disk (`target/`; the workspace's dev and test
profiles are set to keep this small, see `Cargo.toml`):

| You changed | Run |
|---|---|
| The engine, lifecycle, availability | `cargo test -p openmhs-engine` |
| An API method, the gateway | `cargo test -p openmhs-gateway -p openmhs-api` |
| A protocol adapter | `cargo test -p openmhs-scpi` (or `-modbus`, `-mqtt`, `-http`, `-profile`) |
| The MCP server | `cargo test -p openmhs-mcp` |
| The CLI | `cargo test -p openmhs-cli` |
| A schema or a spec example | `python tools/check_schemas.py` and `cargo test -p openmhs-types` |
| The Python SDK | the `python/tests` command above, after `cargo build -p openmhs-cli` |

Run the full set before you open the pull request. If your disk is small, `cargo clean`
frees everything under `target/` and nothing else.

Also:

- Add or update tests for behaviour you change. Prefer a test that fails without your
  change.
- Update the documentation next to the code (`docs/`, `spec/`). If you change a schema
  or a spec, change the examples in `spec/examples/` too.
- New dependencies need a reason, a licence that `deny.toml` allows, and should be at
  least two weeks old. Prefer not adding one.
- Do not hand-write cryptography.

## Larger changes

For changes to the specification, the adapter contract, the API, or the safety model,
open an RFC first: copy [docs/rfcs/0000-template.md](docs/rfcs/0000-template.md) and
open a pull request with it. Decisions that stick become an ADR in
[docs/adr/](docs/adr/README.md).

## Commits and pull requests

- One logical change per pull request. Explain what changed and why; link the issue.
- Commit messages: a short imperative subject, then a body when the reason is not obvious.
- By submitting a contribution you agree to license it under Apache-2.0 (see
  [LICENSE](LICENSE)); there is no separate CLA.
- Security problems: do not open a public issue. See [SECURITY.md](SECURITY.md).

## Good first issues

See [docs/good-first-issues.md](docs/good-first-issues.md).

By taking part you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).
