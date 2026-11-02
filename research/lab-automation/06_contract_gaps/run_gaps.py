"""The properties the first five experiments left blank, each tried in every ecosystem.

Each scenario runs against a real gateway with that ecosystem's real adapter and simulator:

    GAP-01 unauthorized requests do not dispatch
    GAP-02 expired and revoked credentials authorize nothing new
    GAP-03 a stale observation is not presented as fresh, and cannot satisfy a check
    GAP-04 a repeated request follows idempotency
    GAP-05 a gateway restart does not repeat an uncertain command
    GAP-06 a lease keeps conflicting operations off the device
    GAP-07 evidence verifies independently, and an edit is detected
    GAP-08 a superseded write does not produce a misleading reconciliation
    GAP-09 an approval that runs out never runs late
    GAP-10 an adapter process lost mid-command: the outcome stays uncertain, the device comes back

A scenario that cannot be tried here records NOT_TESTED with the reason; nothing is inferred.

    OPENMHS_BIN=... [MM_DIR=...] <venv python> run_gaps.py --ecosystem pylabrobot --out results/...
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import platform
import signal
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path

HERE = Path(__file__).resolve()
sys.path[:0] = [str(HERE.parent), str(HERE.parents[1] / "shared")]

from ecosystems import ALL, Ecosystem  # noqa: E402
from harness import Check, Lab, binary, codes, reached_device, states, summary  # noqa: E402
from run_record import write_results  # noqa: E402

from openmhs.errors import GatewayError, OpenMHSError, UnauthorizedError  # noqa: E402

PASS, FAIL, NOT_TESTED = "PASS", "FAIL", "NOT_TESTED"


class NotTested(Exception):
    """This scenario cannot be tried in this ecosystem; the message says why."""


def submit(client, request: dict, *, key: str | None = None, deadline_ms: int | None = None, timeout: float = 60):
    body = dict(request)
    if key:
        body["idempotency_key"] = key
    if deadline_ms:
        body["deadline_ms"] = deadline_ms
    return client.call("commands.submit", {"request": body}, timeout=timeout)


def code_of(fn) -> str | None:
    try:
        fn()
    except GatewayError as e:
        return e.code
    except UnauthorizedError as e:
        return e.code
    return None


def gap01(eco: Ecosystem, lab: Lab, check: Check) -> dict:
    reader = lab.agent("agent:reader", [g for g in eco.grants if "read" in g["operations"]])
    before = eco.executed(lab)
    denied = submit(reader, eco.request(1))
    check("a credential without a grant is denied", denied["state"] == "DENIED", (denied["state"], codes(denied)))
    check("the denial names the policy", any(c.startswith("policy.") for c in codes(denied)), codes(denied))
    check("it never reached the device", not reached_device(denied), states(denied))
    check("the simulator did nothing", eco.executed(lab) == before, eco.executed(lab))
    return {"denied": summary(denied)}


def gap02(eco: Ecosystem, lab: Lab, check: Check) -> dict:
    agent = lab.agent("agent:gap", eco.grants)
    before = eco.executed(lab)
    created = lab.operator.create_token("agent:gap", any_scope=True)
    from openmhs import Client

    revoked = Client(lab.url, created.token)
    token_id = getattr(created, "id", None) or created.to_dict().get("id")
    lab.operator.revoke_token(token_id)
    code = code_of(lambda: submit(revoked, eco.request(2)))
    check("a revoked credential is refused", code in ("credential.revoked", "gateway.unauthorized"), code)

    expiring = lab.operator.create_token("agent:gap", any_scope=True, ttl_ms=200)
    short = Client(lab.url, expiring.token)
    time.sleep(0.5)
    code = code_of(lambda: submit(short, eco.request(3)))
    check("an expired credential is refused", code in ("credential.expired", "gateway.unauthorized"), code)
    check("neither reached the simulator", eco.executed(lab) == before, eco.executed(lab))
    still = submit(agent, eco.request(4))
    check("a valid credential still works", still["state"] == "VERIFIED", (still["state"], codes(still)))
    return {}


def gap03(eco: Ecosystem, lab: Lab, check: Check) -> dict:
    agent = lab.agent("agent:gap", eco.grants)
    fresh = agent.read(eco.device, eco.readable).to_dict()
    check("a current reading is usable", fresh["usable"] is True, fresh["usable"])
    lab.stale_flag.write_text("1")
    try:
        old = agent.read(eco.device, eco.readable).to_dict()
        check("an hour-old reading is reported old, and not usable",
              old["usable"] is False and old["age_ms"] > 3_000_000, (old["usable"], old["age_ms"]))
        before = eco.executed(lab)
        r = submit(agent, eco.request(5))
        ended = r["state"]
        # Either a precondition refuses it on the old reading, or it runs and its readback
        # cannot confirm it. What it must never be is VERIFIED on an old reading.
        check("a command cannot be confirmed by an old reading", ended != "VERIFIED", (ended, codes(r)))
        check("the receipt says why", ended in ("DENIED", "UNVERIFIABLE", "VERIFICATION_FAILED"), (ended, codes(r)))
        if ended == "DENIED":
            check("refused before dispatch, on the stale state", eco.executed(lab) == before
                  and any("stale" in c for c in codes(r)), codes(r))
    finally:
        lab.stale_flag.unlink(missing_ok=True)
    return {"old_reading": old, "command": summary(r)}


def gap04(eco: Ecosystem, lab: Lab, check: Check) -> dict:
    agent = lab.agent("agent:gap", eco.grants)
    before = eco.executed(lab)
    first = submit(agent, eco.request(6), key="gap-key")
    again = submit(agent, eco.request(6), key="gap-key")
    check("the same key returns the same command", again["command_id"] == first["command_id"]
          and first["state"] == "VERIFIED", (first["state"], again["command_id"] == first["command_id"]))
    code = code_of(lambda: submit(agent, eco.request(7), key="gap-key"))
    check("the same key on a different request is refused", code == "command.idempotency.conflict", code)
    if eco.idempotent:
        check("(no execution counter in this simulator: delivery counted by the conformance suite)", True)
    else:
        check("the simulator did it once", eco.executed(lab) == before + 1, eco.executed(lab) - before)
    return {"first": summary(first)}


def gap05(eco: Ecosystem, lab: Lab, check: Check) -> dict:
    agent = lab.agent("agent:gap", eco.grants)
    before = eco.executed(lab)
    eco.lost_answer(lab)
    lost = submit(agent, eco.request(8), key="gap-restart", deadline_ms=4000)
    check("the answer was lost: UNKNOWN_OUTCOME", lost["state"] == "UNKNOWN_OUTCOME", lost["state"])
    lab.kill()
    lab.start()
    after = lab.operator.call("commands.get", {"command_id": lost["command_id"]})
    check("after a hard restart it is still UNKNOWN_OUTCOME", after["state"] == "UNKNOWN_OUTCOME", after["state"])
    if not eco.idempotent:
        check("nothing was sent again", eco.executed(lab) == before + 1, eco.executed(lab) - before)
    agent2 = lab.agent("agent:gap", eco.grants)
    same = submit(agent2, eco.request(8), key="gap-restart", deadline_ms=4000)
    check("the same key after the restart returns the same command, unsent",
          same["command_id"] == lost["command_id"] and same["state"] == "UNKNOWN_OUTCOME", same["state"])
    rec = lab.operator.call("commands.reconcile", {"command_id": lost["command_id"]})
    check("reconciliation finds what the simulator holds: RECONCILED_APPLIED", rec["state"] == "RECONCILED_APPLIED",
          rec["state"])
    return {"lost": summary(lost), "after_restart": summary(after), "reconciled": summary(rec),
            "operation": eco.request(8),
            "dispatch_count": None if eco.idempotent else eco.executed(lab) - before}


def gap06(eco: Ecosystem, lab: Lab, check: Check) -> dict:
    holder = lab.agent("agent:holder", eco.grants)
    other = lab.agent("agent:other", eco.grants)
    before = eco.executed(lab)
    lease = holder.acquire_lease(60_000, devices=[eco.device])
    refused = submit(other, eco.request(9))
    check("another principal is refused while the lease is held",
          refused["state"] == "DENIED" and "lease.held" in codes(refused), (refused["state"], codes(refused)))
    check("the refused command never reached the device", not reached_device(refused), states(refused))
    check("nothing happened for it", eco.executed(lab) == before, eco.executed(lab) - before)
    ok = submit(holder, eco.request(10))
    check("the holder is carried out", ok["state"] == "VERIFIED", (ok["state"], codes(ok)))
    holder.release_lease(lease.id)
    after = submit(other, eco.request(11))
    check("once released, the other principal may go", after["state"] == "VERIFIED", (after["state"], codes(after)))
    return {"refused": summary(refused)}


def gap07(eco: Ecosystem, lab: Lab, check: Check) -> dict:
    agent = lab.agent("agent:gap", eco.grants)
    submit(agent, eco.request(12))
    denied = submit(agent, eco.invalid)
    check("an out-of-contract request is refused before dispatch", denied["state"] == "DENIED"
          and not reached_device(denied), (denied["state"], codes(denied)))
    out = lab.dir / "evidence.json"
    verified = lab.export_evidence(out)
    check("the export verifies with a separate process, without the gateway's home", verified["exit"] == 0, verified)
    doc = json.loads(out.read_text())
    edited = lab.dir / "evidence-edited.json"
    text = json.dumps(doc).replace('"VERIFIED"', '"FAILED"', 1)
    check("the edit changed the file", text != json.dumps(doc))
    edited.write_text(text)
    tampered = subprocess.run([str(binary()), "--json", "--home", str(lab.dir / "nowhere"), "audit", "verify-file",
                               str(edited), "--checkpoint", str(out.with_suffix(".checkpoint.json"))],
                              capture_output=True, text=True)
    check("an edited export fails verification", tampered.returncode != 0, tampered.returncode)
    return {"verify": {k: verified[k] for k in verified if k != "errors"}}


def gap08(eco: Ecosystem, lab: Lab, check: Check) -> dict:
    agent = lab.agent("agent:gap", eco.grants)
    eco.lost_answer(lab)
    lost = submit(agent, eco.request(13), deadline_ms=4000)
    check("the first write's answer was lost", lost["state"] == "UNKNOWN_OUTCOME", lost["state"])
    next_one = submit(agent, eco.request(14))
    if not eco.idempotent:
        check("a non-idempotent capability refuses the newer command instead: nothing can supersede it",
              next_one["state"] == "DENIED" and "command.outcome_unresolved" in codes(next_one),
              (next_one["state"], codes(next_one)))
        rec = lab.operator.call("commands.reconcile", {"command_id": lost["command_id"]})
        check("so reconciliation can still speak for it", rec["state"] == "RECONCILED_APPLIED", rec["state"])
        return {"newer": summary(next_one), "reconciled": summary(rec)}
    check("an idempotent capability lets the newer write through", next_one["state"] == "VERIFIED",
          (next_one["state"], codes(next_one)))
    code = code_of(lambda: lab.operator.call("commands.reconcile", {"command_id": lost["command_id"]}))
    check("reconciling the overtaken write refuses to conclude", code == "command.reconcile.superseded", code)
    still = lab.operator.call("commands.get", {"command_id": lost["command_id"]})
    check("it stays UNKNOWN_OUTCOME", still["state"] == "UNKNOWN_OUTCOME", still["state"])
    settled = lab.operator.call("commands.resolve", {"command_id": lost["command_id"], "applied": True,
                                                     "reason": "simulator state inspected by the test"})
    check("a person can settle it, on the record", settled["state"] == "RECONCILED_APPLIED", settled["state"])
    return {"lost": summary(lost), "newer": summary(next_one)}


def gap09(eco: Ecosystem, lab: Lab, check: Check) -> dict:
    held_grants = [g if "read" in g["operations"] else {**g, "effect": "require_approval"} for g in eco.grants]
    agent = lab.agent("agent:held", held_grants)
    before = eco.executed(lab)
    held = submit(agent, eco.request(15), deadline_ms=600)
    check("held for approval, not sent", held["state"] == "AWAITING_APPROVAL" and not reached_device(held),
          (held["state"], codes(held)))
    time.sleep(1.0)
    now = agent.call("commands.get", {"command_id": held["command_id"]})
    check("past its deadline it no longer says it is waiting", now["state"] == "EXPIRED", now["state"])
    code = code_of(lambda: lab.operator.call("commands.approve", {"command_id": held["command_id"]}))
    check("a late approval is refused", code == "command.invalid_state", code)
    check("nothing ran", eco.executed(lab) == before, eco.executed(lab) - before)
    return {"expired": summary(now)}


def gap10(eco: Ecosystem, lab: Lab, check: Check) -> dict:
    agent = lab.agent("agent:gap", eco.grants)
    pid = int(lab.stale_flag.with_suffix(".pid").read_text())
    eco.lost_answer(lab)
    before = eco.executed(lab)
    import threading

    out: dict = {}
    worker = threading.Thread(target=lambda: out.setdefault("r", submit(agent, eco.request(16), deadline_ms=20_000)))
    worker.start()
    time.sleep(1.0)  # applied, and the adapter is sitting on its answer
    if platform.system() == "Windows":
        subprocess.run(["taskkill", "/F", "/PID", str(pid)], capture_output=True)
    else:
        os.kill(pid, signal.SIGKILL)
    worker.join(60)
    r = out.get("r")
    check("the adapter process died mid-command: UNKNOWN_OUTCOME", r and r["state"] == "UNKNOWN_OUTCOME",
          r and (r["state"], states(r)))
    if not eco.idempotent:
        check("the adapter's restart did not send it again", eco.executed(lab) == before + 1,
              eco.executed(lab) - before)
    back, deadline = False, time.time() + 90
    while time.time() < deadline and not back:
        with contextlib.suppress(OpenMHSError):
            back = agent.read(eco.device, eco.readable).to_dict().get("usable") is True
        if not back:
            time.sleep(0.5)
    check("the device comes back without being registered again", back)
    rec = lab.operator.call("commands.reconcile", {"command_id": r["command_id"]})
    check("reconciled against the restarted adapter", rec["state"] in ("RECONCILED_APPLIED", "RECONCILED_NOT_APPLIED"),
          rec["state"])
    return {"lost": summary(r), "reconciled": summary(rec)}


SCENARIOS = {
    "GAP-01": ("unauthorized requests do not dispatch", gap01),
    "GAP-02": ("expired and revoked credentials authorize nothing new", gap02),
    "GAP-03": ("a stale observation is not presented as fresh", gap03),
    "GAP-04": ("a repeated request follows idempotency", gap04),
    "GAP-05": ("a gateway restart does not repeat an uncertain command", gap05),
    "GAP-06": ("a lease keeps conflicting operations off the device", gap06),
    "GAP-07": ("evidence verifies independently, and an edit is detected", gap07),
    "GAP-08": ("a superseded write does not reconcile misleadingly", gap08),
    "GAP-09": ("an approval that runs out never runs late", gap09),
    "GAP-10": ("an adapter lost mid-command: uncertain, then back", gap10),
}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--ecosystem", choices=sorted(ALL), required=True)
    ap.add_argument("--out", required=True, help="a results directory; each run gets its own file")
    ap.add_argument("--only", nargs="*")
    ap.add_argument("--work")
    a = ap.parse_args()
    eco = ALL[a.ecosystem]()
    root = Path(a.work or tempfile.mkdtemp(prefix=f"gaps-{a.ecosystem}-"))
    results = {}
    for sid, (title, fn) in SCENARIOS.items():
        if a.only and sid not in a.only:
            continue
        lab = eco.make_lab(root / f"{sid}-{uuid.uuid4().hex[:6]}")
        check = Check()
        t = time.perf_counter()
        outcome, detail, why = None, {}, None
        try:
            lab.sticky_port = True
            lab.start()
            detail = fn(eco, lab, check)
            audit = lab.operator.audit_verify().to_dict()
            check("audit chain verifies", audit["valid"], audit["records"])
        except NotTested as e:
            outcome, why = NOT_TESTED, str(e)
        except Exception as e:  # report, do not hide
            import traceback

            detail = {"exception": f"{type(e).__name__}: {e}", "traceback": traceback.format_exc()[-1800:]}
            check("scenario ran to the end", False, detail["exception"])
        finally:
            with contextlib.suppress(Exception):
                lab.kill()
            kill_all = getattr(lab, "kill_all", None)
            if kill_all:
                with contextlib.suppress(Exception):
                    kill_all()
        outcome = outcome or (PASS if check.passed else FAIL)
        results[sid] = {"title": title, "outcome": outcome, "seconds": round(time.perf_counter() - t, 1),
                        "checks": check.items, "detail": detail, **({"not_tested_because": why} if why else {})}
        print(f"{outcome:10s} {a.ecosystem} {sid} {title} ({results[sid]['seconds']} s)", flush=True)
        for item in check.items:
            if not item["ok"]:
                print(f"     failed: {item['check']}: {str(item['observed'])[:300]}", flush=True)
    path = write_results(Path(a.out), f"gaps-{a.ecosystem}", {"ecosystem": a.ecosystem}, results)
    print(f"wrote {path}")


if __name__ == "__main__":
    main()
