# OpenMHS website

A plain static site: HTML, one stylesheet (`style.css`) and SVG illustrations in `img/`.
No build step, no JavaScript, no external requests.

| Page | Contents |
|---|---|
| `index.html` | What OpenMHS is in one screen, the demo excerpt, status |
| `about.html` | Background: agents, tools, MCP, MHS; the problem; what it does and is not |
| `how-it-works.html` | Architecture, command lifecycle, unknown outcomes, policy, descriptors, adapters, conformance, plans |
| `docs.html` | Getting started: build, demo, gateway, MCP, Python SDK, profiles, adapters, tests |
| `reports.html` | Status, demo and conformance results, limitations, roadmap |
| `guidelines.html` | Contributing rules, checks, AI assistants, hardware safety |

## Preview

```sh
python -m http.server 8000 --directory website
```

Then open http://localhost:8000. Opening `index.html` directly from disk also works.

## Keeping it accurate

The content is taken from the repository (`README.md`, `CONTRIBUTING.md`, `GOVERNANCE.md`,
`SECURITY.md`, `spec/`, `docs/`, `docs/compatibility.md`, `docs/release/M10_TEST_REPORT.md`,
`docs/experiments/`) and real runs of `openmhs demo`, `openmhs conformance` and
`examples/agent-loop/mock_agent.py`. `img/logo.png` and `img/terminal-demo.svg` are copies of
`docs/assets/openmhs-logo-light.png` and `docs/assets/demo.svg`. When those
change, update `reports.html` and the status tables to match. Results on the reports page
are dated; re-run and re-date them rather than editing numbers by hand.

## Publishing

Deployed on Vercel as a static site, from this folder (no build step). `vercel.json`
sets security headers: no scripts, no frames, nothing loaded from other origins.

```sh
cd website
vercel deploy            # a preview URL
vercel deploy --prod     # the production URL
```

Any other static host works too (GitHub Pages, Netlify, a plain web server).
