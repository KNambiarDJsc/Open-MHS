"""Import closed-loop policy rollouts (PushT, LeRobot diffusion policy, simulation) as Open-MHS events (RFC 0005).

    python -m openmhs.importers.policy_rollouts RUNS_DIR --model-revision HUB_SHA --lerobot 0.7.0 --out events.jsonl

Standard library only. RUNS_DIR holds the files written by ``research/outcome-verifier/policy-demo/policy_rollouts.py``:
``<arm>.<first>-<end>.jsonl``, a header line (the arm's configuration) then one line per episode.

SIMULATION. There is no robot and no human. What becomes what:

- one **deployment manifest per arm**: the model (hub revision and the SHA-256 of the weights), the software versions,
  and the sampling configuration with its digest. A different sampler or number of denoising steps is a different
  manifest, so a release comparison between arms compares exactly that change;
- one **task attempt per episode** (``task_type`` ``pusht``), whose **scenario** is the seed: the same seed is the same
  starting position under every arm, which is what lets a comparison pair the arms;
- the episode's outcome is what the harness reported (``gym_pusht`` success: covered area over 95 %). The policy makes no
  claim of its own, so this is ``outcome.reported`` with its source named;
- the value the harness judged on, as ``outcome.reported.conditions`` (ADR-0036): the episode's peak reward against 1.
  ``gym_pusht``'s reward is the covered area divided by 0.95, capped at 1, so a peak of 1 is the success threshold (the
  flag needs strictly over 0.95; equality is the only case where the two differ). The condition's result is the
  harness's own success flag, copied, never recomputed here. A self-judgement by the simulator, not a verification;

- an ``episode.started`` / ``episode.ended`` pair per attempt, with the model and sampler recorded in ``ext``;
- the results file as an **artifact** by reference;
- the wall-clock time of the episode as `cost.wall_s` on the task end: the events' own clock is simulated (steps at
  10 fps), so this is the only record of what the attempt cost to compute.

Not done, on purpose: no intervention events (nobody took over), no action chunks, no rewards per step.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from typing import Any

TENANT = "policy-demo"
SITE = "pusht-simulation"
FPS = 10.0
# Simulation time: each episode starts a fixed distance after the previous seed, so times are ordered and never overlap.
BASE_NS = 1_794_000_000 * 1_000_000_000
SLOT_NS = 1_000 * 1_000_000_000


def canonical(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def digest(value: Any) -> str:
    return "sha256:" + hashlib.sha256(canonical(value).encode("utf-8")).hexdigest()


def rfc(ns: int) -> str:
    import datetime as dt

    s, rem = divmod(ns // 1_000_000, 1000)
    return dt.datetime.fromtimestamp(s, dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S") + f".{rem:03d}Z"


def read_arm(files: list[Path]) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    header: dict[str, Any] | None = None
    episodes: dict[int, dict[str, Any]] = {}
    for f in files:
        for line in f.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            row = json.loads(line)
            if "header" in row:
                h = row["header"]
                if header is not None and {k: header[k] for k in CONFIG_KEYS} != {k: h[k] for k in CONFIG_KEYS}:
                    raise ValueError(f"{f.name}: its configuration differs from the other shards of arm {h['arm']}")
                header = header or h
            else:
                episodes[row["seed"]] = row
    if header is None:
        raise ValueError("no header line")
    return header, [episodes[s] for s in sorted(episodes)]


CONFIG_KEYS = ("arm", "model_sha256", "n_action_steps", "horizon", "n_obs_steps", "num_train_timesteps",
               "noise_scheduler", "num_inference_steps", "max_steps")


def manifest(header: dict[str, Any], model_revision: str, lerobot: str) -> dict[str, Any]:
    sampler = {k: header.get(k) for k in ("noise_scheduler", "num_inference_steps", "num_train_timesteps",
                                           "n_action_steps", "horizon", "n_obs_steps")}
    return {
        "schema": "openmhs.deployment/0.1",
        "components": [
            {"kind": "model", "name": "diffusion_pusht", "source": "hf://lerobot/diffusion_pusht",
             "revision": model_revision, "checkpoint_digest": "sha256:" + header["model_sha256"]},
            {"kind": "software", "name": "lerobot", "version": lerobot},
            {"kind": "software", "name": "torch", "version": header.get("torch", "unknown")},
            {"kind": "config", "name": "sampler", "config_digest": digest(sampler), **sampler},
            {"kind": "hardware", "name": "inference", "device": header.get("device", "cpu")},
        ],
        "labels": {"release": header["arm"]},
    }


def reward_condition(ep: dict[str, Any]) -> dict[str, Any]:
    """What the harness judged the episode on (ADR-0036): its peak reward, against the reward at the success threshold.
    The result is the harness's own success flag."""
    return {"name": "peak reward reaches 1", "evidence": "reward", "op": "ge", "value": 1.0,
            "result": "true" if ep["success"] else "false", "observed": [ep["max_reward"]]}


