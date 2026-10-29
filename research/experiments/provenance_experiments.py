"""Phase 6: what the audit trail can and cannot show afterwards.

Every experiment starts a fresh gateway serving the independent LabBench Thing, runs real
commands, then damages the journal the way a careless or hostile local actor could, and records
what OpenMHS's own verification and the exported report say. Simulators only.

    python provenance_experiments.py [P1 P2 ...] > ../results/provenance.json
    python provenance_experiments.py P8        # waits past LabThings' 300 s retention (slow)

What is being asked: the hash chain is meant to make the audit trail tamper-*evident*. For each
way of damaging the journal: is it noticed, by what, and what does it take to get away with it?
"""

from __future__ import annotations

import hashlib
import json
import shutil
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

import export_report
import lab
from lab import Gateway, direct_move, http_json, make_manifest, settle, truth

DEVICE, CAP = "lab.bench", "move_to"
OUT: dict[str, dict] = {}


def params(target: int, seconds: float = 0.2) -> dict:
    return {"target": target, "seconds": {"value": seconds, "unit": "s"}, "fail_after": {"value": -1.0, "unit": "s"}}


def fresh(name: str, **gw) -> Gateway:
    settle()
    direct_move(0)
    d = lab.WORK / f"prov-{name}"
    manifest = make_manifest(d, lab.BENCH + "/", timeout_ms=3000)
    home = lab.WORK / f"prov-{name}-home"
    if home.exists():
        shutil.rmtree(home, ignore_errors=True)
    (d / "invocations.jsonl").unlink(missing_ok=True)
    return Gateway(home, manifest, **gw).start()


def run_some(gw: Gateway, n: int = 3, tag: str = "c") -> list:
    return [gw.agent.invoke(DEVICE, CAP, params(10 * (i + 1)), idempotency_key=f"{tag}-{i}") for i in range(n)]


def journal(gw: Gateway) -> Path:
    return gw.home / "journal.sqlite"


def verify(home: Path, manifest: Path) -> dict:
    """Restart the gateway on a (possibly damaged) journal and ask it to verify its own chain."""
    gw = Gateway(home, manifest, grants=False).start()
    try:
        v = gw.op.audit_verify()
        return {"records": v.records, "valid": v.valid, "first_invalid": v.first_invalid}
    except Exception as e:
        return {"error": f"{type(e).__name__}: {str(e)[:200]}"}
    finally:
        gw.stop()


def rehash_from(db: Path, seq: int) -> None:
    """What an attacker with write access to the file can do: edit a record, then recompute every
    hash after it. The chain has no secret in it, so nothing stops this."""
    c = sqlite3.connect(db)
    prev = c.execute("SELECT hash FROM audit WHERE seq=?", (seq - 1,)).fetchone()[0]
    for s, at_ms, kind, subj, data in c.execute(
            "SELECT seq, at_ms, kind, subject, data FROM audit WHERE seq>=? ORDER BY seq", (seq,)).fetchall():
        h = hashlib.sha256(f"{prev}\n{s}\n{at_ms}\n{kind}\n{subj}\n{data}".encode()).hexdigest()
        c.execute("UPDATE audit SET prev_hash=?, hash=? WHERE seq=?", (prev, h, s))
        prev = h
    c.commit()
    c.close()


def damage(gw: Gateway, how) -> tuple[Path, Path]:
    """Stop the gateway cleanly-ish, apply `how(connection)` to its journal, return home+manifest."""
    gw.stop()
    time.sleep(0.5)
    c = sqlite3.connect(journal(gw))
    how(c)
    c.commit()
    c.close()
    return gw.home, gw.manifest


def pick_transition(c: sqlite3.Connection) -> int:
    return c.execute("SELECT seq FROM audit WHERE kind='command.transition' AND data LIKE '%APPLIED%' "
                     "ORDER BY seq LIMIT 1").fetchone()[0]


# ------------------------------------------------------------------ experiments

