"""Independent noisy measurement channel and the Bayesian resolution rule.

A reading is dropped with probability DROPOUT; otherwise it reads 1 with probability
1 - EPS_FN when the effect happened (G >= 1) and with probability EPS_FP when it did not.
"""

from __future__ import annotations

import math

EPS_FP = 0.02
EPS_FN = 0.05
DROPOUT = 0.10
PRIOR = 0.5
ALPHA = 0.01
N_READINGS = 5

APPLIED = "applied"
NOT_APPLIED = "not_applied"
UNSURE = "unsure"


def posterior_applied(readings, eps_fp=EPS_FP, eps_fn=EPS_FN, prior=PRIOR) -> float | None:
    """P(applied | readings) assuming conditionally independent noise. None if no valid reading."""
    valid = [r for r in readings if r is not None]
    if not valid:
        return None
    k = sum(valid)
    n_valid = len(valid)
    log_odds = (
        math.log(prior / (1.0 - prior))
        + k * math.log((1.0 - eps_fn) / eps_fp)
        + (n_valid - k) * math.log(eps_fn / (1.0 - eps_fp))
    )
    if log_odds > 700:
        return 1.0
    if log_odds < -700:
        return 0.0
    return 1.0 / (1.0 + math.exp(-log_odds))


def decide(readings, alpha=ALPHA, **kw) -> str:
    post = posterior_applied(readings, **kw)
    if post is None:
        return UNSURE
    if post >= 1.0 - alpha:
        return APPLIED
    if post <= alpha:
        return NOT_APPLIED
    return UNSURE


def decision_probabilities(applied: bool, n=N_READINGS, dropout=DROPOUT, eps_fp=EPS_FP,
                           eps_fn=EPS_FN, alpha=ALPHA) -> dict[str, float]:
    """Exact probability of each decision given the true state, by enumeration."""
    p1 = (1.0 - eps_fn) if applied else eps_fp
    out = {APPLIED: 0.0, NOT_APPLIED: 0.0, UNSURE: 0.0}
    for valid in range(0, n + 1):
        p_valid = math.comb(n, valid) * (1 - dropout) ** valid * dropout ** (n - valid)
        for k in range(0, valid + 1):
            p_k = math.comb(valid, k) * p1 ** k * (1 - p1) ** (valid - k)
            readings = [1] * k + [0] * (valid - k) + [None] * (n - valid)
            out[decide(readings, alpha=alpha, eps_fp=eps_fp, eps_fn=eps_fn)] += p_valid * p_k
    return out
