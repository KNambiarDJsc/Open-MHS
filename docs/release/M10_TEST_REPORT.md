# M10 test report

Release engineering, documentation and open-source launch preparation, on branch `m10`.
Nothing has been published: no release, no PyPI upload, no documentation site, no push.

**Status: M10 is built and locally verified on Windows; it is not releasable until the
blockers below are cleared.** The files existing is not the claim. What was run is listed
under each heading, with what was *not* run.

Environment: Windows 11 x86_64, rustc 1.99.1 (edition 2024), Python 3.11, cargo-dist 0.32.0,
mdBook 0.5.4, lychee 0.24.2. Linux and macOS were not available.

## Deliverables

| # | Deliverable | Where | Verified by |
|---|---|---|---|
| 1 | README rewrite | [README.md](../../README.md) | Quickstart run from a release binary in PowerShell; every matrix row generated from a report |
| 2 | README before/after | [README_REWRITE.md](README_REWRITE.md) | n/a (explains the above) |
| 3 | Release artifacts (dry run) | [dist-workspace.toml](../../dist-workspace.toml), [release.yml](../../.github/workflows/release.yml) | `dist plan` in CI-equivalent; `dist build --target x86_64-pc-windows-msvc` built the zip, checksum matched, `openmhs 0.1.0`, `openmhs demo`, PowerShell quickstart, MCP example, 41 Python tests and the package smoke test ran with the extracted binary. **Linux and macOS builds never ran** |
| 4 | Python package | [python/](../../python), [pypi.yml](../../.github/workflows/pypi.yml) | sdist and wheel built, `twine check` passed, installed in a clean venv, smoke test passed. **No upload** |
| 5 | Documentation build | [docs/book/](../book), [tools/build_docs.py](../../tools/build_docs.py) | mdBook build, 78 pages, lychee offline link check: 2186 links, 0 errors |
| 6 | Registry schema and entries | [registry/](../../registry) | `check_registry.py`, 23 tests |
| 7 | Compatibility matrix | [docs/compatibility.md](../compatibility.md) | Generated from committed conformance reports; `gen_matrix.py --check` in CI |
| 8 | Governance and community files | CONTRIBUTING, SECURITY, SUPPORT, CODE_OF_CONDUCT, GOVERNANCE, templates | Read; links checked by the docs build |
| 9 | Demo and walkthrough scripts | [demo/](../../demo), [docs/assets/](../assets) | Terminal SVG and transcript from a real run. **The VHS video was not rendered** (VHS not installed) |
| 10 | Installation instructions | [docs/installation.md](../installation.md) | Windows steps run; Linux/macOS steps are written, untested |
| 11 | This test report | | |
| 12 | Release blockers | below | |
| 13 | Known limitations | below, [docs/limitations.md](../limitations.md) | |
| 14 | Exact publish steps | below, [RELEASING.md](RELEASING.md) | Not executed |

Also added after the brief, at the maintainer's request: [Experiment 01](../experiments/01-octoprint/README.md),
a read-only OctoPrint adapter tested against OctoPrint's Virtual Printer
([results](../experiments/01-octoprint/REPORT.md)). Simulated peer only.

## Gate results

Full gate run on this branch at commit 257ef6c (before Experiment 01 and this report), Windows:

| Step | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo test --workspace --no-fail-fast` | pass (889 s) |
| `cargo deny check` | pass |
| `check_schemas.py`, `check_registry.py`, `gen_matrix.py --check`, `check_release_versions.py` | pass |
| `python -m unittest discover -s tools` | pass (23 tests) |
| `cargo build`, Python SDK tests, `dist plan`, `openmhs demo`, conformance of the simulators | pass |
| `tools/build_docs.py --check-links` | failed only because this report did not yet exist |

After Experiment 01 (no Rust, schema or spec file changed; `git status` for `crates/`, `spec/`,
`Cargo.toml`, `Cargo.lock` is empty):

| Step | Result |
|---|---|
| `cargo test --workspace --no-fail-fast` (rerun) | pass: 354 tests passed, 0 failed, 0 ignored, 67 test binaries, exit 0 |

| Step | Result |
|---|---|
| `check_schemas.py` (78 documents), `check_registry.py` (11 entries, 9 reports), `gen_matrix.py --check`, `check_release_versions.py` | pass |
| `python -m unittest discover -s tools` | pass (23 tests) |
| Python SDK tests (`python/tests`) | pass |
| OctoPrint adapter tests, real OctoPrint and gateway | 42 run, 41 passed, 1 skipped (abstract base), 0 failed |
| Conformance of the OctoPrint adapter | L1 reached, declared L1, 34 pass, 0 fail, 0 untested |

Documentation build (`python tools/build_docs.py --check-links`, mdBook + lychee offline) after the
last change: 78 pages staged, 2186 links, 0 errors. The earlier "75 pages" figure was before the
OctoPrint and report pages.

## Release blockers

Things that must be true before announcing, in the order to clear them. None is a code defect
known today; all are things not yet done or not possible from this machine.

1. **Nothing has run on Linux or macOS.** The release workflow builds four targets; only Windows
   was built. The tarball layout (the Windows zip is flat; the install page uses `find`) and
   the Linux/macOS binaries are unverified until a tag is pushed to a **draft** release.
2. **CI has not run on this branch.** `ci.yml` gained two jobs (`release-tooling`, `octoprint`)
   that have never executed. The `octoprint` job installs pinned packages on Linux that were
   only resolved on Windows.
3. **Owner actions:** enable private vulnerability reporting (SECURITY.md depends on it);
   confirm the code-of-conduct contact; create the `pypi` environment and the PyPI trusted
   publisher; enable Pages; decide the repository settings in
   [GITHUB_PRESENTATION.md](GITHUB_PRESENTATION.md). None was changed.
4. **The demo video is not rendered.** Run the *Demo recording* workflow and review it.
5. **`openmhs` on PyPI**: the name was free on 2026-10-25; that can change.
6. **crates.io is blocked** and no crate is published: `openmhs-types` embeds `spec/` files with
   `include_str!`, which a published crate cannot reach.

## Known limitations

* **No physical hardware has been tested.** Every conformance report has `sources:
  ["simulator"]`; the matrix has no physically validated row. The OctoPrint experiment used
  OctoPrint's Virtual Printer.
* OPC UA and ROS 2 have no committed reports; the matrix says "Not verified" for them.
* The OctoPrint adapter is read-only and was run against one OctoPrint version on one platform.
* The compatibility matrix reflects Windows x86_64 runs of 2026-10-24.
* Mermaid diagrams appear as source in the built site.
* Dependency choices follow the two-week rule; the pinned OctoPrint set was resolved by hand
  for eight packages because the installed `pip` has no `--uploaded-prior-to`.
* Open from earlier milestones and not part of M10: the M8 review, a ROS 2 rerun, the
  `rumqttc` advisory ignores, the `openmhs-types` packaging blocker.

## Exact publish steps

Every step is the maintainer's decision; none has been taken. Full detail:
[RELEASING.md](RELEASING.md).

1. Review and merge `m10` to `main` (the maintainer's call; the standing rule for this
   repository is fast-forward only, no force).
2. Confirm CI is green on `main`, on Windows, Linux and macOS, including the two new jobs.
3. Do the owner actions in blocker 3.
4. Cut the changelog (`[Unreleased]` to `[0.1.0] - <date>`), run
   `python tools/check_release_versions.py --tag v0.1.0`.
5. `git tag -a v0.1.0 -m "OpenMHS 0.1.0" && git push origin v0.1.0`: builds a **draft** release.
6. Download and run the Linux and macOS archives; `gh attestation verify`; edit the notes.
7. Publish the draft release (the public step).
8. PyPI: Actions, *Python package*, run from the tag with *publish* ticked, approve `pypi`.
9. Docs: Actions, *Docs*, run with *deploy* ticked.
10. Then post: only after steps 6 to 9 have been checked from a clean machine.