def P1() -> dict:
    """A complete trace of one command, exported, and checked against the device's own records."""
    gw = fresh("p1")
    try:
        rec = run_some(gw, 2)[0]
        rep = export_report.build(gw.op, DEVICE)
        cmd = next(c for c in rep["commands"] if c["command_id"] == rec.command_id)
        t = truth()
        ext = (cmd["device_reference"] or "").removeprefix("labthings:")
        chained_ref = any(f"device reference labthings:{ext}" in json.dumps(e.data)
                          for e in gw.op.audit_tail(1000) if e.kind == "command.transition")
        out = Path(lab.WORK / "prov-p1-report")
        Path(str(out) + ".json").write_text(json.dumps(rep, indent=2, default=str), encoding="utf-8")
        Path(str(out) + ".md").write_text(export_report.to_markdown(rep), encoding="utf-8")
        return {
            "fields_present": {k: bool(cmd.get(k)) for k in
                               ("principal", "credential_id", "request", "authorization", "lifecycle",
                                "device_reference", "device_result", "verification")},
            "authorization": cmd["authorization"], "final_state": cmd["final_state"],
            "device_reference": cmd["device_reference"],
            "device_reference_found_on_labthings_server": ext in [i["id"] for i in t["invocations"]],
            "device_reference_in_hash_chained_record": chained_ref,
            "descriptor_sha256_recorded": [bool(c["descriptor_sha256"]) for c in rep["device_contracts"]],
            "td_source_in_contract": "x-wot-source" in json.dumps(json.loads(
                json.dumps(gw.op.describe(DEVICE).extra, default=str))) or None,
            "audit": rep["audit"], "receipts_inconsistent_with_chain": rep["receipts_inconsistent_with_chain"],
            "report_files": [str(out) + ".json", str(out) + ".md"],
        }
    finally:
        gw.stop()


def P1b() -> dict:
    """Does a changed Thing Description change the contract hash recorded at registration? (needs the
    hotplate stand-in on :5200 and WOT_USER=lab WOT_PASSWORD=bench in the environment)"""
    import copy
    td = json.loads((lab.REPO / "research" / "generalization" / "hotplate_td.json").read_text(encoding="utf-8"))
    edited = copy.deepcopy(td)
    edited["description"] = (edited.get("description") or "") + " (edited)"
    imp = str(lab.REPO / "adapters" / "python" / "wot" / "import_td.py")
    out = {}
    for name, doc in (("A_original", td), ("B_original_again", td), ("C_description_edited", edited)):
        d = lab.WORK / f"prov-hash-{name}"
        d.mkdir(parents=True, exist_ok=True)
        (d / "td.json").write_text(json.dumps(doc), encoding="utf-8")
        subprocess.run([sys.executable, imp, "--td", str(d / "td.json"), "--base-url", "http://127.0.0.1:5200/",
                        "--out", str(d / "o"), "--device-id", "lab.hotplate", "--source", "simulator"],
                       check=True, capture_output=True)
        m = json.loads((d / "o" / "manifest.json").read_text(encoding="utf-8"))
        home = lab.WORK / f"prov-hash-{name}-home"
        shutil.rmtree(home, ignore_errors=True)
        gw = Gateway(home, d / "o" / "manifest.json", grants=False).start()
        try:
            reg = next(e for e in gw.op.audit_tail(50) if e.kind == "device.registered").data
            out[name] = {"td_sha256": m["config"]["descriptor"]["extensions"]["x-wot-source"]["td_sha256"][:16],
                         "descriptor_sha256_in_audit": reg["descriptor_sha256"][:16]}
        finally:
            gw.stop()
    return out


def P2() -> dict:
    """Edit one chained record's data, leave the hashes alone."""
    gw = fresh("p2")
    run_some(gw, 3)
    before = gw.op.audit_verify().records

    def how(c):
        seq = pick_transition(c)
        c.execute("UPDATE audit SET data = REPLACE(data, 'APPLIED', 'FAILED') WHERE seq=?", (seq,))
        how.seq = seq

    home, man = damage(gw, how)
    return {"records": before, "edited_seq": how.seq, "gateway_says": verify(home, man)}


