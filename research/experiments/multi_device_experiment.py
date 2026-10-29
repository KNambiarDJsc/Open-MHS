"""Phase 7: one workflow across two independent devices, with a fault in the middle.

Devices (all simulators, no hardware):
  ofm.stage     the OpenFlexure server's simulated stage (LabThings-FastAPI), reached through a
                fault-injecting proxy. Moves need a human approval and are bounded to |x| <= 100.
  lab.hotplate  the stirring-hotplate stand-in (HTTP Basic, synchronous actions).

Workflow: heat to 40 Cel (verified by readback), move the stage to x = 50 (approval), stop heating.
Fault: after the stage move is accepted, every status poll is lost, so its outcome is unknown.

  python multi_device_experiment.py > ../results/multi_device.json

Arms: OpenMHS plan (R1 clean, R2 fault then reconcile and resume, R3 fault then compensate) and a
plain script written directly against the two HTTP interfaces (D1 clean, D2 fault, bare;
D3 fault, with try/finally cleanup). The script is mine and deliberately ordinary.
"""

from __future__ import annotations

import base64
import json
import os
import shutil
import sys
import threading
import time
import urllib.request

import lab  # first: it puts the importer and the Python client on the path
import export_report
import import_td
import td_import
from lab import FaultProxy, Gateway, http_json
from nl_experiments import BOUND, STAGE_REVIEW, probe

sys.path.insert(0, str(lab.REPO / "research" / "generalization"))
import run_generalization as gen  # noqa: E402

OFM = "http://127.0.0.1:5000/api/v3"
HOT = "http://127.0.0.1:5200"
AUTH = {"Authorization": "Basic " + base64.b64encode(b"lab:bench").decode()}
os.environ.update({"WOT_USER": "lab", "WOT_PASSWORD": "bench"})


# ------------------------------------------------------------------ ground truth

