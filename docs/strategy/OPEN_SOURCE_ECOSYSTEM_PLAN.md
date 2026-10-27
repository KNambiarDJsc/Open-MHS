# Open-source ecosystem plan

How OpenMHS becomes something people outside the project install, extend and maintain.
Based on the repository at `37c56fb` and its public GitHub state on 2026-10-28: 3 stars,
1 fork, 0 issues, 2 merged pull requests (both by the maintainer), Discussions off, no
release, CI red on Linux and macOS.

The honest starting point: **the engineering is ahead of the ecosystem.** The project has a
specification, a conformance suite, a registry format, governance files and 360+ tests, and
no outside users yet. Everything below is about closing that gap without lowering the bar.

## 1. Developer experience audit

A new developer, on a fresh machine, against the nine steps a first success needs.

| # | Step | Today | Friction | Fix |
|---|---|---|---|---|
| 1 | Understand the project in 30 seconds | Good. README headline, three-problem "Why", a real terminal recording, explicit "not affiliated" and "no hardware tested" notes | The README is long (≈300 lines) before "Contributing" | Keep; move the comparison table and matrix below a fold |
| 2 | Install | **Poor.** `cargo install --locked --path crates/openmhs-cli`: Rust 1.89, a C compiler (rusqlite bundled), ≈5–15 minutes. No release binary, no PyPI package | The largest single drop-off | Cut v0.1.0 (owner actions in `docs/release/M10_TEST_REPORT.md`). `cargo-dist` and the PyPI workflow are already written |
| 3 | Start a simulator | Good. `openmhs demo`, `openmhs serve --sim`, `--profile-sim` for any profile | None | — |
| 4 | Connect an agent | Good. `docs/mcp.md`, `examples/agent-loop/mock_agent.py` needs no API key | Two terminals, a grant and a token before anything works | A `openmhs playground` command that does the grant and token for a sandbox principal ([PLAYGROUND_DESIGN.md](PLAYGROUND_DESIGN.md)) |
| 5 | Execute an authorized operation | Good. Quickstart shows refusal then success | PowerShell and Unix variants interleaved | Fine as is |
| 6 | Inspect evidence | Good. Receipts, `openmhs audit verify` | Receipts are JSON; no timeline view | `openmhs commands show <id> --timeline` |
| 7 | Modify an adapter | Good for Rust (`openmhs adapter new`, template conformance-tested in CI) and Python (SDK, psu-sim example) | Out-of-tree Rust adapters must depend on git `main` (crates.io blocked by `include_str!` of `spec/` in `openmhs-types`) | Move the schemas into the crate (`build.rs` copy or a `spec` crate) so it can publish |
| 8 | Run the tests | Mixed. `cargo test --workspace` takes ≈15 minutes cold on Windows (889 s in the M10 report). No guidance on running one crate's tests | Contributors will not wait | CONTRIBUTING: "test what you touched" table (`cargo test -p openmhs-scpi`) |
| 9 | Submit a contribution | Templates exist; CONTRIBUTING is clear | CI red on `main`, so every PR starts red | Fix H3 (done here); make macOS non-blocking until runners are reliable (they queued for 6 h on 2026-10-26) |

**Error messages** are a strength: refusals carry codes, rule ids and next steps
(`crates/openmhs-mcp/src/guidance.rs`), and the CLI explains startup failures. The one
confusing failure was H1 (one unreachable device stops the gateway), fixed here.

**Cross-platform:** Windows is the primary development platform; Linux runs in CI; macOS
arm64 is configured but its runners are unreliable; macOS x86_64 has never run.

## 2. Community adapter SDK

### What exists

| Piece | Where | State |
|---|---|---|
| Rust adapter trait | `crates/openmhs-adapter` (200 lines, stable shape) | Good, small, documented |
| Rust scaffold | `openmhs adapter new <name>` → `templates/adapter` | Built and conformance-tested in CI, so it cannot rot |
| Python adapter SDK | `python/openmhs/adapter.py`, stdlib only | Good; psu-sim reaches L3 through the adapter host |
| Out-of-process protocol | `spec/adapter-protocol.md`, JSON-RPC over stdio | Language-neutral; any language can implement it |
| Permission manifests, signatures | `spec/permission-manifest.md`, Ed25519 | Done; no OS enforcement |
| Profiles (no code) | `docs/adapters/profiles.md` for Modbus, MQTT, SCPI, HTTP | The cheapest path for most devices |
| Conformance | `openmhs conformance --profile / --adapter-manifest` | L0–L3, JSON reports |

The design already meets the brief: an adapter author implements six methods and never sees
the engine's orchestration. **Three paths, cheapest first:**

1. **A profile** (JSON) for a device that speaks Modbus TCP, MQTT, SCPI or HTTP. No code.
2. **A Python adapter** (one file plus a manifest) for anything with a Python library.
3. **A Rust adapter** for performance or a protocol that needs one.

### What is missing

