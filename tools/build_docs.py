"""Build the documentation site (mdBook) from the repository's own docs.

There are no copies of the documentation to maintain. This script *stages* the pages listed
in docs/book/SUMMARY.md into docs/book/src/ (ignored by git) with the repository's
directory layout, so relative links between them keep working, then runs `mdbook build`.

While staging it also checks and fixes links in the pages:

- a link to another page in the book stays as it is;
- a link to an image is staged beside it;
- a link to anything else in the repository (source code, JSON, a page that is not part of
  the book) becomes a link to that file on GitHub;
- a link to something that does not exist is an error, and the build fails.

Usage: python tools/build_docs.py [--no-build] [--check-links]
       (needs mdBook; --check-links also needs lychee)
"""
import argparse
import pathlib
import posixpath
import re
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
BOOK = ROOT / "docs" / "book"
SRC = BOOK / "src"
REPO_URL = "https://github.com/KNambiarDJsc/Open-MHS"
IMAGES = {".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp"}

MD_LINK = re.compile(r"(\]\()([^)\s]+)((?:\s+\"[^\"]*\")?\))")
HTML_ATTR = re.compile(r"""((?:src|srcset|href)=")([^"]+)(")""")
CODE_SPAN = re.compile(r"(`+[^`]*`+)")
SUMMARY_LINK = re.compile(r"\]\(([^)\s#]+\.md)\)")


def summary_pages():
    text = (BOOK / "SUMMARY.md").read_text(encoding="utf-8")
    return [m.group(1) for m in SUMMARY_LINK.finditer(text)]


class Stager:
    def __init__(self, pages):
        self.pages = set(pages)
        self.errors = []
        self.rewritten = 0

    def fix(self, page, target):
        """The target to use in `page`, or None to leave it alone."""
        if re.match(r"^([a-z][a-z0-9+.-]*:|//|#)", target, re.I):
            return None
        path, hash_, frag = target.partition("#")
        if not path:
            return None
        resolved = posixpath.normpath(posixpath.join(posixpath.dirname(page), path))
        if resolved.startswith(".."):
            self.errors.append(f"{page}: link '{target}' leaves the repository")
            return None
        full = ROOT / resolved
        if not full.exists():
            self.errors.append(f"{page}: link '{target}' points at '{resolved}', which does not exist")
            return None
        if resolved in self.pages:
            # mdBook writes README.md as index.html but leaves links to README.html.
            if posixpath.basename(resolved) == "README.md":
                return path[: -len("README.md")] + "index.html" + hash_ + frag
            return None
        if full.is_file() and full.suffix.lower() in IMAGES:
            dest = SRC / resolved
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(full, dest)
            return None
        kind = "tree" if full.is_dir() else "blob"
        self.rewritten += 1
        return f"{REPO_URL}/{kind}/main/{resolved.rstrip('/')}{hash_}{frag}"

    def transform_line(self, page, line):
        def md(m):
            new = self.fix(page, m.group(2))
            return m.group(0) if new is None else f"{m.group(1)}{new}{m.group(3)}"

        def attr(m):
            new = self.fix(page, m.group(2))
            return m.group(0) if new is None else f"{m.group(1)}{new}{m.group(3)}"

        parts = CODE_SPAN.split(line)
        for i in range(0, len(parts), 2):  # odd parts are code spans
            parts[i] = HTML_ATTR.sub(attr, MD_LINK.sub(md, parts[i]))
        return "".join(parts)

    def stage(self, page):
        out = []
        fence = None
        for line in (ROOT / page).read_text(encoding="utf-8").splitlines(keepends=True):
            stripped = line.lstrip()
            marker = stripped[:3]
            if fence is None and marker in ("```", "~~~"):
                fence = marker
            elif fence is not None and stripped.startswith(fence):
                fence = None
            out.append(line if fence is not None or marker in ("```", "~~~") else self.transform_line(page, line))
        dest = SRC / page
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text("".join(out), encoding="utf-8", newline="\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--no-build", action="store_true", help="only stage the pages")
    ap.add_argument("--check-links", action="store_true", help="run lychee on the built site")
    args = ap.parse_args()

    pages = summary_pages()
    missing = [p for p in pages if not (ROOT / p).is_file()]
    if missing:
        sys.exit("SUMMARY.md lists pages that do not exist:\n  " + "\n  ".join(missing))

    if SRC.exists():
        shutil.rmtree(SRC)
    SRC.mkdir(parents=True)
    shutil.copyfile(BOOK / "SUMMARY.md", SRC / "SUMMARY.md")
    stager = Stager(pages)
    for page in pages:
        stager.stage(page)
    if stager.errors:
        sys.exit("broken links in the documentation sources:\n  " + "\n  ".join(sorted(set(stager.errors))))
    print(f"staged {len(pages)} pages; {stager.rewritten} links to files outside the book now point at GitHub")
    if args.no_build:
        return

    if not shutil.which("mdbook"):
        sys.exit("mdbook is not installed (cargo install --locked mdbook@0.5.4)")
    subprocess.run(["mdbook", "build", str(BOOK)], check=True)
    print(f"built {BOOK / 'build'}")

    if args.check_links:
        if not shutil.which("lychee"):
            sys.exit("lychee is not installed (cargo install --locked lychee@0.24.2)")
        # Local links and anchors only: CI must not depend on other people's websites.
        subprocess.run(
            ["lychee", "--offline", "--include-fragments", "--no-progress", "--root-dir", str(BOOK / "build"),
             "--exclude-path", "404.html", str(BOOK / "build") + "/**/*.html"],
            check=True,
        )


if __name__ == "__main__":
    main()
