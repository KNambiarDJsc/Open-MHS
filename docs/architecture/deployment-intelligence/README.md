# Open-MHS as deployment intelligence: audit and proposed architecture

- **Status:** reviewed and accepted 2026-11-15/16; first slice merged to `main`.
- **Date:** 2026-11-15, against `main` at `d05c699`.

## Decisions taken in review (2026-11-15)

| Question | Decision |
|---|---|
| First users | **Mobile manipulation, humanoid robot labs and university fleets.** They run learned policies, judge them on small numbers of trials, and feed interventions back into training |
| Direction | Build release comparison and outcome provenance **together** in the first slice, not incident timelines alone. For these users the most valuable question is "did the new policy, checkpoint or setting actually do better, and who says so?" |
| Demo installs | Approved: LeRobot with PyTorch (Demo 2) and Nav2 (a Demo 1 variant). Nav2 is deprioritized: navigation-only failures matter less to the chosen users than manipulation and policy evaluation |
| CI advisories | Fix now (`rmcp ~3.7`, `rusqlite 0.41`) |
| Push | Push the design branch |
| Gateway | **Not deprecated: a supported extension** for governed execution (2026-11-16, ADR-0026) |

What changes because of the users: small samples are the norm, so `insufficient_evidence` and per-trial clustering
warnings matter more than scale; episodes, interventions and checkpoint lineage (Horizon B) move ahead of fleet
operations features; Demo 2 (policy recovery and checkpoint comparison) becomes the lead demo once it can be installed.

## Recommendation in one paragraph

Build a small, open, local-first layer that turns recordings, task events and deployment manifests into
**incidents with explained evidence** and **release comparisons with honest statistics**, beside the robot and never
in its control path. Start from files teams already have (MCAP recordings, CI deployment data), not from a new edge
agent. Keep the existing gateway as a supported, opt-in extension for governed execution, and one event source. Reuse the repository's best
asset, its explicit treatment of uncertainty, as the outcome model: reported, verified and unknown outcomes are never
merged. Integrate with Foxglove, Roboto, InOrbit/OpenRobOps, `ros2_medkit` and LeRobot instead of rebuilding them. Treat
the next 90 days as a test of whether anyone outside the project wants this, with explicit kill and narrow criteria.

## What the audit found

1. The repository is a **governed command gateway** (in the actuation path), with no notion of robot, site, task,
   deployment, release or model. Its only deployment metadata is a free-text firmware string in device descriptors.
2. Its strongest reusable assets are the **uncertainty discipline** (19-state lifecycle with `UNKNOWN_OUTCOME` and
   reconciliation), the **outcome verifier's time and evidence model** (RFC 0004), the **local durability patterns**
   (WAL, quota with critical reserve, sealed segments, explicit loss counters), and the research **MCAP/ROS 2 bridge**.
3. Debt: CI on `main` is red from four dependency advisories; migrations are implicit; documentation has drifted;
   every verified result is simulation or replay.
4. The market is crowded: AI triage of robot logs (Roboto, Foxglove), open-source robot ops with incidents
   (OpenRobOps, Apache-2.0, September 2026), ROS fault management (`ros2_medkit`). What is thin is release-aware
   outcome statistics with stated denominators and uncertainty, and outcome provenance.

## Decisions to review

| Decision | Choice | Question for the reviewer |
|---|---|---|
| [ADR-0026](../../adr/0026-deployment-intelligence-beside-execution.md) | Observation-only layer; gateway a supported extension and one source | Decided: extension, not deprecated |
| [ADR-0027](../../adr/0027-import-first.md) | Import first, SDK spool second, collector only on demand | Accept that there is no edge agent in the first 90 days? |
| [ADR-0028](../../adr/0028-event-envelope.md), [RFC 0005](../../rfcs/0005-deployment-events.md) | Small envelope, namespaced extensions, typed relations, content-addressed manifests | Is the field set right? Anything missing for your target users? |
| [ADR-0029](../../adr/0029-ops-storage.md) | SQLite → PostgreSQL + Parquet → column store, on stated triggers | Agree on the triggers? |
| [ADR-0030](../../adr/0030-correlation-not-causation.md) | Deterministic, explained correlation; no causal claims | Acceptable that the product shows evidence rather than "root causes"? |
| [ADR-0031](../../adr/0031-ops-api.md) | HTTP/JSON for the ops service; gateway keeps JSON-RPC | Two API styles in one repository acceptable? |
| [ADR-0032](../../adr/0032-release-comparison-statistics.md) | Exposure denominators, Wilson/Newcombe, CMH strata, `insufficient_evidence` | Regression margin default (proposed: 1 percentage point, or 25 % relative)? |
| [ADR-0033](../../adr/0033-outcome-provenance.md) | Reported, verified and annotated outcomes kept separate | — |
| [ADR-0034](../../adr/0034-ops-crates-and-languages.md) | Two new Rust crates; Python importer and SDK in the existing package | Accept Python on the import path? |

## Assumptions

- Target users already record MCAP (or can) and have CI that knows what it deployed.
- 0.5 structured events per robot per second is a reasonable middle estimate (range 0.1-1.0).
- One engineer, with AI assistance, for the first 90 days.
- No paying customer or design partner exists yet.

## Open questions

1. Which first users: mobile manipulation startups, warehouse AMR fleets, lab automation, or university fleets? The
   schema works for all; the demos and outreach should pick one.
2. ~~Gateway: frozen or deprecated?~~ Decided 2026-11-16: a supported extension.
3. Approval needed for Demo 2 (LeRobot + PyTorch, several GB) and for a Nav2 variant of Demo 1 (Nav2 + TurtleBot3
   packages in WSL).
4. Should the strategy documents of the earlier directions be marked superseded now, or after this review?

## Documents

| Artifact | Document |
|---|---|
| A. Repository audit | [01-repository-audit.md](01-repository-audit.md) |
| Ecosystem research | [02-landscape.md](02-landscape.md) |
| B. Architecture decision records | [ADR-0026 to ADR-0034](../../adr/README.md) |
| C. System architecture, sequence diagrams, horizons | [03-system-architecture.md](03-system-architecture.md) |
| D. Data architecture | [RFC 0005](../../rfcs/0005-deployment-events.md) |
| E. Integration design | [04-integrations.md](04-integrations.md) |
| F. API specifications | [05-api.md](05-api.md) |
| G. Scalability, reliability, security, quality | [06-scale-reliability-security.md](06-scale-reliability-security.md) |
| H, I. Roadmap and issue breakdown | [07-roadmap-and-issues.md](07-roadmap-and-issues.md) |
| J. First vertical slice, demos | [08-vertical-slice-and-demos.md](08-vertical-slice-and-demos.md) |
| Critical review, product and business | [09-critical-review.md](09-critical-review.md) |
| First slice: what was built and measured, including the first real recording | [10-first-slice-results.md](10-first-slice-results.md) |

## Tests run with this audit (2026-11-15, Windows 11)

| Suite | Result |
|---|---|
| `cargo test --workspace` | 623 passed, 0 failed |
| `python -m unittest discover -s python/tests` (with `OPENMHS_BIN` set to the built binary) | 66 passed, 0 failed (one `ResourceWarning`, an unclosed file) |
| GitHub CI on `main` (`d05c699`) | Failing: `cargo deny` advisories RUSTSEC-2026-0457, -0459, -0461, -0462. All other jobs pass |

Run without `OPENMHS_BIN`, the Python suite reports 30 failures and 15 errors because it cannot find the binary; that
is the test's documented requirement, not a defect.
