"""Check that everything released together carries the same version.

- the Rust workspace version (Cargo.toml [workspace.package]),
- the Python package version (python/openmhs/__init__.py),
- the CHANGELOG has a section for it, or for [Unreleased] while it is not yet cut,
- python/LICENSE is the repository LICENSE.

With --tag vX.Y.Z (or X.Y.Z), the tag must equal that version. The release and PyPI
workflows run this before building anything.

Usage: python tools/check_release_versions.py [--tag v0.1.0]
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent


def main(argv):
    problems = []
    cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    m = re.search(r'\[workspace\.package\][^\[]*?^version\s*=\s*"([^"]+)"', cargo, re.S | re.M)
    rust = m.group(1) if m else None
    init = (ROOT / "python" / "openmhs" / "__init__.py").read_text(encoding="utf-8")
    m = re.search(r'^__version__\s*=\s*"([^"]+)"', init, re.M)
    py = m.group(1) if m else None
    if not rust:
        problems.append("no [workspace.package] version in Cargo.toml")
    if not py:
        problems.append("no __version__ in python/openmhs/__init__.py")
    if rust and py and rust != py:
        problems.append(f"Rust workspace is {rust} but the Python package is {py}")
    if (ROOT / "LICENSE").read_bytes() != (ROOT / "python" / "LICENSE").read_bytes():
        problems.append("python/LICENSE differs from LICENSE; copy it again")
    if "--tag" in argv:
        tag = argv[argv.index("--tag") + 1].removeprefix("v")
        if tag != rust:
            problems.append(f"tag {tag} does not match the workspace version {rust}")
        changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
        if f"## [{tag}]" not in changelog:
            problems.append(f"CHANGELOG.md has no '## [{tag}]' section; cut it before tagging")
    if problems:
        print("\n".join(problems))
        return 1
    print(f"ok: version {rust}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
