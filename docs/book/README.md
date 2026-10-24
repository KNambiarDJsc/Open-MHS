# Documentation site

The site is an [mdBook](https://rust-lang.github.io/mdBook/) built from the pages that
already live in `docs/`, `spec/`, `examples/` and the repository root. There are no copies
to keep in sync: [`tools/build_docs.py`](../../tools/build_docs.py) stages the pages listed
in [`SUMMARY.md`](SUMMARY.md) into `docs/book/src/` (git-ignored) and runs `mdbook build`.

```sh
cargo install --locked mdbook@0.5.4          # once; and lychee@0.24.2 for link checking
python tools/build_docs.py                   # stages and builds into docs/book/build/
python tools/build_docs.py --check-links     # also checks every link in the built site
mdbook serve docs/book                       # preview with live reload, after staging
```

What the build does to the pages:

- links between pages in the book stay as written;
- links to source code, JSON schemas and pages that are not in the book become links to
  the file on GitHub;
- a link to something that does not exist fails the build;
- every page has an "edit this page" link to the real file.

The site has full-text search, copy buttons on code blocks, a navigation tree that follows
`SUMMARY.md`, and a layout that works on narrow screens.

The Mermaid diagram in the README appears as its source text here: rendering it would
need a third-party script, and this site loads none.

To add a page: write it where it belongs in the repository, then add it to `SUMMARY.md`.
Publishing the site (GitHub Pages) and how versions are kept are described in
[docs/release/RELEASING.md](../release/RELEASING.md) and [docs/versions.md](../versions.md).