def hot(method: str, path: str, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(f"{HOT}/{path}", data=data, method=method,
                                 headers={"Content-Type": "application/json", **AUTH})
    with urllib.request.urlopen(req, timeout=5) as r:  # noqa: S310
        raw = r.read()
        return json.loads(raw) if raw else None


def world() -> dict:
    _, pos = http_json("GET", f"{OFM}/stage/position")
    return {"stage_x": (pos or {}).get("x"), "hotplate_target": hot("GET", "properties/targetTemperature"),
            "hotplate_state": hot("GET", "properties/state")}


def reset() -> None:
    http_json("POST", f"{OFM}/stage/move_absolute", {"x": 0, "y": 0, "z": 0}, timeout=20)
    time.sleep(1.5)
    try:
        hot("POST", "actions/stop", {})
    except Exception:
        pass


# ------------------------------------------------------------------ OpenMHS side

def build(label: str, proxy: FaultProxy) -> Gateway:
    tds = http_json("GET", f"{OFM}/thing_descriptions/")[1]
    sm = td_import.map_thing(tds["stage"], base_url=proxy.base + "/", device_id="ofm.stage",
                             overrides=STAGE_REVIEW, samples=probe(tds["stage"]))
    sdir = lab.WORK / f"md-{label}-stage"
    sdir.mkdir(parents=True, exist_ok=True)
    spath = sdir / "manifest.json"
    spath.write_text(json.dumps(import_td.build_manifest(sm, proxy.base + "/", sdir, source="simulator",
                                                        timeout_ms=2000), indent=2), encoding="utf-8")
    hm = td_import.map_thing(gen.TD, base_url=gen.BASE, device_id="lab.hotplate", overrides=gen.REVIEWED)
    hdir = lab.WORK / f"md-{label}-hot"
    hdir.mkdir(parents=True, exist_ok=True)
    hpath = hdir / "manifest.json"
    hpath.write_text(json.dumps(import_td.build_manifest(hm, gen.BASE, hdir, source="simulator",
                                                        timeout_ms=1500), indent=2), encoding="utf-8")
    home = lab.WORK / f"md-{label}-home"
    if home.exists():
        shutil.rmtree(home, ignore_errors=True)
    gw = Gateway(home, [spath, hpath], grants=False).start()
    op = gw.op
    for g in ({"id": "r", "principal": "agent:lab", "device": "*", "capability": "*", "operations": ["read"], "effect": "allow"},
              {"id": "heat", "principal": "agent:lab", "device": "lab.hotplate", "capability": "*",
               "operations": ["invoke"], "effect": "allow"},
              {"id": "move", "principal": "agent:lab", "device": "ofm.stage", "capability": "move_x",
               "operations": ["invoke"], "effect": "require_approval"}):
        op.add_grant(g)
    gw.token = op.create_token("agent:lab", any_scope=True).token
    gw.agent = lab.Client.from_home(home, token=gw.token)
    return gw


def plan() -> dict:
    hotplate = {"device": "lab.hotplate"}
    return {
        "description": "heat the plate, move the stage under approval, stop heating",
        "lease_ttl_ms": 120000,
        "steps": [
            {"id": "heat", "request": {"target": {**hotplate, "capability": "set_program"}, "operation": "invoke",
                                        "params": {"temperature": {"value": 40, "unit": "Cel"},
                                                   "minutes": {"value": 1, "unit": "min"}}},
             "on_failure": "compensate",
             "compensate": {"target": {**hotplate, "capability": "stop"}, "operation": "invoke", "params": {}}},
            {"id": "move", "after": ["heat"],
             "request": {"target": {"device": "ofm.stage", "capability": "move_x"}, "operation": "invoke",
                         "params": {"x": 50}},
             "on_failure": "ask"},
            {"id": "stop", "after": ["move"],
             "request": {"target": {**hotplate, "capability": "stop"}, "operation": "invoke", "params": {}},
             "on_failure": "ask"},
        ],
    }


class Approver:
    """The human: approves the stage move only if it is inside the bound."""

    def __init__(self, gw: Gateway) -> None:
        self.gw, self.log, self.halt = gw, [], threading.Event()
        threading.Thread(target=self.run, daemon=True).start()

    def run(self) -> None:
        seen: set = set()
        while not self.halt.is_set():
            try:
                for r in self.gw.op.commands(states=["AWAITING_APPROVAL"]):
                    if r.command_id in seen:
                        continue
                    seen.add(r.command_id)
                    x = (r.request.get("params") or {}).get("x")
                    x = x.get("value") if isinstance(x, dict) else x
                    ok = r.request["target"]["capability"] == "move_x" and isinstance(x, (int, float)) and abs(x) <= BOUND
                    self.log.append({"command_id": r.command_id, "x": x, "decision": "approved" if ok else "left waiting"})
                    if ok:
                        self.gw.op.approve(r.command_id)
            except Exception as e:
                self.log.append({"error": str(e)[:100]})
            time.sleep(0.3)


def plan_view(p) -> dict:
    return {"state": str(getattr(p.state, "value", p.state)), "reason": getattr(p, "reason", None),
            "current": getattr(p, "current", None),
            "steps": {s.id if hasattr(s, "id") else str(i): str(getattr(s.status, "value", s.status))
                      for i, s in enumerate(p.steps)}}


def run_openmhs(label: str, *, fault: bool, then: str | None = None) -> dict:
    reset()
    proxy = FaultProxy(upstream="http://127.0.0.1:5000")
    gw = build(label, proxy)
    ap = Approver(gw)
    out: dict = {"world_before": world()}
    try:
        if fault:
            proxy.add(r"GET /api/v3/action_invocations/", "reset", 99)
        p = gw.agent.submit_plan(plan())
        out["plan_after_submit"] = plan_view(p)  # returns at the first wait: here, the approval
        end = time.time() + 90
        while time.time() < end:  # the approver's call carries the plan on after it approves
            p = gw.op.plan(p.id)
            if str(getattr(p.state, "value", p.state)) not in ("RUNNING", "WAITING_APPROVAL", "COMPENSATING"):
                break
            time.sleep(0.5)
        out["plan_when_it_stopped"] = plan_view(p)
        out["world_when_plan_stopped"] = world()
        if fault:
            move = next((r for r in gw.op.commands(limit=50) if r.request["target"]["capability"] == "move_x"), None)
            out["move_command"] = {"state": str(move.state), "ref": move.adapter_result.external_ref if move.adapter_result else None}
            proxy.clear()
            time.sleep(3)  # the device finishes on its own; nothing was cancelled or resent
            out["world_after_waiting"] = world()
            if then in ("resume", "compensate"):
                rec = gw.op.reconcile(move.command_id)
                out["reconcile"] = {"state": str(rec.state)}
                time.sleep(0.5)
                p = gw.op.decide_plan(p.id, then, reason="experiment operator") if hasattr(gw.op, "decide_plan") else p
                out["plan_after_decision"] = plan_view(gw.op.plan(p.id))
        out["world_after"] = world()
        rep = export_report.build(gw.op, None)
        out["report"] = {"commands": [{"capability": c["request"]["target"]["capability"], "state": c["final_state"],
                                       "approved_by": c["authorization"]["approved_by"],
                                       "ref": c["device_reference"]} for c in rep["commands"]],
                         "audit_valid": rep["audit"]["valid"], "audit_records": rep["audit"]["records"],
                         "inconsistent_receipts": rep["receipts_inconsistent_with_chain"],
                         "contracts": [c["device"] for c in rep["device_contracts"]]}
        p0 = lab.WORK / f"md-{label}-report"
        (p0.parent / f"{p0.name}.md").write_text(export_report.to_markdown(rep), encoding="utf-8")
        out["approvals"] = ap.log
        return out
    finally:
        ap.halt.set()
        gw.stop()
        proxy.stop()


# ------------------------------------------------------------------ the plain script

def script(fault: bool, cleanup: bool) -> dict:
    """What a lab would write first: three calls in order, no gateway."""
    reset()
    proxy = FaultProxy(upstream="http://127.0.0.1:5000")
    log = []
    try:
        if fault:
            proxy.add(r"GET /api/v3/action_invocations/", "reset", 99)
        try:
            hot("POST", "actions/setProgram", {"temperature": 40, "minutes": 1})
            log.append("heating started")
            _, inv = http_json("POST", f"{proxy.base}/api/v3/stage/move_absolute", {"x": 50, "y": 0, "z": 0}, timeout=20)
            for _ in range(60):
                if inv.get("status") in ("completed", "error", "cancelled"):
                    break
                time.sleep(0.2)
                _, inv = http_json("GET", f"{proxy.base}/api/v3/action_invocations/{inv['id']}", timeout=3)
            log.append(f"move {inv.get('status')}")
            hot("POST", "actions/stop", {})
            log.append("heating stopped")
        except Exception as e:
            log.append(f"script stopped: {type(e).__name__}")
            if cleanup:
                hot("POST", "actions/stop", {})
                log.append("cleanup: heating stopped")
        time.sleep(2)
        proxy.clear()
        return {"script_log": log, "world_after": world(), "record_left_behind": "the script's own print output only"}
    finally:
        proxy.stop()


def main() -> None:
    out = {"_meta": {"when": time.strftime("%FT%T%z"), "note": "simulators only; stage bound is an experiment choice"}}
    for name, fn in (("R1_clean", lambda: run_openmhs("r1", fault=False)),
                     ("R2_fault_reconcile_resume", lambda: run_openmhs("r2", fault=True, then="resume")),
                     ("R3_fault_compensate", lambda: run_openmhs("r3", fault=True, then="compensate")),
                     ("D1_script_clean", lambda: script(False, False)),
                     ("D2_script_fault_bare", lambda: script(True, False)),
                     ("D3_script_fault_cleanup", lambda: script(True, True))):
        t = time.time()
        try:
            out[name] = fn()
        except Exception as e:
            import traceback
            out[name] = {"harness_error": f"{type(e).__name__}: {str(e)[:300]}", "trace": traceback.format_exc()[-700:]}
        print(f"[{name}] {time.time() - t:.1f}s", file=sys.stderr, flush=True)
    reset()
    print(json.dumps(out, indent=1, default=str))


if __name__ == "__main__":
    main()
