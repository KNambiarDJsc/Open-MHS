# Results: Layer 0 (model of ROS 2 action semantics)

**Read this first.** These results describe client strategies against a *model* of ROS 2 action
semantics that I wrote from reading ROS 2 sources. They are **not measurements of ROS 2, not
hardware results, and not field fault rates.** Fault timing is drawn from distributions I chose, so
the rates below depend on those choices (for example, 98/200 duplicates for the naive client under F3 is
just "the server was killed after the effect in about half the draws"). Several outcomes follow from the
modelled semantics by construction; they validate the harness, and the p-values below are not evidence
about the world. The careful clients were written by the same person who wrote the model.

Pre-registration: commit `9a29ea1`. Code: see `results/confirmatory/run_record.json`.
Reproduce: `python -I run_study.py --n 200 --base-seed 20261103 --out <dir>` (under a second; the raw
trial file is byte-identical across runs). Tests: `python -I test_harness.py` (36 tests).

## Non-idempotent effect, n = 200 per cell (count of trials; exact 95 % Clopper-Pearson upper limit when 0)

| Fault | A naive: duplicates | B-M0 careful memory: dup / unresolved | C-M0 careful durable: dup / unresolved | C-M1 durable + measurement: dup / unresolved |
|---|---|---|---|---|
| F0 none | 0 (≤1.8 %) | 0 / 0 | 0 / 0 | 0 / 1 |
| F1 lost accept response | **200** | 0 / 0 | 0 / 0 | 1 / 0 |
| F2 lost result response | **200** | 0 / 0 | 0 / 0 | 0 / 3 |
| F3 server kill + restart | **98** | 0 / **200** | 0 / **200** | 0 / 2 |
| F4 client stalls past result expiry | **186** | 0 / **186** | 0 / **186** | 0 / 0 |
| F5 success reported, no effect | 0 (false confirmations: **200**) | FC **200** | FC **200** | FC 0 / unresolved 6 |
| F6 client crash and restart | **166** | **166** / 0 | 0 / 0 | 0 / 1 |

Full per-cell table with all metrics and intervals: `results/confirmatory/summary.json`. The idempotent
control shows no violations for any arm except F5 (false confirmation, 200 for the arms without
measurement) and one false denial under F3 for the measured arms; retries are harmless there.
Arm B-M1 duplicated in all 200 F6 trials (it forgets its goal id on a crash and resends).

## Pre-registered hypotheses (model form)

| ID | Result | Notes |
|---|---|---|
| H1m naive duplicates | **Not falsified** | Largely a consequence of the model: a fresh UUID after a lost reply re-executes the goal |
| H2m memory vs durable | **Not falsified** | B-M0 duplicates in 166/200 F6 trials, C-M0 in 0; neither duplicates under F1/F2 |
| H3m measurement lowers unresolved by ≥5 points in F3, F4 and F6 | **Falsified as stated** | Holds for F3 (100 % → 1 %) and F4 (93 % → 0 %). Fails for F6: the careful arms without measurement have **0 %** unresolved there, so measurement cannot lower it (and adds 0.5 %). My prediction was wrong |
| H3m wrong-resolution within α | Not violated | 0 wrong final resolutions in 2,376 resolved measured non-idempotent trials (95 % upper limit 0.16 %). See caveat on duplicates below |
| H4m false confirmation | **Not falsified** | Arms without measurement: FC in 200/200. Measured arms: 0/200 (upper limit 1.8 %); the model predicts a miss probability of 0.015 % per verification |
| H5 durable = governed | **Not tested** | Arm D (OpenMHS) is not built in this layer |

## Confirmatory contrasts (paired exact McNemar, Holm over four, non-idempotent)

| Contrast | Discordant pairs (b, c) | Mean difference | Holm p |
|---|---|---|---|
| C1 identity: A vs B-M0, violations, F1–F4 (800 pairs) | 684, 0 | 0.855 | 1e-205 |
| C2 durability: B-M0 vs C-M0, violations, F6 (200) | 166, 0 | 0.83 | 2e-50 |
| C3 measurement: C-M0 vs C-M1, unresolved, F3/F4/F6 (600) | 384, 1 | 0.638 | 3e-113 |
| C4 false confirmation: A vs C-M1, FC, F5 (200) | 200, 0 | 1.00 | 2e-60 |

These p-values are extreme because the model makes the effects deterministic. They show the harness
discriminates the arms; they say nothing about how often ROS 2 deployments would see these faults.

## What the run does show

1. **Stable identity removes most duplicates; durability is needed only when the *client* can die.**
   Under server-side and message-loss faults a careful in-memory client matches the durable one. The
   difference appears only under client crash (F6).
2. **Holding is the price of not guessing.** Without an independent measurement, the careful clients
   avoid duplicates by leaving 93–100 % of F3/F4 operations unresolved. With the modelled channel nearly
   all are resolved (0–1 %).
3. **Measurement is not free of error.** The measured careful clients still produced one duplicate (F1;
   a run of false-negative readings made the client resend) and one wrong final "not applied" (idempotent
   F3). Both are the channel's modelled error rates showing up; both appear twice only because B-M1 and
   C-M1 share seeds. The "0 wrong resolutions" figure counts final reports and does **not** include
   duplicates caused by a wrong intermediate decision.
4. **A client that trusts the action result is fooled by a lying driver (F5) every time.** Success
   reporting without an effect is invisible to every arm that has no independent channel.
5. **The durable client's F6 advantage depends on the 10 s result retention.** After a short client
   outage it simply re-reads a retained result. A client outage longer than the expiry would present
   STATUS_UNKNOWN; that case was not in the pre-registered F6 (restart delay 1–3 s) and is the first
   follow-up.
6. **Exposure differs by arm in F6** (166 triggered for M0 arms, 200 for M1) because longer
   operations are exposed to the crash window longer; see `DEVIATIONS.md`.

## What it does not show

- Nothing about real ROS 2 (`rclpy`/`rcl` behaviour, DDS loss, QoS, discovery, goal-id handling edge
  cases), nothing about hardware, and nothing about real sensors: the "independent" channel is a model whose
  noise is independent of the fault by construction.
- Nothing about OpenMHS: arm D does not exist yet. The closest comparison to the OpenMHS claim is
  that **a careful durable client with measurement (C-M1) already reaches the performance a governed path
  could reach on duplicates and false confirmations in this model**. That is consistent with the
  pre-registered expectation for H5 and means any value OpenMHS adds must be shown in enforcement,
  policy refusal, audit and operational cost, not in these metrics.
- The checker agrees with the metric code in 14,000/14,000 trials. Because the checker is given the
  true effect log this is a consistency check, not an estimate of checker accuracy on noisy logs.

## Next steps in priority order

1. Layer 1: run the same fault set against a real `rclpy` action server and client in the CI-pinned
   Jazzy image. Until then, say "model of ROS 2 action semantics".
2. Add an F6 variant with a client outage longer than the result expiry.
3. Build arm D (a `ros2_action` adapter, descriptor, goal-id persistence) and test H5 honestly.
4. Replace the modelled measurement channel with a real one (the Modbus relay run) to calibrate
   the error rates.
