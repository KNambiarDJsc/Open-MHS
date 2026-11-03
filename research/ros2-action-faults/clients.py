"""The client arms. Each returns APPLIED, NOT_APPLIED or UNKNOWN for one logical operation.

A  native-naive    fresh UUID per attempt; any non-SUCCEEDED outcome triggers a resend (max 3).
B  careful, memory one UUID per logical operation, kept in memory only.
C  careful, durable B plus an intent journal that survives a client crash.

B and C never treat STATUS_UNKNOWN as proof that nothing happened. They resend only on definite
evidence of "not applied" (an ABORTED result, or a measurement posterior <= alpha), at most once.
M1 adds access to the independent measurement channel; M0 has none.
"""

from __future__ import annotations

import channel
from model import ABORTED, SUCCEEDED, UNKNOWN, Env

APPLIED = "APPLIED"
NOT_APPLIED = "NOT_APPLIED"
UNKNOWN_OUT = "UNKNOWN"

ARMS = {
    "A": ("A", False),
    "B-M0": ("B", False),
    "B-M1": ("B", True),
    "C-M0": ("C", False),
    "C-M1": ("C", True),
}


def run_naive(env: Env, op_id: str) -> str:
    for _ in range(3):
        uuid = env.new_uuid()
        if env.send_goal(uuid, op_id) != "ACCEPTED":
            continue  # timeout or rejection: resend with a fresh UUID
        if env.get_result(uuid, op_id) == SUCCEEDED:
            return APPLIED
    return NOT_APPLIED


def _resolve(env: Env, uuid: str, op_id: str, measure_on: bool) -> str:
    """'APPLIED', 'NOT_APPLIED_DEFINITE' or 'UNKNOWN' for one goal id."""
    res = None
    for _ in range(3):
        res = env.get_result(uuid, op_id)
        if res is not None:
            break
    if res is None:
        return "UNKNOWN"
    if res == ABORTED:
        return "NOT_APPLIED_DEFINITE"
    if res == SUCCEEDED and not measure_on:
        return "APPLIED"
    if res == UNKNOWN and not measure_on:
        return "UNKNOWN"
    # measurement available: SUCCEEDED is verified, STATUS_UNKNOWN is resolved
    decision = channel.decide(env.measure(op_id))
    if decision == channel.APPLIED:
        return "APPLIED"
    if decision == channel.NOT_APPLIED:
        return "NOT_APPLIED_DEFINITE"
    return "UNKNOWN"


def run_careful(env: Env, op_id: str, measure_on: bool, journal: dict | None) -> str:
    state = journal.get(op_id) if journal is not None else None
    if state is None:
        state = {"uuid": env.new_uuid(), "sends": 0}
        if journal is not None:
            journal[op_id] = state  # intent is durable before the first send
        need_send = True
    else:
        need_send = False  # durable client after a crash: resolve what it already sent
    while True:
        if need_send:
            state["sends"] += 1
            env.send_goal(state["uuid"], op_id)
        verdict = _resolve(env, state["uuid"], op_id, measure_on)
        if verdict == "APPLIED":
            return APPLIED
        if verdict == "UNKNOWN":
            return UNKNOWN_OUT
        if state["sends"] >= 2:
            return NOT_APPLIED
        state["uuid"] = env.new_uuid()
        need_send = True


def run_arm(arm: str, env: Env, op_id: str, journal: dict | None) -> str:
    kind, measure_on = ARMS[arm]
    if kind == "A":
        return run_naive(env, op_id)
    return run_careful(env, op_id, measure_on, journal if kind == "C" else None)
