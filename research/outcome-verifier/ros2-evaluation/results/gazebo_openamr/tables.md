## Claims per run

| Run | Kind | Label | Claims | Native | Joint-state script right | Ground-truth script right | Verifier |
|---|---|---|---|---|---|---|---|
| bullet1 | lift | `AT_TARGET` | 5 | says there | 5 | 5 | `SATISFIED` 5 |
| bullet1 | lift | `NOT_AT_TARGET` | 5 | says there (wrong) | 5 | 5 | `VIOLATED` 5 |
| bullet1 | base | `AT_TARGET` | 99 | says there | n/a | 99 | `SATISFIED` 99 |
| bullet1 | base | `NOT_AT_TARGET` | 23 | says there (wrong) | n/a | 23 | `VIOLATED` 23 |
| bullet2 | lift | `AT_TARGET` | 5 | says there | 5 | 5 | `SATISFIED` 5 |
| bullet2 | lift | `NOT_AT_TARGET` | 5 | says there (wrong) | 5 | 5 | `VIOLATED` 5 |
| bullet2 | base | `AT_TARGET` | 97 | says there | n/a | 97 | `SATISFIED` 97 |
| bullet2 | base | `NOT_AT_TARGET` | 23 | says there (wrong) | n/a | 23 | `VIOLATED` 23 |
| bullet3 | lift | `AT_TARGET` | 5 | says there | 5 | 5 | `SATISFIED` 5 |
| bullet3 | lift | `NOT_AT_TARGET` | 5 | says there (wrong) | 5 | 5 | `VIOLATED` 5 |
| bullet3 | base | `AT_TARGET` | 99 | says there | n/a | 99 | `SATISFIED` 99 |
| bullet3 | base | `NOT_AT_TARGET` | 23 | says there (wrong) | n/a | 23 | `VIOLATED` 23 |
| dart1 | lift | `AT_TARGET` | 2 | says there | 2 | 2 | `SATISFIED` 2 |
| dart1 | lift | `NOT_AT_TARGET` | 8 | says there (wrong) | 8 | 8 | `VIOLATED` 8 |
| dart1 | base | `AT_TARGET` | 98 | says there | n/a | 98 | `SATISFIED` 98 |
| dart1 | base | `NOT_AT_TARGET` | 24 | says there (wrong) | n/a | 24 | `VIOLATED` 24 |

## False SATISFIED

- bullet1: verifier false SATISFIED 0 []; verifier differs from label on 0 []
- bullet2: verifier false SATISFIED 0 []; verifier differs from label on 0 []
- bullet3: verifier false SATISFIED 0 []; verifier differs from label on 0 []
- dart1: verifier false SATISFIED 0 []; verifier differs from label on 0 []

## Lift goals: carriage error at the reported success (m), by run

| Goal | Target (m) | bullet1 | bullet2 | bullet3 | dart1 |
|---|---|---|---|---|---|
| L01 | 0.02 | -0.0009 SATISFIED | -0.0008 SATISFIED | -0.0009 SATISFIED | -0.0200 VIOLATED |
| L02 | 0.05 | -0.0010 SATISFIED | -0.0009 SATISFIED | -0.0008 SATISFIED | -0.0500 VIOLATED |
| L03 | 0.1 | -0.0149 VIOLATED | -0.0148 VIOLATED | -0.0147 VIOLATED | -0.1000 VIOLATED |
| L04 | 0.2 | -0.0288 VIOLATED | -0.0285 VIOLATED | -0.0287 VIOLATED | -0.2000 VIOLATED |
| L05 | 0.25 | -0.0012 SATISFIED | -0.0010 SATISFIED | -0.0010 SATISFIED | -0.2500 VIOLATED |
| L06 | 0.34 | -0.0540 VIOLATED | -0.0545 VIOLATED | -0.0542 VIOLATED | -0.3400 VIOLATED |
| L07 | 0.1 | +0.0966 VIOLATED | +0.0964 VIOLATED | +0.0962 VIOLATED | -0.1000 VIOLATED |
| L08 | 0.05 | +0.0011 SATISFIED | +0.0010 SATISFIED | +0.0012 SATISFIED | -0.0500 VIOLATED |
| L09 | 0.0 | +0.0324 VIOLATED | +0.0324 VIOLATED | +0.0321 VIOLATED | +0.0000 SATISFIED |
| L10 | 0.0 | +0.0000 SATISFIED | +0.0000 SATISFIED | +0.0000 SATISFIED | +0.0000 SATISFIED |

