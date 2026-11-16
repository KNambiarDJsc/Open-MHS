# Evaluation plan: a learned policy under two sampling configurations, compared on the same scenarios

Written and committed **before any episode of the measured run had finished**. Changes afterwards go under Deviations in
`RESULTS.md`.

**SIMULATION.** A pretrained diffusion policy pushing a T-shaped block in the PushT simulation, on CPU. There is no robot
and no human. Nothing here is evidence about a physical robot or about how a person would intervene.

## Question

An engineer changes one thing in a deployed policy: the sampler (DDPM with 100 denoising steps, as trained and published,
against DDIM with 10 steps, about ten times faster). Did success change? Can OpenMHS answer that from the recorded
attempts alone, with the right denominator and honest uncertainty, and say when it cannot?

## Setup

- Model: `lerobot/diffusion_pusht` at hub revision `84a7c23178445c6bbf7e1a884ff497017910f653` (Apache-2.0), weights
  SHA-256 `995d14d35db57d95c35ad9704c3d79c8612b7bc45f3877e5c46c2cdc516856a8` (verified against the hub's published hash).
- Software: LeRobot 0.7.0, PyTorch 2.15 (CPU), `gym-pusht` 0.1.8, Windows. LeRobot 0.7 does not use the normalization
  statistics stored in this older checkpoint, so `policy_rollouts.py` applies them itself from the file. The loader says
  it is ignoring them; the runner does not.
- Arms: **`ddpm-100`**: the checkpoint as published (DDPM, 100 steps, `n_action_steps` 8). **`ddim-10`**: the same weights,
  DDIM scheduler, 10 inference steps; nothing else changes.
- Scenarios: seeds **1000 to 1019**, the same starting state for both arms (20 scenarios). More seeds can be added later;
  the runner resumes. 300 steps per episode at 10 fps. Success is the environment's own flag (covered area over 95 %).
- Each arm's torch seed is the episode seed, so a rerun of one episode is reproducible in distribution, not necessarily
  bit for bit.

## What is compared

OpenMHS `ops compare --paired` (matched scenarios, McNemar's exact test, Newcombe's paired interval) against the same
data treated as two independent samples, on the failure rate. Reported outcome only: the harness's success flag. No
verified outcome exists here (the simulator is both the executor and the judge).

## Predictions, before running

1. **Reproduction.** The `ddpm-100` success rate will fall inside the 95 % Wilson interval around the published figure
   (65.4 %, 500 episodes, `eval_info.json`) for 20 episodes: about 43 % to 82 %. If it does not, my normalization or
   environment setup is suspect and that is reported as a failed reproduction, before anything is said about the arms.
2. **No detectable difference.** `ddim-10` will be close to `ddpm-100` (the diffusion-policy literature reports DDIM
   with few steps working about as well), but 20 scenarios are too few to say so: the verdict will be
   `insufficient_evidence`, with the minimum of 30 pairs unmet. That is the correct answer and will be reported as such,
   not worked around by lowering the minimum.
3. **Some seeds are hard under both arms.** There will be scenarios that fail under both configurations, so the pairs are
   positively correlated and the paired interval is narrower than the unpaired one.
4. **Cost.** `ddim-10` will be roughly five to ten times faster per episode. That difference will be clear even with 20
   scenarios, and it is the other half of the trade-off.

## Also checked

Importing the same results twice changes nothing; the failed episodes appear as incidents and can be listed as a selection
for evaluation or training; the two arms have different manifests that differ only in the sampler configuration.

## Not tested

A human operator taking over. Action chunks, rewards per step, or any observation data in the incident layer. More than one
checkpoint. A physical robot. Whether CPU timings carry over to a robot's computer.

## Failure conditions

Prediction 1 failing is a failed reproduction. Any arm showing zero successes in 20 episodes means the setup is broken, not
that the policy is bad, and is investigated before reporting. The two arms disagreeing on `n_action_steps` or the horizon
in their headers is a setup error.