def P3() -> dict:
    """Edit a record and recompute every later hash: what someone with the file can do."""
    gw = fresh("p3")
    run_some(gw, 3)

    def how(c):
        seq = pick_transition(c)
        c.execute("UPDATE audit SET data = REPLACE(data, 'APPLIED', 'FAILED') WHERE seq=?", (seq,))
        c.commit()
        rehash_from(journal(gw), seq)
        how.seq = seq

    home, man = damage(gw, how)
    return {"edited_seq": how.seq, "gateway_says": verify(home, man),
            "reading": "no secret is in the chain, so a rewrite that recomputes every later hash passes"}


def P4() -> dict:
    """Delete a record from the middle."""
    gw = fresh("p4")
    run_some(gw, 3)

    def how(c):
        seq = pick_transition(c)
        c.execute("DELETE FROM audit WHERE seq=?", (seq,))
        how.seq = seq

    home, man = damage(gw, how)
    return {"deleted_seq": how.seq, "gateway_says": verify(home, man)}


def P5() -> dict:
    """Remove the newest records, with and without an anchor kept elsewhere."""
    gw = fresh("p5")
    run_some(gw, 3)
    rep0 = export_report.build(gw.op, DEVICE)
    anchor = rep0["audit"]["anchor"]  # what an operator would have stored somewhere else
    keep = anchor["seq"] - 8

    def how(c):
        c.execute("DELETE FROM audit WHERE seq>?", (keep,))
        how.removed = anchor["seq"] - keep

    home, man = damage(gw, how)
    gw2 = Gateway(home, man, grants=False).start()
    try:
        rep1 = export_report.build(gw2.op, DEVICE, anchor)
        return {"records_removed": how.removed, "anchor_kept_elsewhere": anchor,
                "gateway_says": {k: rep1["audit"][k] for k in ("records", "valid", "first_invalid")},
                "against_anchor": rep1["audit"]["against_earlier_anchor"]}
    finally:
        gw2.stop()


def P6() -> dict:
    """Edit only the receipt row, which the chain does not cover."""
    gw = fresh("p6")
    recs = run_some(gw, 2)
    cid = recs[0].command_id

    def how(c):
        (r,) = c.execute("SELECT receipt FROM commands WHERE command_id=?", (cid,)).fetchone()
        d = json.loads(r)
        d["state"], d["outcome"] = "FAILED", "failed"
        c.execute("UPDATE commands SET receipt=?, state='FAILED' WHERE command_id=?", (json.dumps(d), cid))

    home, man = damage(gw, how)
    gw2 = Gateway(home, man, grants=False).start()
    try:
        rep = export_report.build(gw2.op, DEVICE)
        v = gw2.op.audit_verify()
        shown = next(c for c in rep["commands"] if c["command_id"] == cid)
        return {"gateway_chain_says": {"valid": v.valid, "first_invalid": v.first_invalid},
                "receipt_now_shows": shown["final_state"],
                "report_flags_receipts_inconsistent_with_chain": rep["receipts_inconsistent_with_chain"] == [cid]}
    finally:
        gw2.stop()


def P7() -> dict:
    """A hard kill in the middle of a command: is the chain intact, and what does the record say?"""
    gw = fresh("p7")
    run_some(gw, 1)
    import threading
    out: dict = {}
    t = threading.Thread(target=lambda: out.setdefault("r", _try(lambda: gw.agent.invoke(
        DEVICE, CAP, params(77, 3.0), idempotency_key="p7-long"))), daemon=True)
    t.start()
    time.sleep(1.0)
    inflight = truth()
    gw.kill()
    t.join(timeout=10)
    time.sleep(0.5)
    verdict = verify(gw.home, gw.manifest)
    gw2 = Gateway(gw.home, gw.manifest).start()
    try:
        r = gw2.agent.command(next(x for x in gw2.op.commands(limit=10) if x.idempotency_key == "p7-long").command_id)
        settle(timeout=10)
        return {"device_busy_at_kill": inflight["busy"], "chain_after_kill": verdict,
                "command_after_restart": {"state": str(r.state), "path": [str(t.state) for t in r.transitions]},
                "device_ground_truth_after": truth()["position"],
                "note": "a kill leaves a valid chain; the unfinished command is not silently completed"}
    finally:
        gw2.stop()


