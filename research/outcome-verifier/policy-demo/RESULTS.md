# Results: a learned policy under two sampling configurations (simulation)

Plan: [EVALUATION.md](EVALUATION.md), committed before any episode of this run had finished. Reproduce: [README.md](README.md).

**SIMULATION.** A pretrained diffusion policy pushing a T-shaped block in the PushT simulation, on CPU. No robot, no human,
nothing about a physical system or about operator behaviour.

## What was run

`lerobot/diffusion_pusht` at revision `84a7c231…` (weights SHA-256 `995d14d3…`), LeRobot 0.7.0, CPU. Two arms on the same
20 starting states (seeds 1000 to 1019), 300 steps at 10 fps per episode: **`ddpm-100`**, the checkpoint as published (DDPM,
100 denoising steps), and **`ddim-10`**, the same weights sampled with DDIM, 10 steps. Success is the environment's own flag
(covered area over 95 %), recorded as the *reported* outcome; no verified outcome exists (the simulator is executor and
judge). Runs were done on 2026-11-16, 18:24 to 20:04.

Imported as 166 records (two deployment manifests, two artifacts, 40 task attempts) and ingested into a fresh store: 40
attempts, 13 incidents, 2 rollouts. Importing the same file again: 166 duplicates, nothing changed.

## The comparison

```
openmhs ops compare --task-type pusht --a ddpm-100 --b ddim-10 --paired
```

| | scenarios | succeeded | failed | failure rate (95 % Wilson) |
|---|---|---|---|---|
| `ddpm-100` | 20 | 15 | 5 | 25.0 % (11.2 to 46.9) |
| `ddim-10` | 20 | 12 | 8 | 40.0 % (21.9 to 61.3) |

Difference `ddim-10` minus `ddpm-100`: **+15.0 points, 95 % interval -13.4 to +40.4** (Newcombe, paired).

Paired by scenario, 20 pairs: both fail 2, only `ddpm-100` fails 3, only `ddim-10` fails 6, neither fails 9. McNemar's exact
two-sided **p = 0.51**. Nothing was left out: no unknown outcomes, no unmatched scenarios.

**Verdict: `insufficient_evidence`**: "fewer than 30 matched scenario pairs with a known outcome (20)". The minimum was not
lowered. The interval is wide enough to contain no difference, a 13-point improvement from the faster sampler, and a
40-point loss. These runs do not say whether the 10-step sampler is worse; they say that 20 scenarios cannot tell.

## Predictions against what happened

1. **Reproduction: held.** `ddpm-100` succeeded on 15 of 20 seeds (75 %), inside the pre-registered 43 to 82 % window
   around the published 65.4 %. The normalization and environment setup reproduce the published behaviour to the
   resolution 20 episodes allow. Neither arm had zero successes.
2. **No detectable difference, `insufficient_evidence`: held.** `ddim-10` was lower (60 % against 75 %) but not
   distinguishable, and the verdict was `insufficient_evidence` as predicted. I had predicted `ddim-10` would be "close"
   to `ddpm-100`; 15 points apart is not obviously close, and the data cannot say whether that gap is real.
3. **Hard seeds fail under both arms, so pairing narrows the interval: not supported.** Two scenarios failed under both
   arms (seeds 1001 and 1011), but with 5 and 8 failures out of 20 independent failures would give two in expectation
   (5 × 8 / 20 = 2.0). The pair table has no excess correlation (φ = 0), so the paired interval is identical to the
   unpaired one (-13.4 to +40.4 either way). Pairing did not help here, and I have no evidence from this run that seeds are
   intrinsically hard. With more scenarios that could change.
4. **Cost: held, with a caveat.** Mean wall time per episode was 825 s for `ddpm-100` and 126 s for `ddim-10` (medians
   742 s and 128 s), a factor of 6.5 (5.8 by median), inside the predicted 5 to 10. The caveat: three `ddpm-100`
   shards and one `ddim-10` shard shared a 16-thread machine with 4 threads per process, and the load changed as shards
   finished. This is an order of magnitude, not a benchmark.

## Failure analysis

- Eleven of the 13 failures ended just under the success threshold (maximum reward 0.96 to 1.0): the block was pushed nearly
  into place and not far enough. One `ddpm-100` failure (seed 1001, maximum reward 0.096) barely moved the block; one
  `ddim-10` failure (seed 1014) reached 0.71. These are reported outcomes: whether a near miss "failed" is the environment's threshold,
  not a physical judgement.
- Among successes, the median episode length was 162 steps under `ddpm-100` and 168.5 under `ddim-10`: successful episodes
  look alike.
- The two manifests differ in exactly one component, `sampler` (scheduler DDPM or DDIM, number of inference steps); the
  model, software versions and hardware are identical. A release comparison between them therefore compares exactly one
  change, which is what the manifest exists to guarantee.

## What the tool did and did not show

Worked as designed: scenario-level pairing from the seed, the two arms as two content-addressed releases, the correct
denominator (20 scenarios per arm), a refusal to call a verdict on 20 pairs, idempotent re-import, 13 failed episodes as
incidents that can be listed as a selection for evaluation or training, and an interval a reader can act on.

Gaps found while doing this, not fixed here:

- **Duration is simulated time, not compute (fixed afterwards).** The comparison reports episode duration as steps divided by
  10 fps (median 18.6 s against 23.2 s, both capped at 30 s by the step limit), so the wall-clock cost that distinguishes
  the arms was invisible in the report. Events can now carry `cost.wall_s`, the importer emits it, and `ops compare` prints
  it: re-importing the same results gives a median wall time of 741.8 s for `ddpm-100` and 127.7 s for `ddim-10` (a
  factor of 5.8), the same figures as computed directly from the result files. These are the producer's own timings on a
  shared machine, as noted under prediction 4.
- `ops compare` listed at most five incidents per arm without saying so; it now says "newest 5" when the list is cut
  (one-line change in this commit).
- The failure's *shape* (nearly there, did not move) is in `ext` but incidents carry only `reported_failed`; there is
  nothing to group failures by.

## Limits

20 scenarios, one checkpoint, one simulator, CPU only, one run per episode (a rerun of one episode is reproducible in
distribution, not bit for bit). The success flag comes from the same simulator that executes. Nothing here involves a
person taking over, observation or action data in the incident layer, or a physical robot, and nothing here is evidence
about one.

## Deviations from the plan

- `run_arms.sh` comments quoted per-episode times estimated before the run ("about 5 minutes", "about 45 seconds");
  measured under shared load they were about 14 and 2 minutes.
- Four processes were run at a time, not eight: eight exhausted memory in the first launch, so the first attempt was stopped
  and relaunched with the shard layout in `run_arms.sh`. No episode from the failed launch was used.
- **A quantile bug in the tool, found here.** The first comparison output quoted medians of 20.5 s and 25.6 s for the
  simulated durations, and the first wall-time output 800.6 s and 142.8 s against 742 s and 128 s computed from the files:
  for an even count the tool took the upper of the two middle values. Fixed (interpolated quantiles) before this page was
  finalised; every figure above is from the fixed tool. The same bug had affected the duration figures in the Gazebo
  write-up, corrected there.

## Next, if wanted

Seeds 1020 to 1029 would reach the 30 pairs the tool requires. This would be an extension decided after seeing these
results, so it would be reported as one, with the 20-seed result kept as the pre-registered one. A second checkpoint or
an observation-level quantity (action chunks) would need the importer and schema to carry them.
