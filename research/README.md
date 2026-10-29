# Research: OpenMHS x LabThings x OpenFlexure (simulator only)

Start with [DECISION_MEMO.md](DECISION_MEMO.md). No physical hardware was used; no hardware,
safety, endorsement or collaboration claim is made.

| Phase | Document |
|---|---|
| 1 Capabilities | [CURRENT_CAPABILITIES.md](CURRENT_CAPABILITIES.md) |
| 2 Environment | [EXPERIMENT_ENVIRONMENT.md](EXPERIMENT_ENVIRONMENT.md) |
| 3 Thing Description importer | [WOT_IMPORTER_RESULTS.md](WOT_IMPORTER_RESULTS.md) |
| 4 Natural-language agents | [NATURAL_LANGUAGE_EXPERIMENTS.md](NATURAL_LANGUAGE_EXPERIMENTS.md) |
| 5 Async action reliability | [ASYNC_ACTION_RELIABILITY.md](ASYNC_ACTION_RELIABILITY.md) |
| 6 Audit and provenance | [AUDIT_AND_PROVENANCE.md](AUDIT_AND_PROVENANCE.md) |
| 7 Multi-device workflow | [MULTI_DEVICE_EXPERIMENT.md](MULTI_DEVICE_EXPERIMENT.md) |
| 8 Reliability benchmark | [RELIABILITY_BENCHMARK.md](RELIABILITY_BENCHMARK.md) |
| 9 Generalization | [GENERALIZATION_RESULTS.md](GENERALIZATION_RESULTS.md) |
| 10 Upstream opportunities | [UPSTREAM_OPPORTUNITIES.md](UPSTREAM_OPPORTUNITIES.md) |
| 11 Publication assessment | [PUBLICATION_ASSESSMENT.md](PUBLICATION_ASSESSMENT.md) |
| 12 Collaboration brief (draft, unsent) | [COLLABORATION_BRIEF.md](COLLABORATION_BRIEF.md) |

Code: `experiments/` (harness), `generalization/` (second device), `fixtures/`. Raw results:
`results/`. Importer and adapter: `../adapters/python/wot/`. The next phase (distributed
architecture) is specified in [../docs/handoff/](../docs/handoff/HANDOFF.md).