def _try(fn):
    try:
        return fn()
    except Exception as e:
        return f"{type(e).__name__}: {str(e)[:120]}"


def P8() -> dict:
    """Storage exhaustion: a journal with a hard size limit. Does the gateway refuse before it
    dispatches something it cannot record, and do stops and reads behave as ADR-0021 says?"""
    gw = fresh("p8", extra=["--journal-max-mib", "3", "--journal-reserve-mib", "1"])
    try:
        start_jobs = truth()["jobs_started"]
        size0 = journal(gw).stat().st_size
        refused_at, refusal = None, None
        for i in range(600):
            try:
                r = gw.agent.invoke(DEVICE, CAP, params(i % 50, 0.1), idempotency_key=f"x-{i}")
            except Exception as e:
                refused_at, refusal = i, f"{type(e).__name__}: {str(e)[:200]}"
                break
            if str(r.state).endswith("DENIED"):
                refused_at, refusal = i, "; ".join(f"{e.code}: {e.message[:140]}" for e in r.policy.errors)
                break
        settle(timeout=15)
        t = truth()
        jobs = t["jobs_started"] - start_jobs
        read = _try(lambda: gw.agent.read(DEVICE, "position"))
        return {"limit_mib": 3, "reserve_mib": 1, "journal_bytes_at_start": size0,
                "commands_executed_before_refusal": refused_at, "refusal": refusal,
                "journal_bytes_at_refusal": journal(gw).stat().st_size,
                "device_jobs_started": jobs,
                "no_job_without_a_command": jobs == refused_at,
                "read_when_full": str(read)[:200],
                "chain_valid_after": gw.op.audit_verify().valid}
    finally:
        gw.stop()


def P9() -> dict:
    """LabThings forgets an invocation after its retention time. OpenMHS should still hold the story."""
    gw = fresh("p9")
    try:
        r = run_some(gw, 1)[0]
        ref = r.adapter_result.external_ref.removeprefix("labthings:")
        inv_url = f"{lab.BENCH}/action_invocations/{ref}"
        s0, _ = http_json("GET", inv_url)
        t0 = time.time()
        while time.time() - t0 < 330:
            time.sleep(10)
        s1, body = http_json("GET", inv_url)
        # LabThings expires old invocations lazily: only when the next action is invoked.
        settle()
        direct_move(0)
        time.sleep(0.5)
        s2, body2 = http_json("GET", inv_url)
        rep = export_report.build(gw.op, DEVICE)
        cmd = next(c for c in rep["commands"] if c["command_id"] == r.command_id)
        return {"status_on_labthings_right_after": s0, "status_on_labthings_after_330s": s1,
                "status_after_330s_and_one_more_action": s2,
                "labthings_404_body": body2 if isinstance(body2, dict) else str(body2)[:120],
                "openmhs_still_has": {"state": cmd["final_state"], "device_reference": cmd["device_reference"],
                                      "verification": cmd["verification"]}}
    finally:
        gw.stop()


ALL = {f.__name__: f for f in (P1, P1b, P2, P3, P4, P5, P6, P7, P8, P9)}


def main() -> None:
    names = sys.argv[1:] or [n for n in ALL if n not in ("P9", "P1b")]
    for n in names:
        t = time.time()
        try:
            OUT[n] = {"doc": ALL[n].__doc__.strip().split("\n")[0], **ALL[n]()}
        except Exception as e:
            import traceback
            OUT[n] = {"harness_error": f"{type(e).__name__}: {e}", "trace": traceback.format_exc()[-600:]}
        print(f"[{n}] {time.time() - t:.1f}s", file=sys.stderr, flush=True)
    print(json.dumps(OUT, indent=1, default=str))


if __name__ == "__main__":
    main()