## Drive segments: odometry travel vs Gazebo travel (m)

| Run | Task | Command | Odometry | Gazebo | Ratio |
|---|---|---|---|---|---|
| bullet1 | B01 | 0.2 m/s, 0.0 rad/s, 3.0 s | 0.6056 | 0.1097 | 5.5x |
| bullet1 | B02 | 0.0 m/s, 0.5 rad/s, 2.0 s | 0.0002 | 0.0003 | - |
| bullet1 | B03 | 0.2 m/s, 0.0 rad/s, 3.0 s | 0.6023 | 0.1117 | 5.4x |
| bullet1 | B04 | -0.1 m/s, 0.0 rad/s, 2.0 s | 0.2003 | 0.002 | - |
| bullet2 | B01 | 0.2 m/s, 0.0 rad/s, 3.0 s | 0.6054 | 0.1129 | 5.4x |
| bullet2 | B02 | 0.0 m/s, 0.5 rad/s, 2.0 s | 0.0002 | 0.0005 | - |
| bullet2 | B03 | 0.2 m/s, 0.0 rad/s, 3.0 s | 0.6013 | 0.1085 | 5.5x |
| bullet2 | B04 | -0.1 m/s, 0.0 rad/s, 2.0 s | 0.2012 | 0.0032 | - |
| bullet3 | B01 | 0.2 m/s, 0.0 rad/s, 3.0 s | 0.598 | 0.1127 | 5.3x |
| bullet3 | B02 | 0.0 m/s, 0.5 rad/s, 2.0 s | 0.0001 | 0.0003 | - |
| bullet3 | B03 | 0.2 m/s, 0.0 rad/s, 3.0 s | 0.6008 | 0.1124 | 5.3x |
| bullet3 | B04 | -0.1 m/s, 0.0 rad/s, 2.0 s | 0.202 | 0.0029 | - |
| dart1 | B01 | 0.2 m/s, 0.0 rad/s, 3.0 s | 0.603 | 0.0001 | - |
| dart1 | B02 | 0.0 m/s, 0.5 rad/s, 2.0 s | 0.0 | 0.0 | - |
| dart1 | B03 | 0.2 m/s, 0.0 rad/s, 3.0 s | 0.6036 | 0.0 | - |
| dart1 | B04 | -0.1 m/s, 0.0 rad/s, 2.0 s | 0.2019 | 0.0 | - |

## Injected faults (bullet1, 120 trials)

| Fault | Changes | Trials | Verifier | Reasons | Script right | Script false "there" | Verifier false `SATISFIED` |
|---|---|---|---|---|---|---|---|
| self_confirming | source | 20 | `UNKNOWN` 20 | evidence.missing | 11 | 9 | 0 |
| other_clock | values | 20 | `UNKNOWN` 20 | evidence.rejected, time.clock_domain | 15 | 0 | 0 |
| reference_gap | availability | 20 | `UNKNOWN` 20 | evidence.outside_window, evidence.rejected | 0 | 0 | 0 |
| other_attempt | metadata | 20 | `UNKNOWN` 20 | evidence.identity_mismatch, timing.anchor_missing | 20 | 0 | 0 |
| frozen_reference | values, undetectable by design | 20 | `SATISFIED` 1, `VIOLATED` 19 | - | 15 | 0 | 0 |
| reordered | nothing | 20 | `SATISFIED` 6, `VIOLATED` 14 | - | 20 | 0 | 0 |

## Other measurements

- bullet1: claims {'lift': 10, 'base': 122}, determinism True (12), selection True (4), status placement uncertainty {'min': 50.5, 'median': 50.5, 'max': 50.5, 'n': 10} ms, median verify 0.133 s
- bullet2: claims {'lift': 10, 'base': 120}, determinism True (13), selection True (4), status placement uncertainty {'min': 50.5, 'median': 50.5, 'max': 50.5, 'n': 10} ms, median verify 0.128 s
- bullet3: claims {'lift': 10, 'base': 122}, determinism True (12), selection True (4), status placement uncertainty {'min': 50.5, 'median': 50.5, 'max': 50.5, 'n': 10} ms, median verify 0.129 s
- dart1: claims {'lift': 10, 'base': 122}, determinism True (12), selection True (4), status placement uncertainty {'min': 50.5, 'median': 50.5, 'max': 50.5, 'n': 10} ms, median verify 0.124 s
