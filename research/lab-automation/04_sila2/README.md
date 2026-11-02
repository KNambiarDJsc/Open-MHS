# 04 · SiLA 2: a standard instrument interface through one generic adapter

Quickstart: [EXPERIMENT_REPORT.md](EXPERIMENT_REPORT.md) §18. Raw results: [results/](results/).

| Path | What it is |
|---|---|
| [feature/](feature/) | The Feature Definition used (written for this research, not a SiLA standard feature) |
| [servers/heater_a](servers/heater_a/), [servers/heater_b](servers/heater_b/) | Two simulated servers generated from it with `sila2-codegen`, implemented separately |
| [overrides/](overrides/) | What OpenMHS needs that the definition does not say, reviewed |
| [native_baseline/](native_baseline/) | The SDK's client alone |
| [experiments/](experiments/) | SILA-01..05 and a latency run; run against both servers for SILA-06 |

The adapter itself is generic: [adapters/python/sila2](../../../adapters/python/sila2/).
