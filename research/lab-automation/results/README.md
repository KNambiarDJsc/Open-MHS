# Results

Simulated, every one. No result here says anything about a physical effect. Each directory holds raw
result files and a `SUMMARY.md` derived from them by [`../summarize.py`](../summarize.py).

## 2026-11-02: Windows, three runs

[2026-11-02/SUMMARY.md](2026-11-02/SUMMARY.md). One Windows 11 desktop, release gateway, `reproduce.py
--runs 3`. The machine was also compiling at times, so its timings are noisier than the earlier
reports' and should not be compared with them.

**MAD-05 and MAD-08 show `FAIL` here, 2/3, and that is correct to leave.** Each failed one run on the
check that a command is held for approval. The scenario looked as soon as the command existed, and
twice caught it during `VALIDATING`, the moment before the hold. The gateway and the step behaved as
designed; the check was wrong. It was fixed in `49417ab`, and rerun:

## 2026-11-02-madsci-rerun: MADSci again, with the fixed check

[2026-11-02-madsci-rerun/SUMMARY.md](2026-11-02-madsci-rerun/SUMMARY.md). The same machine and gateway,
three more runs, all eleven scenarios.

## Linux: CI artifacts, not committed

One run of conformance, contract gaps and scenarios for each ecosystem, from a clean install of the
locks, on GitHub's `ubuntu-latest`, Python 3.12, debug gateway. Run
[41464110182](https://github.com/KNambiarDJsc/Open-MHS/actions/runs/41464110182) at `1b8fd52`, read from
its artifacts, not from its status:

| Ecosystem | Conformance | Contract gaps | Scenarios |
|---|---|---|---|
| PyLabRobot | L3, 64 pass, 0 fail, 0 untested | 10 of 10 | 8 of 8 |
| Opentrons | L3, 44 pass, 0 fail, 0 untested | 10 of 10 | 8 of 8 |
| Micro-Manager | L3, 89 pass, 0 fail, 0 untested | 10 of 10 | 8 of 8 |
| SiLA 2 | L3, 39 pass, 0 fail, 0 untested | 12 of 12, on both servers | 16 of 16, on both servers |
| MADSci | n/a | n/a | 11 of 11 |

GitHub keeps artifacts for a limited time. One run on one kind of runner is thin evidence, and worth
stating as exactly that. The Micro-Manager job is marked experimental in CI and may fail without failing
the build, though here it passed.

## Older results

Each experiment's own `results/` directory holds the runs its report was written from, before the
hardening. Those files were left as they were: they show, for example, MM-05 concluding
`RECONCILED_NOT_APPLIED` for a move that happened, which is what the finding was about.