def import_arm(files: list[Path], model_revision: str, lerobot: str) -> list[dict[str, Any]]:
    header, episodes = read_arm(files)
    arm = header["arm"]
    m = manifest(header, model_revision, lerobot)
    md = digest(m)
    robot = f"pusht-policy/{arm}"
    domain = f"sim:pusht/{arm}"
    out: list[dict[str, Any]] = [m]
    out.append({"schema": "openmhs.artifact/0.1", "tenant": TENANT,
                "artifact_id": digest([[f.name, f.stat().st_size] for f in files]),
                "uri": ";".join(f.resolve().as_uri() for f in files), "media_type": "application/x-ndjson",
                "robot": robot, "dataset": "policy rollouts, PushT, simulation"})

    def event(eid: str, kind: str, ns: int, **fields: Any) -> None:
        e: dict[str, Any] = {
            "schema": "openmhs.event/0.1", "event_id": f"{arm}/{eid}", "type": kind,
            "source": {"id": f"policy-rollouts/{arm}", "kind": "import", "boot_id": arm},
            "time": {"observed": rfc(ns), "clock_domain": domain, "uncertainty_ms": 0},
            "scope": {"tenant": TENANT, "site": SITE, "robot": robot}, "provenance": "imported",
        }
        e.update({k: v for k, v in fields.items() if v is not None})
        out.append(e)

    event("deployment", "deployment.applied", BASE_NS - SLOT_NS, deployment={"manifest": md})
    for ep in episodes:
        seed = ep["seed"]
        start = BASE_NS + (seed - episodes[0]["seed"]) * SLOT_NS
        end = start + int(ep["steps"] / FPS * 1e9)
        task_id = f"{arm}/seed-{seed}"
        subject = {"task_id": task_id, "task_type": "pusht", "scenario_id": f"pusht/seed-{seed}",
                   "episode_id": task_id}
        ext = {"pusht": {"seed": seed, "steps": ep["steps"], "max_reward": ep["max_reward"],
                         "final_reward": ep["final_reward"], "wall_seconds": ep["seconds"]}}
        event(f"{seed}/task.started", "task.started", start, subject=subject, deployment={"manifest": md}, ext=ext)
        event(f"{seed}/episode.started", "episode.started", start, subject=subject, ext=ext)
        outcome = {"reported": {"value": "succeeded" if ep["success"] else "failed", "source": "gym_pusht.is_success",
                                "detail_code": "coverage>0.95" if ep["success"] else f"max_reward_{ep['max_reward']}",
                                "conditions": [reward_condition(ep)]}}
        event(f"{seed}/episode.ended", "episode.ended", end, subject=subject, outcome=outcome, ext=ext)
        event(f"{seed}/task.ended", "task.ended", end, subject=subject, outcome=outcome, ext=ext,
              cost={"wall_s": ep["seconds"]})
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("runs", type=Path, help="directory of <arm>.<first>-<end>.jsonl files")
    ap.add_argument("--model-revision", required=True, help="the hub commit of the checkpoint (immutable)")
    ap.add_argument("--lerobot", required=True, help="the installed lerobot version")
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()
    arms: dict[str, list[Path]] = {}
    for f in sorted(args.runs.glob("*.jsonl")):
        arms.setdefault(f.name.split(".")[0], []).append(f)
    if not arms:
        print(f"{args.runs}: no .jsonl files", file=sys.stderr)
        return 1
    lines: list[str] = []
    for files in arms.values():
        lines.extend(canonical(r) for r in import_arm(files, args.model_revision, args.lerobot))
    args.out.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print(f"{len(lines)} records from {len(arms)} arms -> {args.out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
