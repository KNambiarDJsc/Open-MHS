"""Exact statistics used by the study. Pure standard library.

* clopper_pearson: exact two-sided binomial interval.
* mcnemar_exact: exact two-sided McNemar test on discordant pairs.
* holm: Holm step-down adjustment.
"""

from __future__ import annotations

import math


def _log_pmf(i: int, n: int, p: float) -> float:
    if p <= 0.0:
        return 0.0 if i == 0 else -math.inf
    if p >= 1.0:
        return 0.0 if i == n else -math.inf
    return (
        math.lgamma(n + 1)
        - math.lgamma(i + 1)
        - math.lgamma(n - i + 1)
        + i * math.log(p)
        + (n - i) * math.log1p(-p)
    )


def binom_cdf(k: int, n: int, p: float) -> float:
    """P(X <= k) for X ~ Binomial(n, p)."""
    if k < 0:
        return 0.0
    if k >= n:
        return 1.0
    return min(1.0, sum(math.exp(_log_pmf(i, n, p)) for i in range(0, k + 1)))


def clopper_pearson(k: int, n: int, conf: float = 0.95) -> tuple[float, float]:
    """Exact two-sided Clopper-Pearson interval for k successes in n trials."""
    if n <= 0:
        raise ValueError("n must be positive")
    if not 0 <= k <= n:
        raise ValueError("k must be in [0, n]")
    alpha = (1.0 - conf) / 2.0

    def bisect(f, lo: float, hi: float) -> float:
        for _ in range(100):
            mid = (lo + hi) / 2.0
            if f(mid):
                hi = mid
            else:
                lo = mid
        return (lo + hi) / 2.0

    if k == 0:
        lower = 0.0
    else:
        # smallest p with P(X >= k | p) >= alpha; P(X >= k) is increasing in p
        lower = bisect(lambda p: 1.0 - binom_cdf(k - 1, n, p) >= alpha, 0.0, 1.0)
    if k == n:
        upper = 1.0
    else:
        # largest p with P(X <= k | p) >= alpha; P(X <= k) is decreasing in p
        upper = bisect(lambda p: binom_cdf(k, n, p) < alpha, 0.0, 1.0)
    return lower, upper


def mcnemar_exact(b: int, c: int) -> float:
    """Exact two-sided McNemar p-value from the discordant counts b and c."""
    n = b + c
    if n == 0:
        return 1.0
    k = min(b, c)
    return min(1.0, 2.0 * binom_cdf(k, n, 0.5))


def holm(pvalues: list[float]) -> list[float]:
    """Holm step-down adjusted p-values, returned in the input order."""
    m = len(pvalues)
    order = sorted(range(m), key=lambda i: pvalues[i])
    adjusted = [0.0] * m
    running = 0.0
    for rank, i in enumerate(order):
        running = max(running, (m - rank) * pvalues[i])
        adjusted[i] = min(1.0, running)
    return adjusted
