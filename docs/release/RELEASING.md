# Releasing OpenMHS

How a maintainer cuts a release. Nothing here runs by itself: every public step is a
deliberate action by the maintainer.

**State today:** no binaries, packages, crates or documentation site have been published.
The pipelines below are configured and have been dry-run locally where a laptop can
(see [what was and was not verified](#what-was-and-was-not-verified)).

## What a release consists of

| Artifact | Built by | Published to | Trigger |
|---|---|---|---|
| `openmhs` and `openmhsd` archives for Windows x86_64, Linux x86_64, macOS arm64 and x86_64, with SHA-256 files and signed provenance | [`release.yml`](../../.github/workflows/release.yml) ([cargo-dist](https://opensource.axo.dev/cargo-dist/) 0.32.0) | A **draft** GitHub Release | Pushing a version tag |
| `openmhs` Python package (sdist and wheel) | [`pypi.yml`](../../.github/workflows/pypi.yml) | PyPI, by Trusted Publishing | Manual run from a version tag, with "publish" ticked, then approval of the `pypi` environment |
| Documentation site | [`docs.yml`](../../.github/workflows/docs.yml) (mdBook) | GitHub Pages | Manual run with "deploy" ticked |
| Rust crates | not released | | See [crates.io](#cratesio-is-blocked) |

## Before tagging

1. **Everything is green** on `main`: CI (Windows, Linux, macOS), the docs build, the
   release-tooling job. Look at macOS in particular; it has been slow to start.
2. **Run the gate locally** (the full list is in the M10 test report):
   ```sh
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace --no-fail-fast
   cargo deny check
   python tools/check_schemas.py
   python tools/check_registry.py && python tools/gen_matrix.py --check
   python -m unittest discover -s tools -p "test_*.py"
   python tools/build_docs.py --check-links
   dist plan
   ```
3. **Regenerate conformance reports** on the release commit if the runtime changed:
   `openmhs conformance --report-dir <dir>`, add them as **new** files under
   `conformance/reports/<device>/` (never overwrite), update `registry/adapters/*.json`
   (hash and date), then `python tools/gen_matrix.py`.
4. **Decide the version.** It is in two places: `[workspace.package] version` in
   `Cargo.toml` and `__version__` in `python/openmhs/__init__.py`. Change both, run
   `cargo build` to refresh `Cargo.lock`, copy `LICENSE` to `python/LICENSE` if it changed.
5. **Cut the changelog.** In `CHANGELOG.md`, rename `[Unreleased]` to `[X.Y.Z] - YYYY-MM-DD`
   and start a fresh `[Unreleased]`.
6. `python tools/check_release_versions.py --tag vX.Y.Z` must pass. The release and PyPI
   workflows run it again.
7. Re-read the [README](../../README.md), [limitations](../limitations.md) and the
   [compatibility matrix](../compatibility.md). Do they still say only what the evidence
   supports? In particular: **no physical hardware has been tested**; if that changed, the
   matrix is the place that shows it, from a report.

## Cutting the release

```sh
git tag -a vX.Y.Z -m "OpenMHS X.Y.Z"
git push origin vX.Y.Z
```

The tag starts `release.yml`: it plans, builds the four targets, attests the archives, and
creates a **draft** GitHub Release with the archives, their `.sha256` files and a
`sha256.sum`. Nothing is public yet.

If a job fails or an archive is wrong, delete the draft release and the tag
(`gh release delete vX.Y.Z --yes`, `git push --delete origin vX.Y.Z`,
`git tag -d vX.Y.Z`), fix, and tag again. This is the rehearsal; the draft protects you.

Then, with the draft open:

1. Download the Linux and macOS archives, and verify them: `sha256sum -c`, then
   `gh attestation verify <archive> --repo KNambiarDJsc/Open-MHS`.
2. Unpack each on its platform and run the [quickstart](../../README.md#quickstart) with that
   binary: `openmhs --version`, `openmhs demo`, `openmhs serve --sim`. **The Linux and macOS
   binaries have never been run by the project before this step**; the Windows archive was
   built and run locally.
3. Edit the generated release notes: lead with what is real, and repeat the pre-alpha and
   no-hardware caveats.
4. **Publish the draft** (the "Publish release" button). This is the public step.

## PyPI

One-time setup, on pypi.org with the maintainer's account: *Publishing → Add a pending
publisher* for project name `openmhs`, owner `KNambiarDJsc`, repository `Open-MHS`,
workflow `pypi.yml`, environment `pypi`. In the repository, create an environment named
`pypi` and require a reviewer for it (Settings → Environments). There is no API token
anywhere. (The name `openmhs` was unregistered on PyPI when checked on 2026-10-25; that can
change.)

To publish: Actions → *Python package* → *Run workflow* → choose the tag `vX.Y.Z` → tick
*publish* → run → approve the `pypi` environment. A tag push alone only builds and checks
the package. After publishing, install it in a clean environment and run
`python -I tools/smoke_python_package.py --bin <openmhs>`.

PyPI uploads cannot be replaced. If one is wrong, yank it and release the next patch version.

## Documentation site

Settings → Pages → Source: *GitHub Actions*. Then Actions → *Docs* → *Run workflow* with
*deploy* ticked. The site is built from the ref you choose; see
[Versions](../versions.md#how-the-documentation-is-versioned) for the plan for keeping
versions.

## crates.io is blocked

Six crates are marked publishable ([ADR-0014](../adr/0014-protocol-adapters.md)), but
`openmhs-types` embeds files from `spec/` with `include_str!`, and a published crate
cannot reach outside its own directory. Publishing needs the schemas to move into the
crate (or be copied at build time) and the other five to be re-packaged against it. This is
not done in M10; no crate is published.

## Recommended repository settings

Not changed by anyone but the owner; see [GITHUB_PRESENTATION.md](GITHUB_PRESENTATION.md).

## What was and was not verified

Recorded in the M10 test report. In short, on Windows 11 x86_64: the dist archive was
built, its checksum verified, extracted to a fresh directory, and `openmhs --version`,
`openmhs demo`, the PowerShell quickstart, the MCP example, the Python SDK tests and the
Python-package smoke test were run with that binary. The Linux and macOS builds, the
attestation, the PyPI publish, and the Pages deploy cannot be exercised without pushing
and are untested.
