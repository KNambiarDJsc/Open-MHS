# Pre-registration: unknown outcomes of ROS 2 actions (Layer 0, model harness)

Status: written and committed **before any harness code or any trial was run**. The commit hash of this
file is the pre-registration identifier. Any later change is recorded in `DEVIATIONS.md`, not edited
here.

## What this layer is, and is not

Layer 0 is a **model of ROS 2 action semantics derived from reading the ROS 2 source and design
documents** (in-memory goal table, client-generated goal UUID, duplicate-UUID rejection while the goal
exists, result expiry with a 10 s default, `STATUS_UNKNOWN` for any goal the server does not know,
cancel as a request). It runs in virtual time with no ROS 2 installed.

- Results describe **client strategies against this model**. They are **not** measurements of ROS 2.
- Several outcomes below follow from the modelled semantics by construction (for example, that a naive
  client which resends with a fresh UUID can duplicate an effect). Those outcomes validate the harness
  and the model, not the world. The informative results are the *relative* effects of stable identity,
  durability and independent measurement, and the unresolved-versus-wrong trade-off.
- Layer 1 (real `rclpy` in the CI-pinned Jazzy image) is required before any statement about ROS 2.
- The arm that uses OpenMHS (arm D) is **not built in this layer**. Hypothesis H5 (governed arm versus
  careful durable arm) is therefore untested here.
- Simulation is not physical validation. OpenMHS is not a functional-safety system.

## Model

All times are virtual seconds. Each trial is a pure function of a seed; the **same seed drives every arm**
(paired design). Per-trial random parameters are drawn once from the seed, independent of the arm.

| Parameter | Value |
|---|---|
| Goal duration D | U(2, 5) |
| Effect time within a goal | accept time + f·D, f ~ U(0.2, 0.8) |
| Result expiry after goal finish | 10 (ROS 2 default, from source reading) |
| Client send_goal response timeout | 1.0 |
| Client get_result wait timeout | 8.0 |
| Operation deadline | 60 |
| Server restart delay r | U(1, 3) |
| Client restart delay (F6) | U(1, 3) |

A ground-truth **ledger** in the plant records every effect event with the logical operation id. No
client can read it. Labels come only from the ledger: G(o) is the number of effect events for logical
operation o.

## Faults (nemesis), one per trial cell, timing randomised

| ID | Fault |
|---|---|
| F0 | none (control; expected violations: 0 for every arm) |
| F1 | the first goal-accept response is lost; the goal is accepted and runs |
| F2 | the first get_result response is lost; the server keeps the result |
| F3 | server killed at accept + U(0, D) and restarted after r; in-memory table cleared; goal in flight dies |
| F4 | client stalls for U(12, 30) after the goal is accepted, then asks for the result (often after expiry) |
| F5 | the server reports SUCCEEDED for the first goal but applies no effect |
| F6 | client crashes at dispatch + U(0.1, D + 1) and restarts after U(1, 3); the supervisor resubmits the same logical operation |

F7 (measurement-channel faults) is exercised only through the default channel noise below; a sensitivity
sweep is exploratory and out of scope for the confirmatory results. F8 (out-of-limit goal, policy) does
not apply to arms A to C and is deferred to arm D.

## Scenarios

- **non-idempotent**: each delivered execution adds one dose. A violation is G ≥ 2.
- **idempotent control**: the effect sets a target position. G ≥ 2 is **not** a violation. The control
  exists to show where retry is harmless.

## Arms

- **A, native-naive**: fresh UUID per attempt; any timeout, non-SUCCEEDED result or `STATUS_UNKNOWN`
  triggers a resend; at most 3 attempts; reports APPLIED on the first SUCCEEDED result, otherwise
  NOT_APPLIED. Forgets everything on a client crash.
- **B, careful in-memory**: one random UUID per logical operation held in memory. After a timeout it
  queries `get_result` on the same UUID before any resend. `STATUS_UNKNOWN` is **never** treated as
  proof of "not applied". Resend only on definite evidence of not-applied (an ABORTED result, or a
  measurement posterior ≤ α). At most 2 sends. Forgets the UUID on a client crash (so it sends a fresh
  UUID after restart).
- **C, careful durable**: B plus an intent journal that survives a client crash: after restart it reuses
  the UUID and queries before acting.
- Each of B and C runs **M0** (no measurement access) and **M1** (independent noisy measurement
  channel). A runs M0 only. That is five variants: A, B-M0, B-M1, C-M0, C-M1.

These careful clients are deliberately conservative (they never resend on `STATUS_UNKNOWN`). This favours
the careful arms and therefore makes any later claim that OpenMHS adds value harder to establish, not
easier.

## Measurement channel (M1)

