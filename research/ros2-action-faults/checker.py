"""History checker with indeterminate operations.

Input: the client-visible history (invoke / ok / fail / timeout events per logical operation)
and the effect log (the plant's record of effect events). Output: typed violations.

V1 duplicate effect    two or more effects for one non-idempotent operation
V2 phantom effect      an effect for an operation that was never invoked
V3 false confirmation  the client reported success and no effect occurred
V4 false denial        the client reported failure and an effect occurred

A `timeout` is indeterminate: the operation may or may not have taken effect, so no violation
is raised for it. In Layer 0 the checker is handed the true effect log, so its agreement with the
metric code is a consistency check between two implementations. It is not an estimate of how a
checker would perform on noisy real logs.
"""

from __future__ import annotations

from collections import Counter


def check(history: list[dict], effects: list[tuple[float, str]], idempotent: bool) -> list[dict]:
    invoked = {e["op"] for e in history if e["event"] == "invoke"}
    final: dict[str, str] = {}
    for e in history:
        if e["event"] in ("ok", "fail", "timeout"):
            final[e["op"]] = e["event"]
    counts = Counter(op for _, op in effects)
    violations: list[dict] = []
    for op, n in counts.items():
        if op not in invoked:
            violations.append({"op": op, "type": "V2", "effects": n})
        elif n >= 2 and not idempotent:
            violations.append({"op": op, "type": "V1", "effects": n})
    for op in invoked:
        n = counts.get(op, 0)
        outcome = final.get(op)
        if outcome == "ok" and n == 0:
            violations.append({"op": op, "type": "V3", "effects": n})
        if outcome == "fail" and n >= 1:
            violations.append({"op": op, "type": "V4", "effects": n})
    return violations
