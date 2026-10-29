"""Phase 8: a repeatable fault-injection benchmark. Direct LabThings clients vs the same
work through OpenMHS, against the same independent LabThings server, under the same faults.

    python reliability_benchmark.py [--reps 5] [--only F1 F4] > ../results/reliability_benchmark.json

Arms (the client logic is the variable; the server, faults and ground truth are shared):

  direct-naive    plain HTTP to the LabThings server. If an attempt fails or cannot be confirmed,
                  try again (up to 3 attempts). What a first script, or an agent told to "be
                  persistent", does.
  direct-careful  plain HTTP, written by someone who has read this repository's findings: never
                  repeats a possibly-executed non-idempotent action; after an ambiguous failure it
                  waits for the bench to go idle and reads the position back; says "unknown"
                  rather than guessing.
  mhs-same-key    through OpenMHS, the naive retry loop reusing one idempotency key.
  mhs-new-key     through OpenMHS, the naive retry loop with a fresh key per attempt (the worst
                  agent: it does not know the retry is a repeat).
  mhs-reconcile   through OpenMHS, with a client that heeds the receipt: it does not resend work
                  that failed part-way or ran unconfirmed, and when an outcome is unresolved an
                  operator waits for the work to finish and asks OpenMHS to reconcile it by
                  readback (applied is a success, not-applied is safe to send again). Run
                  separately with ``--arms mhs-reconcile`` and merged.

A scenario is one fault on one intended command ("move to target T"). Ground truth is read from the
LabThings server directly: how many times the work started, where the stage is. This is a
simulated-fault benchmark on one server and one action; it is not a survey.
"""

from __future__ import annotations

import argparse
import inspect
import json
import shutil
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

import export_report
import lab
from lab import FaultProxy, Gateway, direct_move, http_json, make_manifest, settle, truth

DEVICE, CAP = "lab.bench", "move_to"
TARGET = 100
SECONDS = 1.5

# name: (description, proxy rules [(match, action, times)], request overrides, gateway options)
SCENARIOS = {
    "F0": ("no fault", [], {}, {}),
    "F1": ("the response to the request is lost; the server acted",
           [(r"POST /bench/move_to", "drop_response", 1)], {}, {}),
    "F2": ("the connection is reset before the server sees the request",
           [(r"POST /bench/move_to", "reset", 1)], {}, {}),
    "F3": ("the server answers 503 and did not act",
           [(r"POST /bench/move_to", "status:503", 1)], {}, {}),
    "F4": ("the request is accepted; every status poll is then lost",
           [(r"GET /action_invocations/", "reset", 99)], {}, {}),
    "F5": ("the action fails part-way (a stalled motor)", [], {"fail_after": 0.4}, {}),
    "F6": ("a cache serves the position as it was before the move",
           [(r"GET /bench/position", "freeze", None)], {}, {"stamp": "date_header", "skew_ms": 1000}),
    "F6b": ("as F6, but the cached position is 2.5 s old (outside the 1 s blind spot of the Date-header stamp)",
            [(r"GET /bench/position", "freeze", None)], {},
            {"stamp": "date_header", "skew_ms": 1000, "pre_sleep": 2.5}),
    "F7": ("a target outside the declared limit (+/-1000)", [], {"target": 5000}, {}),
}

CLAIMS = ("success", "failure", "unknown")


# ------------------------------------------------------------------ direct arms

