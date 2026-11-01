# Lab automation: OpenMHS across scientific instrument ecosystems

Research question: can one execution boundary enforce the same contracts, permissions,
lifecycle, recovery and evidence across heterogeneous laboratory automation software,
without replacing that software? Answered by experiment, one ecosystem at a time; the
OpenFlexure microscope work in [research/](../) is the completed baseline.

Everything here is simulated. Nothing connects to an instrument, and no result here says
anything about a physical effect.

| Stage | Project | Status |
|---|---|---|
| 0 | [Ecosystem assessment](00_ECOSYSTEM_ASSESSMENT.md) | All five installed, run natively and assessed; revised after the experiments, with what was not inspected listed |
| 1 | [PyLabRobot](01_pylabrobot/) | **Done**: 7 scenarios and a latency run pass in repeated runs; conformance L1, with L2 blocked by a finding ([report](01_pylabrobot/EXPERIMENT_REPORT.md)) |
| 2 | [Opentrons](02_opentrons/) | **Done**: 7 scenarios and a latency run pass in three runs; conformance L1, the same gap as 01 ([report](02_opentrons/EXPERIMENT_REPORT.md)) |
| 3 | [Micro-Manager](03_micromanager/) | **Done**: 7 scenarios and a latency run pass in three runs; conformance L1 with every stage, exposure and channel check passing at L2 and L3 ([report](03_micromanager/EXPERIMENT_REPORT.md)) |
| 4 | [SiLA 2](04_sila2/) | **Done**: 5 scenarios and a latency run pass, three runs each on two separately written servers; one generic adapter, identical descriptors (SILA-06) ([report](04_sila2/EXPERIMENT_REPORT.md)) |
| 5 | [MADSci](05_madsci/) | **Done**: 9 scenarios (two are native baselines) pass in three complete runs; a lost answer retried leaves one dose with OpenMHS and two without ([report](05_madsci/EXPERIMENT_REPORT.md)) |
| 6 | Cross-project evaluation | **Done**: [FINAL_RESEARCH_REPORT.md](FINAL_RESEARCH_REPORT.md) answers the ten questions, with the gaps left visible |

Reusable adapters live under [adapters/python/](../../adapters/python/); this directory holds
the experiments, their harnesses and their results.

Also here: [blog notes](BLOG_NOTES.md), points to build a write-up from, and
[shared/harness.py](shared/harness.py), the harness all five experiments use.
