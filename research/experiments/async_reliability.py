"""Phase 5: LabThings asynchronous actions under failure, observed through OpenMHS.

Every scenario starts a fresh OpenMHS gateway (fresh journal) serving the independent
`LabBench` Thing through the generated adapter and a fault proxy, runs one situation, and
records what OpenMHS said next to what the LabThings server says (ground truth, read directly).

    python async_reliability.py [S05 S06 ...] > ../results/async_reliability.json

Simulators only. Nothing here touches hardware.
"""

from __future__ import annotations

import json
import subprocess
import sys
import threading
import time
from pathlib import Path

import lab
from lab import FaultProxy, Gateway, direct_move, http_json, make_manifest, settle, truth

DEVICE, CAP = "lab.bench", "move_to"
RESULTS: dict[str, dict] = {}


def params(target: int, seconds: float = 1.0, fail_after: float = -1.0) -> dict:
    return {"target": target, "seconds": {"value": seconds, "unit": "s"},
            "fail_after": {"value": fail_after, "unit": "s"}}


class Scn:
    """One isolated scenario."""

    def __init__(self, name: str, *, adapter_timeout_ms: int = 1000, overrides=None, skew_ms: int = 0,
                 stamp: str = "received") -> None:
        self.name, self.t_ms, self.overrides, self.skew = name, adapter_timeout_ms, overrides, skew_ms
        self.stamp = stamp

    def __enter__(self) -> "Scn":
        settle()
        direct_move(0)
        self.base = truth()
        self.proxy = FaultProxy()
        self.dir = lab.WORK / f"exp-{self.name}"
        self.manifest = make_manifest(self.dir, self.proxy.base + "/", timeout_ms=self.t_ms,
                                      overrides=self.overrides, stamp=self.stamp)
        self.home = lab.WORK / f"exp-{self.name}-home"
        if self.home.exists():
            subprocess.run(["powershell", "-NoProfile", "-Command", f"Remove-Item -Recurse -Force '{self.home}'"],
                           capture_output=True)
        (self.dir / "invocations.jsonl").unlink(missing_ok=True)
        self.gw = Gateway(self.home, self.manifest, skew_ms=self.skew).start()
        return self

    def __exit__(self, *exc) -> None:
        self.gw.stop()
        self.proxy.stop()

    # ---- actions
    def invoke(self, target: int, seconds: float = 1.0, key: str | None = None, fail_after: float = -1.0,
               deadline_ms: int | None = None, client=None):
        opts = {"idempotency_key": key or f"{self.name}-{time.monotonic_ns()}"}
        if deadline_ms:
            opts["deadline_ms"] = deadline_ms
        return (client or self.gw.agent).invoke(DEVICE, CAP, params(target, seconds, fail_after), **opts)

    def background(self, **kw):
        out: dict = {}

        def run() -> None:
            try:
                out["receipt"] = self.invoke(**kw)
            except Exception as e:  # the client may lose the gateway (restart scenarios)
                out["error"] = f"{type(e).__name__}: {e}"

        t = threading.Thread(target=run, daemon=True)
        t.start()
        return t, out

    def latest(self):
        rs = self.gw.agent.commands(limit=20)
        return rs[0] if rs else None

    def jobs_delta(self) -> int:
        return truth()["jobs_started"] - self.base["jobs_started"]

    def ledger(self) -> list[dict]:
        p = self.dir / "invocations.jsonl"
        return [json.loads(l) for l in p.read_text().splitlines()] if p.exists() else []


def summarize(r) -> dict:
    last = r.transitions[-1]
    return {
        "command_id": r.command_id,
        "external_ref": getattr(r.adapter_result, "external_ref", None) if r.adapter_result else None,
        "state": r.state.value,
        "outcome": r.outcome.value,
        "path": [t.state.value for t in r.transitions],
        "last_detail": (last.detail or "")[:300],
        "errors": [{"code": e.code, "message": e.message[:300]} for e in r.policy.errors],
        "verification": (None if not r.verification else
                         {"matched": r.verification.matched, "reason": r.verification.reason,
                          "observed": getattr(getattr(r.verification, "observed", None), "value", None)}),
    }


def record(sid: str, title: str, **fields) -> None:
    RESULTS[sid] = {"title": title, **fields}
    print(f"[{sid}] {title}", file=sys.stderr)