For an operation in doubt, and also to confirm every reported SUCCEEDED, the client waits 6 s, then takes
L = 5 readings 0.5 s apart. Each reading is dropped with probability 0.10; otherwise it reads 1 with
probability 1 − ε_fn = 0.95 when G ≥ 1 and with probability ε_fp = 0.02 when G = 0. Prior π0 = 0.5.
Posterior odds are computed as in the report (§5), with α = 0.01: resolve APPLIED if posterior ≥ 0.99,
NOT_APPLIED if ≤ 0.01, otherwise UNKNOWN. By Bayes' rule the wrong-resolution probability is at most α
**only if** the channel model is calibrated and noise is independent of the fault. Here the simulation
makes both true by construction, so the observed wrong-resolution rate is a check of the code, not of any
real sensor.

## Trial counts

- Pilot: n = 30 per cell, for debugging only. No pilot number is reported as a result.
- Confirmatory: **n = 200** per cell, cells = 7 faults × 2 scenarios × 5 variants = 70 cells, 14,000
  trials, one fixed base seed (recorded in the run record) chosen before the run.

## Outcome labels and metrics (labels from the ledger only)

For each trial the client reports one of APPLIED, NOT_APPLIED, UNKNOWN.

- **D** (duplicate): non-idempotent and G ≥ 2.
- **FC** (false confirmation): reports APPLIED and G = 0.
- **FD** (false denial): reports NOT_APPLIED and G ≥ 1.
- **U** (unresolved): reports UNKNOWN.
- **Violation V** = D or FC or FD.
- **Omission**: reports NOT_APPLIED with G = 0 after giving up (an honest failure, not a violation).

Every proportion is reported with an exact two-sided 95 % Clopper-Pearson interval. With zero events in
n = 200 the upper bound is about 1.8 %.

## Hypotheses (model form) and what falsifies them

| ID | Statement in model form | Falsified if |
|---|---|---|
| H1m | Under F1 to F4 and F6, arm A produces duplicates in a non-zero fraction of non-idempotent trials | the 95 % upper bound on D for arm A is below 2 % in every one of those cells |
| H2m | B removes duplicates under F1 and F2 but not under F6; C removes them under F6 too, at the price of some unresolved outcomes | B shows no more duplicates than C under F6 |
| H3m | Without measurement (M0) every careful arm leaves a non-zero unresolved fraction under F3, F4 and F6; with M1 the unresolved fraction is lower and the wrong-resolution rate stays within the α bound plus sampling error | M1 does not lower U at a margin of 5 percentage points, or its wrong-resolution rate exceeds the Clopper-Pearson upper bound for α |
| H4m | Under F5, arms without measurement (A, B-M0, C-M0) record FC in essentially every trial; arms with M1 detect it at a rate consistent with the channel error | arms without measurement avoid FC, or M1 arms miss F5 more often than the channel error predicts |
| H5 | careful durable equals the governed arm | **not testable in this layer** (arm D not built) |

H1m and the structure of H2m are largely consequences of the model. They are pre-registered so a bug in
the harness would show up as a failure to reproduce them, not as evidence about ROS 2.

## Confirmatory contrasts

Paired exact McNemar on discordant seeds, Holm-corrected over these four, non-idempotent scenario only:

1. **C1 identity**: A versus B-M0 on violation V, pooled over F1, F2, F3, F4.
2. **C2 durability**: B-M0 versus C-M0 on violation V, F6 only.
3. **C3 measurement**: C-M0 versus C-M1 on the unresolved indicator U, pooled over F3, F4, F6.
4. **C4 false confirmation**: A versus C-M1 on FC, F5 only.

Effect sizes are reported as rate differences with intervals. All other comparisons are descriptive.

## History checker

Each client emits `invoke`, `ok`, `fail`, `timeout` and `info` events with the logical operation id. The
checker, given the client history and the effect log, flags V1 duplicate effect, V2 phantom effect
(an effect with no invoked operation), V3 false confirmation, V4 false denial. A timed-out operation is
indeterminate. The checker is validated by **mutation**: corrupted histories (dropped effect, duplicated
effect, flipped client result, injected phantom effect) must be flagged, and the checker's per-cell
counts must equal the metric code's counts. Because the checker is given the true effect log, **its
agreement with the ledger is a consistency check between two implementations, not an estimate of how a
checker would perform on noisy real logs.** That question stays open until a real measurement channel
exists.

## Stopping and reporting rules

- Failures and null results are reported as run, including any cell where the model or a client
  misbehaves.
- No number is rounded toward a favourable story. Raw per-trial records and the run record (commit,
  dirty flag, Python version, seed) accompany every result.
- Results are described as "in the model of ROS 2 action semantics" everywhere they appear.

## Out of scope for this layer

Arm D (OpenMHS-governed), the `ros2_action` adapter, real `rclpy`, DDS-level loss, network emulation,
physical hardware, F8 policy checks, Bench-schema export, and a sensitivity sweep over channel error.
