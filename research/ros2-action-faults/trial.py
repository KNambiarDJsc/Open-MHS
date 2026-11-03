"""Run one trial: one logical operation, one fault, one arm, one seed."""

from __future__ import annotations

import hashlib

import checker
from clients import APPLIED, NOT_APPLIED, UNKNOWN_OUT, run_arm
from model import ClientCrash, Env, draw_params

FAULTS = ["F0", "F1", "F2", "F3", "F4", "F5", "F6"]
SCENARIOS = ["non-idempotent", "idempotent"]
_FINAL_EVENT = {APPLIED: "ok", NOT_APPLIED: "fail", UNKNOWN_OUT: "timeout"}


def trial_seed(base_seed: int, fault: str, scenario: str, index: int) -> int:
    """Same seed for every arm of a given (fault, scenario, index): the design is paired."""
    digest = hashlib.sha256(f"{base_seed}:{fault}:{scenario}:{index}".encode()).hexdigest()
    return int(digest[:12], 16)


def run_trial(arm: str, fault: str, scenario: str, seed: int) -> dict:
    idempotent = scenario == "idempotent"
    env = Env(fault=fault, params=draw_params(seed))
    op_id = f"op-{seed}"
    env.log("invoke", op_id)
    journal: dict = {}  # survives a client crash; only the durable arm reads it
    outcome = UNKNOWN_OUT
    for _restart in range(3):
        try:
            outcome = run_arm(arm, env, op_id, journal)
            break
        except ClientCrash:
            env.log("info", op_id, note="client crashed and restarted; supervisor resubmits")
    env.log(_FINAL_EVENT[outcome], op_id)
    # Let the plant run to quiescence so late effects are counted before labelling.
    env.server.advance(env.now + 60.0)

    g = env.plant.count(op_id)
    duplicate = (not idempotent) and g >= 2
    false_confirmation = outcome == APPLIED and g == 0
    false_denial = outcome == NOT_APPLIED and g >= 1
    expected = set()
    if duplicate:
        expected.add("V1")
    if false_confirmation:
        expected.add("V3")
    if false_denial:
        expected.add("V4")
    found = {v["type"] for v in checker.check(env.history, env.plant.ledger, idempotent)}
    return {
        "arm": arm, "fault": fault, "scenario": scenario, "seed": seed,
        "outcome": outcome, "effects": g, "sends": env.n_sends,
        "D": duplicate, "FC": false_confirmation, "FD": false_denial,
        "U": outcome == UNKNOWN_OUT,
        "V": duplicate or false_confirmation or false_denial,
        "fault_triggered": env.fault_triggered(),
        "t_end": round(env.now, 3),
        "checker_types": sorted(found), "checker_agrees": found == expected,
    }