def _post(base: str, target: int, fail_after: float, timeout: float) -> tuple[str, dict | None]:
    """'ok' with the invocation, 'unexecuted' (a definite refusal), or 'ambiguous' (it may have run)."""
    body = json.dumps({"target": target, "seconds": SECONDS, "fail_after": fail_after}).encode()
    req = urllib.request.Request(f"{base}/bench/move_to", data=body, method="POST",
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:  # noqa: S310
            return "ok", json.loads(r.read())
    except urllib.error.HTTPError as e:
        return ("unexecuted" if e.code in (502, 503) else "ambiguous"), None
    except Exception:
        return "ambiguous", None


def _follow(base: str, inv: dict, timeout: float) -> str:
    """The invocation's terminal status, or 'lost'."""
    end = time.time() + timeout
    while time.time() < end:
        if inv.get("status") in ("completed", "error", "cancelled"):
            return inv["status"]
        try:
            _, inv = http_json("GET", f"{base}/action_invocations/{inv['id']}", timeout=2)
        except Exception:
            return "lost"
        time.sleep(0.15)
    return "lost"


def direct_naive(base: str, target: int, fail_after: float) -> str:
    for _ in range(3):
        kind, inv = _post(base, target, fail_after, timeout=3)
        if kind == "ok" and _follow(base, inv, 6) == "completed":
            return "success"
    return "failure"


def direct_careful(base: str, target: int, fail_after: float) -> str:
    for _ in range(3):
        kind, inv = _post(base, target, fail_after, timeout=3)
        if kind == "unexecuted":
            continue  # a definite refusal: nothing ran, so trying again is safe
        if kind == "ok":
            status = _follow(base, inv, 6)
            if status == "completed":
                return "success"
            if status in ("error", "cancelled"):
                return "failure"
        # it may have run: never send it again. Wait for the bench to settle, then look.
        for _ in range(40):
            try:
                _, busy = http_json("GET", f"{base}/bench/busy", timeout=2)
                if busy is False:
                    break
            except Exception:
                pass
            time.sleep(0.2)
        try:
            _, pos = http_json("GET", f"{base}/bench/position", timeout=2)
        except Exception:
            return "unknown"
        return "success" if pos == target else "unknown"
    return "failure"


# ------------------------------------------------------------------ OpenMHS arms

def mhs_loop(agent, target: int, fail_after: float, same_key: bool, tag: str, operator=None) -> str:
    saw_unknown = False
    for i in range(3):
        key = f"{tag}-k" if same_key else f"{tag}-k{i}"
        try:
            r = agent.invoke(DEVICE, CAP, {"target": target, "seconds": {"value": SECONDS, "unit": "s"},
                                           "fail_after": {"value": fail_after, "unit": "s"}},
                             idempotency_key=key)
        except Exception:
            saw_unknown = True
            continue
        outcome = str(getattr(r.outcome, "value", r.outcome)).lower()
        if outcome == "succeeded":
            return "success"
        if outcome in ("denied", "rejected"):
            return "failure" if not saw_unknown else "unknown"
        if outcome == "uncertain" and operator is not None:
            time.sleep(SECONDS + 1.5)  # the operator lets the work finish before asking
            try:
                rec = operator.reconcile(r.command_id)
                state = str(getattr(rec.state, "value", rec.state))
            except Exception:
                state = "ERROR"
            if state == "RECONCILED_APPLIED":
                return "success"
            if state == "RECONCILED_NOT_APPLIED":
                continue  # it did not happen: sending it again is safe
            saw_unknown = True
            continue
        if operator is not None and outcome in ("unverified", "failed"):
            # a client that heeds the receipt does not resend work the device may have done:
            # "unverified" ran but was not confirmed; "failed" may have stopped part-way
            return "unknown" if outcome == "unverified" else "failure"
        if outcome in ("uncertain", "unverified", "pending"):
            saw_unknown = True
    return "unknown" if saw_unknown else "failure"


# ------------------------------------------------------------------ ground truth and scoring

def classify(before: dict, after: dict, target: int) -> str:
    """What actually happened on the server: not_run, done, or ran_but_wrong."""
    started = after["jobs_started"] - before["jobs_started"]
    if started == 0:
        return "not_run"
    return "done" if after["position"] == target else "ran_but_wrong"


def score(claim: str, kind: str, started: int) -> dict:
    return {
        "claim": claim, "truth": kind, "executions": started,
        "duplicate": started > 1,
        "false_success": claim == "success" and kind != "done",
        "false_failure": claim == "failure" and kind == "done",
        "honest_unknown": claim == "unknown",
        "correct_definitive": (claim == "success" and kind == "done")
                              or (claim == "failure" and kind in ("not_run", "ran_but_wrong")),
        "completed": claim == "success" and kind == "done",
    }


def run_one(arm: str, sid: str, rep: int) -> dict:
    _, rules, req, opts = SCENARIOS[sid]
    target, fail_after = req.get("target", TARGET), req.get("fail_after", -1.0)
    settle()
    direct_move(0)
    time.sleep(0.2)
    proxy = FaultProxy()
    gw = None
    try:
        before = truth()
        if arm.startswith("mhs"):
            d = lab.WORK / f"bench-{arm}-{sid}"
            manifest = make_manifest(d, proxy.base + "/", timeout_ms=3000, stamp=opts.get("stamp", "received"))
            (d / "invocations.jsonl").unlink(missing_ok=True)
            home = lab.WORK / f"bench-{arm}-{sid}-home"
            if home.exists():
                shutil.rmtree(home, ignore_errors=True)
            gw = Gateway(home, manifest, skew_ms=opts.get("skew_ms", 0)).start()
        for m, a, n in rules:
            proxy.add(m, a, n)
        time.sleep(opts.get("pre_sleep", 0))  # age any snapshot the rules just took
        t0 = time.time()
        if arm == "direct-naive":
            claim = direct_naive(proxy.base, target, fail_after)
        elif arm == "direct-careful":
            claim = direct_careful(proxy.base, target, fail_after)
        else:
            claim = mhs_loop(gw.agent, target, fail_after, arm == "mhs-same-key", f"{sid}-{rep}",
                             operator=gw.op if arm == "mhs-reconcile" else None)
        elapsed = time.time() - t0
        proxy.clear()
        time.sleep(SECONDS * 2 + 0.5)  # let anything still in flight finish before looking
        settle(timeout=15)
        after = truth()
        res = score(claim, classify(before, after, target), after["jobs_started"] - before["jobs_started"])
        res["seconds"] = round(elapsed, 2)
        if gw:
            audit = gw.op.audit_verify()
            rep_ = export_report.build(gw.op, DEVICE)
            res["audit_chain_valid"] = audit.valid
            res["commands_recorded"] = len(rep_["commands"])
            res["audit_complete"] = all(
                c["principal"] and c["authorization"]["decision"] and c["lifecycle"] for c in rep_["commands"]) \
                and bool(rep_["commands"])
            res["with_device_reference"] = sum(bool(c["device_reference"]) for c in rep_["commands"])
        return res
    finally:
        if gw:
            gw.stop()
        proxy.stop()


def overhead(n: int = 20) -> dict:
    out = {}
    settle()
    direct_move(0)
    d: list[float] = []
    for i in range(n):
        t = time.time()
        direct_naive(lab.BENCH, 10 + (i % 2) * 10, -1.0)
        d.append(time.time() - t)
    out["direct_s"] = _stats(d)
    d = lab.WORK / "bench-overhead"
    manifest = make_manifest(d, lab.BENCH + "/", timeout_ms=5000)
    home = lab.WORK / "bench-overhead-home"
    if home.exists():
        shutil.rmtree(home, ignore_errors=True)
    gw = Gateway(home, manifest).start()
    try:
        m: list[float] = []
        for i in range(n):
            t = time.time()
            mhs_loop(gw.agent, 10 + (i % 2) * 10, -1.0, True, f"oh{i}")
            m.append(time.time() - t)
        out["openmhs_s"] = _stats(m)
    finally:
        gw.stop()
    out["note"] = ("includes the action's own 1.5 s runtime in both; the difference is what the gateway adds "
                   "(validation, authorization, journal writes, readback verification)")
    return out


def _stats(v: list[float]) -> dict:
    v = sorted(v)
    return {"n": len(v), "median": round(statistics.median(v), 3), "p95": round(v[int(0.95 * (len(v) - 1))], 3)}


def effort() -> dict:
    loc = lambda f: len([l for l in inspect.getsource(f).splitlines() if l.strip() and not l.strip().startswith("#")])  # noqa: E731
    return {"direct_naive_lines": loc(direct_naive) + loc(_post) + loc(_follow),
            "direct_careful_lines": loc(direct_careful) + loc(_post) + loc(_follow),
            "openmhs_client_lines": loc(mhs_loop),
            "openmhs_onboarding": "one import command; judgement entries in the override file (see WOT_IMPORTER_RESULTS.md)",
            "note": "lines of Python in this file, excluding comments; a rough size, not a quality measure"}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--reps", type=int, default=5)
    ap.add_argument("--only", nargs="*")
    ap.add_argument("--arms", nargs="*", default=["direct-naive", "direct-careful", "mhs-same-key", "mhs-new-key"])
    ap.add_argument("--skip-overhead", action="store_true")
    a = ap.parse_args()
    out: dict = {"_meta": {"reps": a.reps, "when": time.strftime("%FT%T%z"), "target": TARGET, "action_seconds": SECONDS,
                           "note": "simulated faults, one LabThings server, one action; ground truth read from the server"},
                 "scenarios": {k: v[0] for k, v in SCENARIOS.items()}, "runs": {}}
    for sid in a.only or list(SCENARIOS):
        for arm in a.arms:
            rows = []
            for rep in range(a.reps):
                try:
                    rows.append(run_one(arm, sid, rep))
                except Exception as e:
                    rows.append({"harness_error": f"{type(e).__name__}: {str(e)[:200]}"})
            out["runs"][f"{sid}/{arm}"] = rows
            ok = [r for r in rows if "claim" in r]
            print(f"[{sid}/{arm}] " + ", ".join(f"{c}={sum(r['claim'] == c for r in ok)}" for c in CLAIMS)
                  + f", duplicates={sum(r['duplicate'] for r in ok)}, false_success={sum(r['false_success'] for r in ok)}"
                  + f", errors={len(rows) - len(ok)}", file=sys.stderr, flush=True)
    if not a.skip_overhead:
        out["overhead"] = overhead()
    out["effort"] = effort()
    print(json.dumps(out, indent=1, default=str))


if __name__ == "__main__":
    main()
