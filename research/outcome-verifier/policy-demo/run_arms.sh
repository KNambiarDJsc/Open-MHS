#!/bin/bash
# Two arms on the same seeds, in parallel shards (each process pinned to a few CPU threads).
#   arm ddpm-100: the checkpoint as published (DDPM, 100 denoising steps), about 14 minutes per episode when four shards share 16 CPU threads
#   arm ddim-10 : the same weights sampled with DDIM, 10 steps, about 2 minutes per episode under the same load
# Usage: run_arms.sh MODEL_DIR OUT_DIR PYTHON [FIRST_SEED END_SEED]
# Resumable: finished seeds in an output file are skipped.
set -u
MODEL=${1:?model dir}; OUT=${2:?out dir}; PY=${3:?python of the lerobot environment}
FIRST=${4:-1000}; END=${5:-1020}
HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$OUT"
shard() { # arm first end threads extra...
  local arm=$1 a=$2 b=$3 th=$4; shift 4
  "$PY" "$HERE/policy_rollouts.py" --model "$MODEL" --arm "$arm" --seeds "$a" "$b" --threads "$th" \
    --out "$OUT/$arm.$a-$b.jsonl" "$@" > "$OUT/$arm.$a-$b.log" 2>&1 &
}
# Memory, not CPU, is the limit: each process holds the 1 GB model plus activations (about 2 GB), and eight of them
# exhausted a 16 GB machine. Defaults: 3 shards for arm A and 1 for arm B, 4 threads each.
A_SHARDS=${A_SHARDS:-3}; B_SHARDS=${B_SHARDS:-1}; THREADS=${THREADS:-4}
span=$((END - FIRST))
for ((i = 0; i < A_SHARDS; i++)); do
  a=$((FIRST + i * span / A_SHARDS)); b=$((FIRST + (i + 1) * span / A_SHARDS))
  shard ddpm-100 "$a" "$b" "$THREADS"
done
for ((i = 0; i < B_SHARDS; i++)); do
  a=$((FIRST + i * span / B_SHARDS)); b=$((FIRST + (i + 1) * span / B_SHARDS))
  shard ddim-10 "$a" "$b" "$THREADS" --sampler ddim --inference-steps 10
done
wait
echo ARMS_DONE
