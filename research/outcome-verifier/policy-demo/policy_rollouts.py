"""Closed-loop rollouts of a pretrained LeRobot diffusion policy in the PushT simulation, one JSON line per episode.

    python policy_rollouts.py --model MODEL_DIR --arm NAME --seeds 0 20 --out arm.jsonl [--n-action-steps 8]

SIMULATION. Runs in the LeRobot environment (see README.md), not in the OpenMHS Python package. It reads the model and
writes results; it never contacts a robot.

Normalization is applied here, from the statistics stored inside the checkpoint's `model.safetensors`
(`normalize_inputs.*`, `unnormalize_outputs.*`): LeRobot 0.7 moved normalization out of the policy model, and a
checkpoint saved before that carries the numbers but the loader does not use them. Without this the policy would run on
raw pixels and positions.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import time
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")

import gymnasium as gym  # noqa: E402
import gym_pusht  # noqa: E402,F401  (registers the environment)
import numpy as np  # noqa: E402
import torch  # noqa: E402
from lerobot.policies.diffusion.configuration_diffusion import DiffusionConfig  # noqa: E402
from lerobot.policies.diffusion.modeling_diffusion import DiffusionPolicy  # noqa: E402
from safetensors import safe_open  # noqa: E402


def load_stats(model_dir: Path) -> dict[str, torch.Tensor]:
    with safe_open(str(model_dir / "model.safetensors"), "pt") as f:
        return {k: f.get_tensor(k) for k in f.keys() if "normalize" in k}  # noqa: SIM118


class Normalizer:
    """MEAN_STD for the image, MIN_MAX to [-1, 1] for state and action, with the checkpoint's own numbers."""

    def __init__(self, stats: dict[str, torch.Tensor]):
        self.mean = stats["normalize_inputs.buffer_observation_image.mean"]
        self.std = stats["normalize_inputs.buffer_observation_image.std"]
        self.s_min = stats["normalize_inputs.buffer_observation_state.min"]
        self.s_max = stats["normalize_inputs.buffer_observation_state.max"]
        self.a_min = stats["unnormalize_outputs.buffer_action.min"]
        self.a_max = stats["unnormalize_outputs.buffer_action.max"]

    def image(self, pixels: np.ndarray) -> torch.Tensor:
        x = torch.from_numpy(pixels).float().div(255.0).permute(2, 0, 1)
        return ((x - self.mean) / self.std).unsqueeze(0)

    def state(self, agent_pos: np.ndarray) -> torch.Tensor:
        x = torch.from_numpy(agent_pos).float()
        return (2.0 * (x - self.s_min) / (self.s_max - self.s_min) - 1.0).unsqueeze(0)

    def action(self, normalized: torch.Tensor) -> np.ndarray:
        a = (normalized.squeeze(0) + 1.0) / 2.0 * (self.a_max - self.a_min) + self.a_min
        return a.numpy().astype(np.float32)


def run_episode(policy: DiffusionPolicy, norm: Normalizer, env, seed: int, max_steps: int) -> dict:
    t0 = time.time()
    obs, _ = env.reset(seed=seed)
    policy.reset()
    torch.manual_seed(seed)
    rewards: list[float] = []
    success = False
    chunks = 0
    for step in range(max_steps):
        batch = {"observation.image": norm.image(obs["pixels"]), "observation.state": norm.state(obs["agent_pos"])}
        with torch.no_grad():
            before = len(policy._queues["action"]) if hasattr(policy, "_queues") else None
            action = policy.select_action(batch)
            if before == 0:
                chunks += 1
        obs, reward, terminated, truncated, info = env.step(norm.action(action))
        rewards.append(float(reward))
        if terminated or info.get("is_success"):
            success = True
            break
        if truncated:
            break
    return {"seed": seed, "success": success, "steps": len(rewards), "max_reward": round(max(rewards), 4),
            "final_reward": round(rewards[-1], 4), "rewards": [round(r, 3) for r in rewards],
            "seconds": round(time.time() - t0, 1)}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", type=Path, required=True)
    ap.add_argument("--arm", required=True)
    ap.add_argument("--seeds", type=int, nargs=2, metavar=("FIRST", "END"), required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--n-action-steps", type=int, default=None, help="override the checkpoint's n_action_steps")
    ap.add_argument("--max-steps", type=int, default=300)
    ap.add_argument("--sampler", choices=["ddpm", "ddim"], default=None,
                    help="override the checkpoint's noise scheduler (it was trained with DDPM; DDIM samples faster)")
    ap.add_argument("--inference-steps", type=int, default=None, help="denoising steps at inference (default: all training steps)")
    ap.add_argument("--threads", type=int, default=None)
    args = ap.parse_args()
    if args.threads:
        torch.set_num_threads(args.threads)

    config = DiffusionConfig.from_pretrained(str(args.model))
    if args.sampler:
        config.noise_scheduler_type = args.sampler.upper()
    if args.inference_steps:
        config.num_inference_steps = args.inference_steps
    policy = DiffusionPolicy.from_pretrained(str(args.model), config=config)
    if args.n_action_steps is not None:
        policy.config.n_action_steps = args.n_action_steps
    policy.eval()
    norm = Normalizer(load_stats(args.model))
    env = gym.make("gym_pusht/PushT-v0", obs_type="pixels_agent_pos", render_mode="rgb_array", max_episode_steps=args.max_steps)

    header = {"arm": args.arm, "model": str(args.model),
              "model_sha256": hashlib.sha256((args.model / "model.safetensors").read_bytes()).hexdigest(),
              "n_action_steps": policy.config.n_action_steps, "horizon": policy.config.horizon,
              "n_obs_steps": policy.config.n_obs_steps, "num_train_timesteps": policy.config.num_train_timesteps,
              "noise_scheduler": policy.config.noise_scheduler_type, "num_inference_steps": policy.config.num_inference_steps,
              "max_steps": args.max_steps, "torch": torch.__version__,
              "device": "cpu", "started": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    done = set()
    if args.out.exists():
        for line in args.out.read_text(encoding="utf-8").splitlines():
            row = json.loads(line)
            if "seed" in row:
                done.add(row["seed"])
    else:
        args.out.write_text(json.dumps({"header": header}) + "\n", encoding="utf-8", newline="\n")
    for seed in range(*args.seeds):
        if seed in done:
            continue
        row = {"arm": args.arm, **run_episode(policy, norm, env, seed, args.max_steps)}
        with open(args.out, "a", encoding="utf-8", newline="\n") as f:
            f.write(json.dumps(row) + "\n")
        print(f"{args.arm} seed {seed}: success={row['success']} steps={row['steps']} max_reward={row['max_reward']} {row['seconds']} s", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