# ======================================================================= scenarios

def s01() -> None:
    """Invocation accepted but not yet completed."""
    with Scn("s01") as s:
        t, out = s.background(target=40, seconds=3.0)
        seen, first_applied_inv = [], None
        t0 = time.time()
        while t.is_alive() and time.time() - t0 < 20:
            c = s.latest()
            if c and (not seen or seen[-1][0] != c.state.value):
                inv = [i["status"] for i in truth()["invocations"] if i["action"] == CAP][-1:]
                seen.append((c.state.value, round(time.time() - t0, 2), inv[0] if inv else None,
                             truth()["position"]))
            time.sleep(0.12)
        t.join()
        r = out["receipt"]
        record("S01", "accepted but not yet completed",
               openmhs_states_seen=seen, final=summarize(r), truth_after=truth(),
               premature_success=any(st in ("APPLIED", "VERIFIED") and inv in ("running", "pending")
                                     for st, _, inv, _ in seen),
               jobs_delta=s.jobs_delta())


def s03() -> None:
    """The action raised an error part-way (a stalled motor)."""
    with Scn("s03") as s:
        r = s.invoke(60, seconds=2.0, fail_after=0.5)
        settle()
        tr = truth()
        # Can the next command go straight through? (An error must not leave a hold.)
        r2 = s.invoke(5, seconds=0.3)
        record("S03", "action error part-way", final=summarize(r), truth_after=tr,
               position_reached_target=(tr["position"] == 60), followup=summarize(r2),
               adapter_ledger=[e["event"] for e in s.ledger()], jobs_delta=s.jobs_delta())


def s04() -> None:
    """Cancelled by someone else (not OpenMHS) while OpenMHS is waiting."""
    with Scn("s04") as s:
        t, out = s.background(target=80, seconds=4.0)
        time.sleep(1.0)
        inv = [i for i in truth()["invocations"] if i["action"] == CAP][-1]
        code, _ = http_json("DELETE", f"{lab.BENCH}/action_invocations/{inv['id']}")
        t.join(timeout=20)
        settle()
        tr = truth()
        record("S04", "cancelled externally", cancel_http=code, final=summarize(out["receipt"]), truth_after=tr,
               stopped_partway=(0 < tr["position"] < 80), jobs_delta=s.jobs_delta())


def s05() -> None:
    """The caller's deadline passes before the action finishes."""
    with Scn("s05") as s:
        r = s.invoke(70, seconds=4.0, deadline_ms=1500)
        mid = truth()
        second = s.invoke(10, seconds=0.2, key="s05-second")
        settle()
        end = truth()
        rec = s.gw.agent.reconcile(r.command_id)
        ledger = [e for e in s.ledger() if e.get("invocation_id")]
        record("S05", "caller deadline passes before completion",
               first=summarize(r), truth_at_unknown=mid, followup_while_unresolved=summarize(second),
               truth_after=end, reconcile=summarize(rec),
               invocation_id_in_error=any(e.get("invocation_id", "") and e["invocation_id"] in
                                          json.dumps(summarize(r)) for e in ledger),
               invocation_id_in_adapter_ledger=bool(ledger), jobs_delta=s.jobs_delta())


def s06() -> None:
    """Connection to the status endpoint fails after the invocation was accepted."""
    with Scn("s06") as s:
        s.proxy.add(r"GET /action_invocations/", "reset")
        r = s.invoke(55, seconds=2.0)
        s.proxy.clear()
        settle()
        end = truth()
        rec = s.gw.agent.reconcile(r.command_id)
        record("S06", "status polling lost after acceptance", first=summarize(r), truth_after=end,
               reconcile=summarize(rec), adapter_ledger=[e["event"] for e in s.ledger()],
               jobs_delta=s.jobs_delta())


def s07() -> None:
    """The server no longer knows the invocation (expired or restarted)."""
    with Scn("s07") as s:
        s.proxy.add(r"GET /action_invocations/", "status:404")
        r = s.invoke(45, seconds=1.0)
        s.proxy.clear()
        settle()
        end = truth()
        rec = s.gw.agent.reconcile(r.command_id)
        record("S07", "invocation unknown to the server (404)", first=summarize(r), truth_after=end,
               reconcile=summarize(rec), jobs_delta=s.jobs_delta())


