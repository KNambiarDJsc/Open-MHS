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
| Realistic imaging workflow (discover, survey, analyse, propose, adapt, fault, report) | [SAMPLE_SURVEY_WORKFLOW.md](SAMPLE_SURVEY_WORKFLOW.md) |
| Technician natural-language session (two MCP servers, one request) | [../examples/openflexure-agent/README.md](../examples/openflexure-agent/README.md) |
| OpenFlexure simulator walkthrough | [OPENFLEXURE_SIMULATOR_QUICKSTART.md](OPENFLEXURE_SIMULATOR_QUICKSTART.md) |
| Direct versus OpenMHS comparison | [DIRECT_VS_OPENMHS.md](DIRECT_VS_OPENMHS.md) |
| WoT units implementation investigation | [WOT_UNITS_PROPOSAL.md](WOT_UNITS_PROPOSAL.md) |

Code: `experiments/` (harness, including `openflexure_quickstart.py`), `generalization/`
(second device), and `fixtures/`. Raw results: `results/`. Importer and adapter:
`../adapters/python/wot/`. The OpenFlexure quickstart has been run; its receipts, traces and audit
reports are in `results/reports/openflexure_quickstart/`, including approval-gated and autonomous
GPT-6 Luna tool-calling runs that reached `VERIFIED` in the simulator, plus a guided capability-discovery square run with four matching readbacks and a valid audit chain. The existing benchmark is separate and
must not be presented as an OpenFlexure result. The LabThings unit metadata generation/import
probe passed; HTTP endpoint serialization was not separately exercised. Results are recorded in
`experiments/labthings_units_probe.py` and `results/reports/labthings_units_probe.json`. All
material remains simulator-only until supervised physical validation is separately planned.
