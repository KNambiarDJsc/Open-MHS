# Lab automation: OpenMHS across scientific instrument ecosystems

Research question: can one execution boundary enforce the same contracts, permissions,
lifecycle, recovery and evidence across heterogeneous laboratory automation software,
without replacing that software? Answered by experiment, one ecosystem at a time; the
OpenFlexure microscope work in [research/](../) is the completed baseline.

Everything here is simulated. Nothing connects to an instrument, and no result here says
anything about a physical effect.

| Stage | Project | Status |
|---|---|---|
| 0 | [Ecosystem assessment](00_ECOSYSTEM_ASSESSMENT.md) | PyLabRobot assessed; the other four not yet inspected beyond their package versions |
| 1 | [PyLabRobot](01_pylabrobot/) | **Done**: 7 scenarios and a latency run pass in four runs; conformance L1, with L2 blocked by a finding ([report](01_pylabrobot/EXPERIMENT_REPORT.md)) |
| 2 | [Opentrons](02_opentrons/) | **Done**: 7 scenarios and a latency run pass in three runs; conformance L1, the same gap as 01 ([report](02_opentrons/EXPERIMENT_REPORT.md)) |
| 3 | Micro-Manager | Not started |
| 4 | SiLA 2 | Not started |
| 5 | MADSci | Not started (needs two reproducible integrations first) |
| 6 | Cross-project evaluation and final report | Not started |

Reusable adapters live under [adapters/python/](../../adapters/python/); this directory holds
the experiments, their harnesses and their results.