def s08() -> None:
    """Status answers, but later than the adapter will wait."""
    with Scn("s08", adapter_timeout_ms=800) as s:
        s.proxy.add(r"GET /action_invocations/", "delay:2.5")
        r = s.invoke(35, seconds=1.0)
        s.proxy.clear()
        settle()
        end = truth()
        rec = s.gw.agent.reconcile(r.command_id)
        record("S08", "status returns after the client gave up", first=summarize(r), truth_after=end,
               reconcile=summarize(rec), job_completed_normally=(end["position"] == 35),
               jobs_delta=s.jobs_delta())


def s09() -> None:
    """The adapter process dies while an action is outstanding."""
    with Scn("s09") as s:
        t, out = s.background(target=75, seconds=4.0)
        time.sleep(1.2)
        cmd = ("(Get-CimInstance Win32_Process | Where-Object { $_.ParentProcessId -eq %d -and "
               "$_.CommandLine -like '*wot_adapter*' }).ProcessId" % s.gw.proc.pid)
        pids = subprocess.run(["powershell", "-NoProfile", "-Command", cmd],
                              capture_output=True, text=True).stdout.split()
        for pid in pids:
            subprocess.run(["taskkill", "/F", "/PID", pid], capture_output=True)
        t.join(timeout=20)
        settle()
        end = truth()
        time.sleep(2.0)
        status = [d.availability.value for d in s.gw.agent.device_status()]
        rec = s.gw.agent.reconcile(out["receipt"].command_id) if "receipt" in out else None
        record("S09", "adapter crash during an outstanding action", killed=pids,
               first=summarize(out["receipt"]) if "receipt" in out else out, truth_after=end,
               device_status_after=status, reconcile=summarize(rec) if rec else None,
               adapter_ledger=[e["event"] for e in s.ledger()], jobs_delta=s.jobs_delta())


def s10() -> None:
    """The gateway is killed while an action is outstanding, then restarted."""
    with Scn("s10") as s:
        t, out = s.background(target=65, seconds=4.0)
        time.sleep(1.2)
        s.gw.kill()
        t.join(timeout=10)
        settle()
        end = truth()
        s.gw.start()
        time.sleep(1.5)
        listed = [summarize(r) for r in s.gw.agent.commands(limit=5)]
        cid = listed[0]["command_id"] if listed else None
        rec = summarize(s.gw.agent.reconcile(cid)) if cid and listed[0]["state"] == "UNKNOWN_OUTCOME" else None
        again = s.invoke(5, seconds=0.2, key="s10-after")
        record("S10", "gateway restart with an unresolved action", client_saw=out.get("error", "completed"),
               truth_after_kill=end, after_restart=listed, reconcile=rec, next_command=summarize(again),
               jobs_delta_before_next=end["jobs_started"] - s.base["jobs_started"],
               jobs_delta=s.jobs_delta())


def s11() -> None:
    """Duplicate requests: same key, different key during an unresolved outcome, and direct."""
    with Scn("s11") as s:
        a = threading.Thread(target=lambda: s.invoke(30, seconds=1.5, key="dup"), daemon=True)
        a.start()
        time.sleep(0.3)
        same_key = s.invoke(30, seconds=1.5, key="dup")
        a.join()
        settle()
        same_key_jobs = s.jobs_delta()
        # A different key while an outcome is unresolved.
        before = truth()["jobs_started"]
        first = s.invoke(70, seconds=3.0, deadline_ms=1200, key="dup-first")
        blocked = s.invoke(70, seconds=3.0, key="dup-second")
        settle()
        blocked_jobs = truth()["jobs_started"] - before
        # Baseline: two direct POSTs.
        direct_before = truth()["jobs_started"]
        for _ in range(2):
            http_json("POST", f"{lab.BENCH}/bench/move_to", {"target": 20, "seconds": 0.3})
        settle()
        record("S11", "duplicate requests",
               same_key={"state": same_key.state.value, "jobs_started": same_key_jobs},
               different_key_while_unresolved={"first": summarize(first), "second": summarize(blocked),
                                                "jobs_started_by_both": blocked_jobs},
               direct_labthings_two_posts={"jobs_started": truth()["jobs_started"] - direct_before})