| Gap | Why it matters | Fix |
|---|---|---|
| No published packages | A third party cannot `pip install openmhs` or depend on a crate version | Release v0.1.0; unblock crates.io (§1 step 7) |
| No out-of-tree example | Every adapter lives in this repository, so "without editing core" is untested in practice | A `openmhs-adapter-example` repository the maintainer does not own, built only against published packages |
| Capability contract versioning | Descriptors carry `spec_version`, but there is no rule for when a profile change is breaking | Hash the descriptor (already done for conformance) and record it in the registry entry; a changed hash invalidates the report |
| No device constraints beyond the descriptor | Firmware-specific quirks have nowhere to go | `notes` and `firmware` fields in the registry entry; profiles per firmware range |
| No sandbox | A community adapter runs with the user's full rights | Before community adapters are enabled by default (gap S2) |

## 3. Adapter registry

A Git-based registry **already exists** (`registry/`), and its design is right: one JSON
entry per adapter, pinned by SHA-256 to a committed conformance report, with categories
that describe evidence rather than endorsement, and a generated compatibility matrix that
never shows an adapter as better than its report (`tools/gen_matrix.py`, checked in CI).

Fields the brief asks for, against the schema (`registry/schema/adapter-entry.schema.json`):

| Field | Present | Add |
|---|---|---|
| Name, maintainer, protocol, package, version, licence | Yes | — |
| Device family and models | Only through `profiles` | `devices: [{manufacturer, model, firmware}]` |
| Firmware compatibility | No | in `devices` |
| Capability descriptions | Through the descriptor of each profile | — |
| Security requirements | Through the manifest (endpoints, files, processes) | Summarize in the entry |
| Test reports, simulator status | Yes (`report`, `peer`) | Allow several reports (one per platform) |
| Physical-device validation | `peer: device` exists, refused unless the report's sources are not a simulator | A `hardware_report` with the evidence bundle of [HARDWARE_VALIDATION_PLAN.md](HARDWARE_VALIDATION_PLAN.md) |
| Known limitations | `notes` | — |
| Maintenance status | No | `maintenance: active / looking-for-maintainer / unmaintained`, `last_verified` |

Keep it in Git. A hosted index is worth building only when there are more entries than a
README table can show, which is far off. When external entries arrive, a separate
`openmhs-registry` repository with its own reviewers lets adapter listings move faster than
the core.

**Registry inclusion is not certification.** The README of `registry/` already says so; the
compatibility matrix says it again. Keep both.

## 4. Governance

| File | State | Change |
|---|---|---|
| CONTRIBUTING.md | Clear, 82 lines, Apache-2.0 inbound with no separate CLA, explains tests and RFCs | Add the "test what you touched" table and a 15-minute warning for the full suite |
| CODE_OF_CONDUCT.md | Contributor Covenant reference | Owner must confirm the contact (release blocker 3) |
| SECURITY.md | Points to private vulnerability reporting | **Owner must enable it** on GitHub (blocker 3) |
| GOVERNANCE.md | Honest: one maintainer, how decisions are made, fixed principles | Fine. Add a path from contributor to reviewer for adapters |
| Maintainer roles | One person | The bus factor is 1. Recruit a second maintainer for adapters first; it is the lowest-risk area to share |
| RFC process | `docs/rfcs/0000-template.md` | Good. No RFC has been filed yet; file the first one (H4, readback freshness) to show the process works |
| Release and compatibility policy | `docs/versions.md`, `RELEASING.md` | Good |
| Issue and PR templates | Bug, feature, adapter listing | Good |
| Good first issues | `docs/good-first-issues.md`, ten ideas, **not filed** | File them when the repository is announced; ten real issues are worth more than a list |
| Discussions | Off | Turn on with three categories: Q&A, Show your device, Ideas |

Nothing here is over-complicated. The gap is not documents; it is people.

## 5. Contributor roadmap

| Stage | Who | What they can do | What the project provides |
|---|---|---|---|
| First visit | Student, hobbyist | Run `openmhs demo`; run the playground; file a bug | A binary release; a playground |
| First contribution | Anyone | Docs fix, error message, a fourth example descriptor, a profile against a simulator | Filed good-first issues, "test what you touched" |
| Adapter author | Device owner | A profile or Python adapter, a conformance report, a registry entry | Out-of-tree example, published SDKs |
| Hardware validator | Lab member, instrument owner | Run the validation protocol on a real device and submit evidence | [HARDWARE_VALIDATION_PLAN.md](HARDWARE_VALIDATION_PLAN.md), a report template |
| Adapter reviewer | Repeat adapter author | Review other adapter PRs | CODEOWNERS for `adapters/`, `registry/` |
| Maintainer | Sustained, careful contributor | Merge rights for an area | GOVERNANCE.md update |

## 6. First 90 days, in order

1. Green CI on all three platforms (H3 fixed here; make macOS non-blocking until stable).
2. Owner actions, then v0.1.0: binaries, PyPI, docs site (all already scripted).
3. File the ten good-first issues; open Discussions.
4. Unblock crates.io; publish `openmhs-types`, `openmhs-adapter`, `openmhs-conformance`.
5. One out-of-tree adapter built only from published packages, by someone else if possible.
6. Registry schema: `devices`, `maintenance`, multiple reports.
7. The first hardware validation report ([HARDWARE_VALIDATION_PLAN.md](HARDWARE_VALIDATION_PLAN.md)).
