# Policy sampler comparison (simulation)

A pretrained diffusion policy pushes a T-shaped block in the PushT simulation under two sampling configurations. The
rollouts are imported as OpenMHS events and the two releases are compared on the same scenarios.

**This is a simulation.** There is no robot and no human. The plan is in [EVALUATION.md](EVALUATION.md), written before
the measured run; what happened is in `RESULTS.md`.

## Pieces

| File | What it does |
|---|---|
| `policy_rollouts.py` | Runs episodes of a LeRobot diffusion policy in `gym-pusht`, one JSON line per episode, resumable. Runs in the LeRobot environment, not in the OpenMHS package. |
| `run_arms.sh` | Runs the two arms on the same seeds in parallel shards. |
| `../../../python/openmhs/importers/policy_rollouts.py` | Turns the result files into OpenMHS events: a deployment manifest per arm, a task attempt per episode, the seed as the scenario. Standard library only. |

## Reproduce

1. A Python 3.12 environment with `lerobot==0.7.0`, `gym-pusht==0.1.8` and CPU PyTorch. Download
   `lerobot/diffusion_pusht` at revision `84a7c23178445c6bbf7e1a884ff497017910f653` and check the SHA-256 of
   `model.safetensors` against the one in EVALUATION.md.
2. Run both arms (each process holds about 1 GB of model plus activations; memory, not CPU, limits how many shards fit):

   ```
   bash run_arms.sh MODEL_DIR runs PYTHON 1000 1020
   ```

3. Import, ingest, compare:

   ```
   python -m openmhs.importers.policy_rollouts runs \
       --model-revision 84a7c23178445c6bbf7e1a884ff497017910f653 --lerobot 0.7.0 --out events.jsonl
   openmhs ops ingest events.jsonl --store ops.sqlite
   openmhs ops compare --store ops.sqlite --task-type pusht --a ddpm-100 --b ddim-10 --paired
   ```

   The same command without `--paired` treats the arms as independent samples, for comparison.

Results are not bit-for-bit repeatable: a rerun of one episode is reproducible in distribution (the torch seed is the
episode seed), not exactly.
