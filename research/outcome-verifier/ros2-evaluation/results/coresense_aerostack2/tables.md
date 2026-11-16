| Behavior | Claims | At target (label) | Script right | Verifier `SATISFIED` | `VIOLATED` | `UNKNOWN` | Error at claim (m): median / max |
|---|---|---|---|---|---|---|---|
| GoTo | 107 | 9 | 107 | 4 | 96 | 7 | 0.265 / 0.289 |
| Takeoff | 9 | 9 | 9 | 9 | 0 | 0 | 0.193 / 0.195 |
| Land | 8 | 6 | 8 | 6 | 2 | 0 | 0.067 / 0.408 |

| Flight | Claims | `SATISFIED` | `VIOLATED` | `UNKNOWN` |
|---|---|---|---|---|
| no_failure | 41 | 7 | 32 | 2 |
| panel_failure | 42 | 6 | 33 | 3 |
| double_failure | 41 | 6 | 33 | 2 |

| GoTo tolerance (xy) | Within tolerance at the claim |
|---|---|
| 0.10 m | 3 of 107 |
| 0.20 m (planned) | 9 of 107 |
| 0.30 m | 107 of 107 |

Verifier false `SATISFIED`: 0. Verifier differs from the label on 7 claims (Counter({('evidence.conflict', 'evidence.outside_window'): 7})). Script differs from the label on 0.

| Injected fault | Trials | Verifier | Reasons | Script right | Script false `SATISFIED` | Verifier false `SATISFIED` |
|---|---|---|---|---|---|---|
| wrong_frame | 124 | `UNKNOWN` 124 | evidence.frame, evidence.rejected | 115 | 0 | 0 |
| other_clock | 124 | `UNKNOWN` 124 | evidence.rejected, time.clock_domain | 100 | 1 | 0 |
| reference_gap | 124 | `UNKNOWN` 124 | evidence.outside_window, evidence.rejected | 0 | 0 | 0 |
| other_attempt | 124 | `UNKNOWN` 124 | evidence.identity_mismatch, timing.anchor_missing | 124 | 0 | 0 |

Determinism: True (13 claims verified twice). Median verification 0.109 s per claim.
