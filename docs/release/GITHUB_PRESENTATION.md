# GitHub presentation and recommended settings

Proposals for the repository owner. **Nothing here has been applied**: repository settings
were not changed by anyone but the owner, and nothing was pushed.

## Description

```text
Open-source runtime for connecting AI agents to programmable hardware.
```

Website field: the documentation site, once it is published. Leave it empty until then.

## Topics

At most 20. Suggested (all describe what is in the repository today):

`ai-agents` `mcp` `model-context-protocol` `hardware` `lab-automation` `instrumentation`
`modbus` `mqtt` `scpi` `opc-ua` `ros2` `rust` `python` `simulation` `agent-safety` `iot`

To apply: `gh repo edit KNambiarDJsc/Open-MHS --description "..." --add-topic ai-agents ...`.

## Social preview

[`docs/assets/social-preview.png`](../assets/social-preview.png), 1280 x 640: the name, the
tagline and a one-line status. Upload it in Settings -> General -> Social preview. It
makes no claim the README does not.

## Settings to consider

| Setting | Recommendation | Why |
|---|---|---|
| General -> Discussions | Enable | SUPPORT.md and the README point questions there; without it they go to issues |
| Security -> Private vulnerability reporting | **Enable before announcing** | SECURITY.md and the issue-template chooser tell reporters to use it |
| Security -> Secret scanning and push protection | Enable | The repository handles credentials in code and tests |
| Security -> Dependabot alerts and security updates | Enable | Advisories for crates and Python |
| Dependabot version updates | Consider adding `.github/dependabot.yml` (below) | Keeps pinned actions current |
| Branch protection on `main` | Require pull requests and these checks: `Rust (ubuntu-latest)`, `Rust (windows-latest)`, `Dependencies (licences, advisories, sources)`, `Spec fixtures and schema portability`, `Registry, compatibility matrix and release metadata`. Disallow force-push and deletion | Matches the project's standing rule that `main` never rewrites history. (Check the exact check names in the Actions tab; macOS is slow to schedule and can stay optional) |
| Actions -> General -> Workflow permissions | Read repository contents by default | Workflows already declare their own permissions |
| Actions -> General -> Fork pull request workflows | Require approval for first-time contributors | |
| Environments | Create `pypi` (required reviewer) before the first PyPI publish; `github-pages` is created when Pages is enabled | The publish jobs wait on them |
| Pages | Source: GitHub Actions, only when you are ready to publish the docs | The site is not published until the *Docs* workflow runs with *deploy* |
| Releases | Keep releases as drafts until verified | `release.yml` already creates drafts |
| Tag protection (rulesets) | Protect `v*` tags | A tag starts the release workflow |

### Dependabot (proposal, not added)

```yaml
version: 2
updates:
  - package-ecosystem: cargo
    directory: /
    schedule: { interval: weekly }
  - package-ecosystem: pip
    directory: /python
    schedule: { interval: weekly }
  - package-ecosystem: github-actions
    directory: /
    schedule: { interval: weekly }
```

Note the project's rule that new dependencies and tools should be at least two weeks old;
review automated bumps against it. The release workflow's actions are pinned to SHAs, and
Dependabot can update the SHA and its trailing version comment together.

## Before the repository is made public (or announced)

- Private vulnerability reporting is on (SECURITY.md depends on it).
- The conduct contact in CODE_OF_CONDUCT.md works for you.
- A maintainer has read the README's claims against
  [limitations](../limitations.md) one more time.
- Discussions is on, or SUPPORT.md is edited to say it is not.