def s12() -> None:
    """A readback older than the action (a caching intermediary serves the old position)."""
    for name, stamp, skew in (("s12", "received", 0), ("s12-date", "date_header", 1000)):
        with Scn(name, stamp=stamp, skew_ms=skew) as s:
            s.proxy.add(r"GET /bench/position", "freeze")
            time.sleep(2.5)
            r = s.invoke(50, seconds=0.5)
            settle()
            end = truth()
            s.proxy.clear()
            record("S12" if stamp == "received" else "S12-date",
                   f"stale readback served by an intermediary (stamp={stamp}, readback_skew_ms={skew})",
                   first=summarize(r), truth_after=end,
                   openmhs_blamed_device=(r.state.value == "VERIFICATION_FAILED" and end["position"] == 50),
                   jobs_delta=s.jobs_delta())


def s04b() -> None:
    """Cancelling an action that ignores cancellation."""
    with Scn("s04b") as s:
        _, inv = http_json("POST", f"{lab.BENCH}/bench/move_uncancellable", {"target": 60, "seconds": 3.0})
        time.sleep(0.8)
        code, _ = http_json("DELETE", f"{lab.BENCH}/action_invocations/{inv['id']}")
        after_cancel = [truth()["position"], http_json("GET", inv["href"])[1]["status"]]
        time.sleep(0.5)
        later = [truth()["position"], http_json("GET", inv["href"])[1]["status"]]
        settle()
        end = truth()
        record("S04b", "cancel request on an action that ignores it (direct LabThings)", cancel_http=code,
               just_after_cancel_position_and_status=after_cancel, half_second_later=later, truth_after=end,
               still_moved_after_cancel=(later[0] > after_cancel[0] or end["position"] == 60))


def s12b() -> None:
    """A stale reading used to reconcile an unresolved action (the dangerous case)."""
    for name, stamp, skew in (("s12b", "received", 0), ("s12b-date", "date_header", 1000)):
        with Scn(name, stamp=stamp, skew_ms=skew) as s:
            s.proxy.add(r"GET /bench/position", "freeze")
            time.sleep(2.5)  # the cached response is now 2.5 s old, beyond the 1 s allowance
            r = s.invoke(50, seconds=2.0, deadline_ms=1000)
            settle()
            end = truth()
            try:
                rec = summarize(s.gw.agent.reconcile(r.command_id))
            except Exception as e:
                rec = {"refused": f"{type(e).__name__}: {e}"[:300]}
            record("S12b" if stamp == "received" else "S12b-date",
                   f"stale reading used to reconcile (stamp={stamp}, readback_skew_ms={skew})",
                   first=summarize(r), truth_after=end, reconcile=rec,
                   dangerous=(isinstance(rec, dict) and rec.get("state") == "RECONCILED_NOT_APPLIED"
                              and end["position"] == 50),
                   jobs_delta=s.jobs_delta())


def s12c() -> None:
    """The Date-header stamp on a normal (uncached) verified move, with a 1 s allowance."""
    with Scn("s12c", stamp="date_header", skew_ms=1000) as s:
        r = s.invoke(50, seconds=0.5)
        record("S12c", "Date-header stamping, uncached, readback_skew_ms=1000", first=summarize(r),
               truth_after=truth(), jobs_delta=s.jobs_delta())


ALL = {"S01": s01, "S03": s03, "S04": s04, "S05": s05, "S06": s06, "S07": s07, "S08": s08,
       "S09": s09, "S10": s10, "S11": s11, "S12": s12,
       "S04b": s04b, "S12b": s12b, "S12c": s12c}

if __name__ == "__main__":
    wanted = sys.argv[1:] or list(ALL)
    for sid in wanted:
        try:
            ALL[sid]()
        except Exception as e:
            import traceback
            RESULTS[sid] = {"title": ALL[sid].__doc__, "harness_error": f"{type(e).__name__}: {e}",
                            "trace": traceback.format_exc()[-600:]}
            print(f"[{sid}] HARNESS ERROR {e}", file=sys.stderr)
    meta = {"openmhs_commit": __import__("subprocess").run(["git", "rev-parse", "--short", "HEAD"], capture_output=True,
                                                              text=True, cwd=str(lab.REPO)).stdout.strip(),
            "labthings_fastapi": "0.3.0 (761441e)", "when": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "note": "simulators only; ground truth read directly from the LabThings server"}
    print(json.dumps({"_meta": meta, **RESULTS}, indent=1, default=str))
