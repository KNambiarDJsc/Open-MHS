"""MAD-01..MAD-07: a MADSci closed loop with OpenMHS governing the liquid handler.

Every scenario builds a fresh stack:
- an OpenMHS gateway with the PyLabRobot adapter (governed runs only);
- MADSci's two nodes (stack/nodes.py), its location manager, and its workcell manager
  with in-memory backends.

Checks compare three records:
- what MADSci reported (workflows, steps, attempts);
- what OpenMHS reported (receipts, audit);
- what the simulator holds: the PyLabRobot state file, with the dose in each well and the
  simulator's own transfer counter.

    $env:OPENMHS_BIN = "...\\openmhs.exe"; $env:PLR_PYTHON = "<venv with pylabrobot>\\python.exe"
    <venv with madsci and pylabrobot>\\python run_scenarios.py [--out ...] [--only MAD-04]
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import platform
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from pathlib import Path

import httpx

HERE = Path(__file__).resolve()
EXP = HERE.parents[1]
sys.path[:0] = [str(EXP / "stack"), str(HERE.parents[2] / "shared")]
os.environ.setdefault("PYTHONIOENCODING", "utf-8")
# MADSci prints box-drawing tables; on a cp1252 console that would end the scenario.
sys.stdout.reconfigure(encoding="utf-8", errors="replace")
sys.stderr.reconfigure(encoding="utf-8", errors="replace")
from harness import REPO, Check, Lab, summary  # noqa: E402
from run_record import finish_report  # noqa: E402

import experiment  # noqa: E402
from madsci.client.workcell_client import WorkcellClient  # noqa: E402

PLR_ADAPTER = REPO / "adapters" / "python" / "pylabrobot" / "plr_adapter.py"
DEVICE = "plr.lh-01"
TRUE_EC50 = 42.0
PY = sys.executable


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Stack:
    def __init__(self, root: Path, mode: str = "openmhs", approval: bool = False, seed: int = 7,
                 approval_wait_s: float = 60.0):
        self.root, self.mode, self.approval, self.seed = root, mode, approval, seed
        self.approval_wait_s = approval_wait_s
        root.mkdir(parents=True)
        self.state = root / "plr_state.json"
        self.fault = root / "plr_fault.json"
        self.native_fault = root / "native_fault.json"
        self.procs: dict[str, subprocess.Popen] = {}
        self.ports = {n: free_port() for n in ("liquid_handler", "plate_reader", "workcell", "location", "data")}
        self.lab = None
        if mode == "openmhs":
            self.lab = Lab(root / "openmhs", {
                "openmhs_manifest": "0.1",
                "adapter": {"id": "openmhs.py.pylabrobot", "version": "0.1.0"},
                "runtime": {"command": [os.environ["PLR_PYTHON"], str(PLR_ADAPTER)],
                            "env": {"PYTHONPATH": str(REPO / "python")}, "protocol": ["0.1"]},
                "capabilities": {"allow": [{"name": "*", "operations": ["read"]},
                                           {"name": "liquid.transfer", "operations": ["invoke"]}]},
                "filesystem": {"write": [str(root)]},
                "config": {"state_file": str(self.state), "fault_file": str(self.fault), "tips": 96,
                           "source_ul": {"A1": 360, "A2": 360, "A3": 360},
                           "dest_wells": [f"B{i}" for i in range(1, 13)]},
            })

    def url(self, name: str) -> str:
        return f"http://127.0.0.1:{self.ports[name]}"

    def spawn(self, name: str, args: list[str], env: dict | None = None) -> None:
        log = open(self.root / f"{name}.log", "ab")
        # MADSci keeps a per-machine identity registry that locks a node's name: give each stack its
        # own, or a second stack's identically named nodes are refused while the first runs.
        self.procs[name] = subprocess.Popen(
            [PY, *args], stdout=log, stderr=subprocess.STDOUT, cwd=str(self.root),
            env={**os.environ, "PYTHONIOENCODING": "utf-8",
                 "MADSCI_REGISTRY_PATH": str(self.root / "madsci-registry.json"), **(env or {})})

    def wait_http(self, name: str, path: str, timeout: float = 60) -> None:
        deadline = time.time() + timeout
        while time.time() < deadline:
            with contextlib.suppress(Exception):
                if httpx.get(self.url(name) + path, timeout=2).status_code < 500:
                    return
            if self.procs[name].poll() is not None:
                raise RuntimeError(f"{name} exited: see {self.root / (name + '.log')}")
            time.sleep(0.25)
        raise RuntimeError(f"{name} not up in {timeout} s")

    def start_liquid_handler(self) -> None:
        args = [str(EXP / "stack" / "nodes.py"), "liquid_handler", "--port", str(self.ports["liquid_handler"]),
                "--mode", self.mode]
        if self.mode == "openmhs":
            args += ["--gateway", self.lab.url, "--approval-wait-s", str(self.approval_wait_s)]
        else:
            args += ["--state-file", str(self.state), "--native-fault-file", str(self.native_fault)]
        # In the environment, not on the command line, where any process could read it.
        self.spawn("liquid_handler", args, env={"OPENMHS_TOKEN": self.token} if self.mode == "openmhs" else None)
        self.wait_http("liquid_handler", "/status")

    def start(self) -> None:
        if self.lab:
            self.lab.sticky_port = True
            self.lab.start()
            effect = "require_approval" if self.approval else "allow"
            grants = [{"id": "read", "device": DEVICE, "capability": "*", "operations": ["read"], "effect": "allow"},
                      {"id": "transfer", "device": DEVICE, "capability": "liquid.transfer",
                       "operations": ["invoke"], "effect": effect}]
            self.agent = self.lab.agent("agent:madsci", grants)
            self.token = self.agent.token if hasattr(self.agent, "token") else self.agent._token
        self.start_liquid_handler()
        self.spawn("plate_reader", [str(EXP / "stack" / "nodes.py"), "plate_reader", "--port",
                                    str(self.ports["plate_reader"]), "--state-file", str(self.state),
                                    "--seed", str(self.seed)])
        self.wait_http("plate_reader", "/status")
        self.spawn("location", [str(EXP / "stack" / "location.py"), "--port", str(self.ports["location"])])
        self.wait_http("location", "/locations")
        self.spawn("data", [str(EXP / "stack" / "data.py"), "--port", str(self.ports["data"])])
        self.wait_http("data", "/health")
        os.environ["DATA_SERVER_URL"] = self.url("data")  # read by experiment.result_of
        self.spawn("workcell", [str(EXP / "stack" / "workcell.py"), "--port", str(self.ports["workcell"]),
                                "--node", f"liquid_handler={self.url('liquid_handler')}",
                                "--node", f"plate_reader={self.url('plate_reader')}"],
                   env={"LOCATION_SERVER_URL": self.url("location"), "DATA_SERVER_URL": self.url("data")})
        self.wait_http("workcell", "/state")
        time.sleep(2)  # the workcell's first poll of its nodes
        self.client = WorkcellClient(self.url("workcell"))

    def kill(self, name: str) -> None:
        p = self.procs.get(name)
        if p and p.poll() is None:
            if platform.system() == "Windows":
                subprocess.run(["taskkill", "/T", "/F", "/PID", str(p.pid)], capture_output=True)
            else:
                p.kill()
            p.wait()

    def stop(self) -> None:
        for name in list(self.procs):
            self.kill(name)
        if self.lab:
            self.lab.kill()

    def sim(self) -> dict:
        s = json.loads(self.state.read_text())
        return {"count": s["count"], "doses": {w: s["deck"][f"dest_well_{w}"]["volume"] for w in experiment.WELLS
                                              if s["deck"][f"dest_well_{w}"]["volume"]}}

    def receipt(self, key: str) -> dict | None:
        r = self.agent.command_by_key(DEVICE, key)
        return r.to_dict() if r else None


def start_async(stack: Stack, well: str, dose: float, key: str):
    return stack.client.start_workflow(experiment.workflow(well, dose, key), await_completion=False,
                                       prompt_on_error=False, raise_on_failed=False)


def await_wf(stack: Stack, wf_id: str, timeout: float = 90):
    deadline = time.time() + timeout
    while time.time() < deadline:
        wf = stack.client.query_workflow(wf_id)
        if experiment.status_of(wf) != "running":
            return wf
        time.sleep(0.3)
    return stack.client.query_workflow(wf_id)


def retry(stack: Stack, wf_id: str, index: int = 0):
    stack.client.retry_workflow(wf_id, index=index, await_completion=False, raise_on_failed=False,
                                prompt_on_error=False)
    time.sleep(0.5)
    return await_wf(stack, wf_id)


# ------------------------------------------------------------------ scenarios


def await_held(s: Stack, key: str, timeout: float = 20) -> dict | None:
    """The command once it has settled into waiting. Between submission and the hold it is
    briefly CREATED and VALIDATING, and an earlier look would see those."""
    deadline, found = time.time() + timeout, None
    while time.time() < deadline:
        found = s.receipt(key)
        if found and found["state"] not in ("CREATED", "VALIDATING"):
            return found
        time.sleep(0.05)
    return found


def mad01(s: Stack, check: Check) -> dict:
    result = experiment.run(s.client.workcell_server_url, 6, "mad01")
    recs = result["records"]
    check("6 iterations, every workflow completed", len(recs) == 6
          and all(r["workflow_status"] == "completed" for r in recs), [r["workflow_status"] for r in recs])
    sim = s.sim()
    check("the simulator ran exactly one transfer per iteration", sim["count"] == 6, sim["count"])
    for r in recs:
        receipt = s.receipt(r["key"])
        measured = r["measure"]["result"]["dose_ul"]
        check(f"iteration {r['iteration']}: planned, OpenMHS, MADSci and measured dose agree",
              receipt and receipt["state"] == "VERIFIED"
              and r["dispense"]["result"]["command_id"] == receipt["command_id"]
              and abs(measured - r["planned_dose_ul"]) < 1e-9,
              (r["planned_dose_ul"], measured, receipt and receipt["state"]))
    lo, hi = result["bracket"]
    check("the estimate brackets the true EC50 (42 uL)", lo <= TRUE_EC50 <= hi, result["bracket"])
    return result


def mad01_native(s: Stack, check: Check) -> dict:
    result = experiment.run(s.client.workcell_server_url, 6, "mad01n")
    check("native: 6 iterations completed", all(r["workflow_status"] == "completed" for r in result["records"]))
    check("native: one transfer per iteration", s.sim()["count"] == 6, s.sim()["count"])
    return result


def mad02(s: Stack, check: Check) -> dict:
    out = {}
    # (a) The OpenMHS gateway is down: the dispense step fails and nothing is sent.
    s.lab.kill()
    before = s.sim()["count"]
    wf = experiment.run_iteration(s.client, 0, 50.0, "mad02a")
    check("gateway down: MADSci's workflow failed at the dispense step", wf["workflow_status"] == "failed"
          and wf["dispense"]["status"] == "failed", (wf["workflow_status"], wf["dispense"]))
    check("gateway down: nothing was transferred", s.sim()["count"] == before)
    out["gateway_down"] = wf
    # (b) The plate reader is down: what does MADSci do with the measure step?
    s.lab.start()
    s.kill("plate_reader")
    started = start_async(s, "B2", 40.0, "mad02b")
    time.sleep(15)
    wf2 = s.client.query_workflow(started.workflow_id)
    out["reader_down_after_15s"] = {"status": experiment.status_of(wf2), **experiment.steps_of(wf2)}
    check("reader down: the dispense still happened, once", s.sim()["count"] == before + 1, s.sim()["count"])
    check("reader down: MADSci does not report the workflow complete",
          experiment.status_of(wf2) != "completed", out["reader_down_after_15s"]["status"])
    return out


def mad03(s: Stack, check: Check) -> dict:
    results = {}

    def run(tag, offset):
        results[tag] = experiment.run(s.client.workcell_server_url, 3, tag, well_offset=offset)

    threads = [threading.Thread(target=run, args=(t, o)) for t, o in (("mad03x", 0), ("mad03y", 6))]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    recs = [r for res in results.values() for r in res["records"]]
    check("both experiments' workflows completed", all(r["workflow_status"] == "completed" for r in recs),
          [r["workflow_status"] for r in recs])
    check("six transfers, each in its own well, doses as planned", s.sim()["count"] == 6
          and all(abs(r["measure"]["result"]["dose_ul"] - r["planned_dose_ul"]) < 1e-9 for r in recs), s.sim())
    windows = []
    for r in recs:
        rec = s.receipt(r["key"])
        ts = {t["state"]: t["at"] for t in rec["transitions"]}
        windows.append((ts["DISPATCHED"], ts["VERIFIED"], r["key"]))
    windows.sort()
    overlap = any(windows[i + 1][0] < windows[i][1] for i in range(len(windows) - 1))
    check("OpenMHS never had two transfers in flight on the device", not overlap, windows)
    return {"experiments": results, "openmhs_windows": windows}


def mad04(s: Stack, check: Check) -> dict:
    s.fault.write_text(json.dumps({"next_transfer": "apply_then_hang"}))
    first = experiment.run_iteration(s.client, 0, 60.0, "mad04")
    after_first = s.sim()
    check("the transfer happened; its answer was lost: MADSci step UNKNOWN, workflow failed",
          first["dispense"]["status"] == "unknown" and first["workflow_status"] == "failed"
          and after_first["count"] == 1, (first["dispense"]["status"], first["workflow_status"], after_first))
    # Someone retries the step in MADSci before reconciling: the same key, so the same command.
    wf = retry(s, first["workflow_id"], 0)
    steps = experiment.steps_of(wf)
    check("retry before reconciling: still UNKNOWN, and no second transfer",
          steps["dispense"]["status"] == "unknown" and s.sim()["count"] == 1, (steps["dispense"], s.sim()))
    receipt = s.receipt("mad04-iter0-dispense")
    rec = s.lab.operator.call("commands.reconcile", {"command_id": receipt["command_id"]})
    check("reconciled in OpenMHS: RECONCILED_APPLIED", rec["state"] == "RECONCILED_APPLIED", rec["state"])
    wf = retry(s, first["workflow_id"], 0)
    steps = experiment.steps_of(wf)
    check("retry after reconciling: the step succeeds from the existing command, the workflow completes",
          experiment.status_of(wf) == "completed" and steps["dispense"]["result"]["openmhs_state"] == "RECONCILED_APPLIED",
          (experiment.status_of(wf), steps["dispense"]))
    check("the well holds one dose, not two, and the measurement says so",
          s.sim()["count"] == 1 and abs(steps["measure"]["result"]["dose_ul"] - 60.0) < 1e-9,
          (s.sim(), steps["measure"]["result"]))
    return {"first": first, "final": steps}


def mad04_native(s: Stack, check: Check) -> dict:
    s.native_fault.write_text("lost")
    first = experiment.run_iteration(s.client, 0, 60.0, "mad04n")
    check("native: the transfer happened, MADSci was told it failed", first["dispense"]["status"] == "failed"
          and s.sim()["count"] == 1, (first["dispense"]["status"], s.sim()))
    wf = retry(s, first["workflow_id"], 0)
    steps = experiment.steps_of(wf)
    check("native: the retry transfers again; the well holds two doses",
          experiment.status_of(wf) == "completed" and s.sim()["count"] == 2
          and abs(steps["measure"]["result"]["dose_ul"] - 120.0) < 1e-9, (s.sim(), steps["measure"]["result"]))
    return {"first": first, "final": steps, "measured_dose_ul": steps["measure"]["result"]["dose_ul"]}


def mad05(s: Stack, check: Check) -> dict:
    started = start_async(s, "B1", 50.0, "mad05")
    held = await_held(s, "mad05")
    check("OpenMHS holds the transfer for approval; nothing transferred", held and held["state"] == "AWAITING_APPROVAL"
          and s.sim()["count"] == 0, held and held["state"])
    mid = s.client.query_workflow(started.workflow_id)
    check("meanwhile MADSci shows the step running", experiment.status_of(mid) == "running", experiment.status_of(mid))
    time.sleep(2)
    approved = s.lab.operator.call("commands.approve", {"command_id": held["command_id"]}, timeout=60)
    wf = await_wf(s, started.workflow_id)
    check("approved by the operator: VERIFIED, and the MADSci workflow completes",
          approved["state"] == "VERIFIED" and experiment.status_of(wf) == "completed", (approved["state"],
                                                                                        experiment.status_of(wf)))
    return {"held": summary(held) if held else None, "approved": summary(approved), "workflow": experiment.steps_of(wf)}


def await_step(s: Stack, wf_id: str, timeout: float = 30) -> dict:
    wf = await_wf(s, wf_id, timeout)
    return {"workflow": experiment.status_of(wf), **experiment.steps_of(wf)}


def mad08(s: Stack, check: Check) -> dict:
    """Nobody approves in time. The step must say what is true, and nothing may run later."""
    started = start_async(s, "B1", 50.0, "mad08")
    first = await_step(s, started.workflow_id)
    held = s.receipt("mad08")
    check("no approval in time: the command was withdrawn, on record, and never sent",
          held["state"] == "CANCELLED" and "DISPATCHED" not in [t["state"] for t in held["transitions"]],
          held["state"])
    check("MADSci's step failed because it is known not to have run, and says why",
          first["dispense"]["status"] == "failed" and first["workflow"] == "failed"
          and "withdrawn" in str(first["dispense"]["errors"]), first["dispense"])
    check("nothing was transferred", s.sim()["count"] == 0, s.sim())
    # The approver turns up late. It must change nothing.
    late = None
    try:
        s.lab.operator.call("commands.approve", {"command_id": held["command_id"]}, timeout=30)
    except Exception as e:
        late = getattr(e, "code", type(e).__name__)
    check("a late approval is refused and nothing runs", late == "command.invalid_state" and s.sim()["count"] == 0,
          (late, s.sim()))
    # MADSci retries the step: approval is asked for again, under a linked new attempt.
    s.client.retry_workflow(started.workflow_id, index=0, await_completion=False, raise_on_failed=False,
                            prompt_on_error=False)
    second = await_held(s, "mad08~2")
    check("retry: a new attempt, held for approval, linked to the withdrawn one",
          second and second["state"] == "AWAITING_APPROVAL" and second["command_id"] != held["command_id"],
          second and second["state"])
    approved = s.lab.operator.call("commands.approve", {"command_id": second["command_id"]}, timeout=30)
    wf = await_wf(s, started.workflow_id)
    steps = experiment.steps_of(wf)
    check("approved in time: VERIFIED, the workflow completes, exactly one dose in the well",
          approved["state"] == "VERIFIED" and experiment.status_of(wf) == "completed" and s.sim()["count"] == 1
          and abs(steps["measure"]["result"]["dose_ul"] - 50.0) < 1e-9, (approved["state"], s.sim()))
    check("the step's record ties the attempts together",
          steps["dispense"]["result"]["earlier_attempts"] == [{"command_id": held["command_id"], "state": "CANCELLED"}]
          and steps["dispense"]["result"]["command_id"] == second["command_id"]
          and steps["dispense"]["result"]["approved_by"], steps["dispense"]["result"])
    return {"first": first, "withdrawn": summary(held), "second": summary(approved), "final": steps}


def mad09(s: Stack, check: Check) -> dict:
    """An approver says no. That is a definite answer, and a retry does not ask again."""
    started = start_async(s, "B1", 50.0, "mad09")
    held = await_held(s, "mad09")
    approval = next(a for a in s.lab.operator.call("approvals.list", {}) if a.get("command_id") == held["command_id"])
    s.lab.operator.call("approvals.deny", {"id": approval["id"], "reason": "wrong plate"})
    wf = await_wf(s, started.workflow_id)
    steps = experiment.steps_of(wf)
    check("denied: MADSci's step failed, with the approver's denial, nothing transferred",
          steps["dispense"]["status"] == "failed" and "approval.denied" in str(steps["dispense"]["errors"])
          and s.sim()["count"] == 0, (steps["dispense"], s.sim()))
    wf2 = retry(s, started.workflow_id, 0)
    steps2 = experiment.steps_of(wf2)
    check("retry: the denial stands; no new approval is requested, nothing is sent",
          steps2["dispense"]["status"] == "failed" and s.receipt("mad09~2") is None and s.sim()["count"] == 0,
          (steps2["dispense"]["status"], s.sim()))
    return {"denied": steps, "after_retry": steps2}


def mad06(s: Stack, check: Check) -> dict:
    s.fault.write_text(json.dumps({"next_transfer": "apply_then_hang"}))
    started = start_async(s, "B1", 55.0, "mad06")
    deadline = time.time() + 20
    while time.time() < deadline and s.sim()["count"] < 1:
        time.sleep(0.1)
    time.sleep(0.5)
    s.lab.kill()  # the gateway (and its adapter) die mid-transfer
    wf = await_wf(s, started.workflow_id)
    check("gateway killed mid-step: MADSci's step UNKNOWN, not failed or succeeded",
          experiment.steps_of(wf)["dispense"]["status"] == "unknown", experiment.steps_of(wf)["dispense"])
    s.lab.start()
    receipt = s.receipt("mad06")
    check("after the gateway restarts: UNKNOWN_OUTCOME, nothing re-sent", receipt["state"] == "UNKNOWN_OUTCOME"
          and s.sim()["count"] == 1, (receipt["state"], s.sim()))
    rec = s.lab.operator.call("commands.reconcile", {"command_id": receipt["command_id"]})
    check("reconciled: RECONCILED_APPLIED", rec["state"] == "RECONCILED_APPLIED", rec["state"])
    wf = retry(s, started.workflow_id, 0)
    steps = experiment.steps_of(wf)
    check("MADSci retry completes from the existing command; one dose in the well",
          experiment.status_of(wf) == "completed" and s.sim()["count"] == 1
          and abs(steps["measure"]["result"]["dose_ul"] - 55.0) < 1e-9, (experiment.status_of(wf), s.sim()))
    return {"final": steps}


def mad07(stacks: list[Stack], check: Check) -> dict:
    runs = []
    for i, s in enumerate(stacks):
        os.environ["DATA_SERVER_URL"] = s.url("data")  # read by experiment.result_of: this stack's, not the last started
        res = experiment.run(s.client.workcell_server_url, 6, "mad07")  # same tag, same seed: same keys
        receipts = [s.receipt(r["key"]) for r in res["records"]]
        runs.append({"planned": [r["planned_dose_ul"] for r in res["records"]],
                     "measured": [r["measure"]["result"]["dose_ul"] for r in res["records"]],
                     "responses": [r["measure"]["result"]["response"] for r in res["records"]],
                     "openmhs_states": [[t["state"] for t in rc["transitions"]] for rc in receipts],
                     "simulator": s.sim(), "estimate": res["ec50_estimate_ul"],
                     "audit_valid": s.lab.operator.audit_verify().to_dict()["valid"]})
    a, b = runs
    for key in ("planned", "measured", "responses", "openmhs_states", "simulator", "estimate"):
        check(f"identical across the two runs: {key}", a[key] == b[key], (a[key], b[key]))
    check("both audit chains verify", a["audit_valid"] and b["audit_valid"])
    return {"runs": runs}


SCENARIOS = {
    "MAD-01": ("a six-iteration closed loop, governed", mad01, {}),
    "MAD-01-native": ("the same loop without OpenMHS (baseline A)", mad01_native, {"mode": "native"}),
    "MAD-02": ("a device unavailable: the gateway, then the plate reader", mad02, {}),
    "MAD-03": ("two experiments at once on one liquid handler", mad03, {}),
    "MAD-04": ("the transfer happens, its answer is lost; retried in MADSci", mad04, {}),
    "MAD-04-native": ("the same, without OpenMHS", mad04_native, {"mode": "native"}),
    "MAD-05": ("a transfer that needs approval", mad05, {"approval": True}),
    "MAD-06": ("the gateway killed mid-step, restarted, reconciled, retried", mad06, {}),
    "MAD-07": ("the same experiment twice with a fixed seed", mad07, "twice"),
    "MAD-08": ("no approval in time: withdrawn, a late approval refused, retried, one dose", mad08,
               {"approval": True, "approval_wait_s": 3.0}),
    "MAD-09": ("an approver denies: a definite failure that a retry does not undo", mad09, {"approval": True}),
}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="results.json")
    ap.add_argument("--only", nargs="*")
    ap.add_argument("--work")
    a = ap.parse_args()
    import madsci.client
    import pylabrobot

    root = Path(a.work or tempfile.mkdtemp(prefix="mad-runs-"))
    report = {"pylabrobot": pylabrobot.__version__,
              "madsci_client": getattr(madsci.client, "__version__", "0.8.1"),
              "started": time.strftime("%Y-%m-%dT%H:%M:%S%z"), "scenarios": {}}
    for sid, (title, fn, opts) in SCENARIOS.items():
        if a.only and sid not in a.only:
            continue
        check = Check()
        t = time.perf_counter()
        stacks = [Stack(root / f"{sid}-{i}-{uuid.uuid4().hex[:6]}") for i in range(2)] if opts == "twice" \
            else [Stack(root / f"{sid}-{uuid.uuid4().hex[:6]}", **opts)]
        try:
            for st in stacks:
                st.start()
            detail = fn(stacks if opts == "twice" else stacks[0], check)
            for st in stacks:
                if st.lab and st.lab.proc and st.lab.proc.poll() is None:
                    audit = st.lab.operator.audit_verify().to_dict()
                    check("OpenMHS audit chain verifies", audit["valid"], audit["records"])
        except Exception as e:
            import traceback

            detail = {"exception": f"{type(e).__name__}: {e}", "traceback": traceback.format_exc()[-2500:]}
            check("scenario ran to the end", False, detail["exception"])
        finally:
            for st in stacks:
                with contextlib.suppress(Exception):
                    st.stop()
        report["scenarios"][sid] = {"title": title, "passed": check.passed, "seconds": round(time.perf_counter() - t, 1),
                                    "checks": check.items, "detail": detail, "run_dir": str(stacks[0].root)}
        print(f"{'PASS' if check.passed else 'FAIL'} {sid} {title} ({report['scenarios'][sid]['seconds']} s)", flush=True)
        for item in check.items:
            if not item["ok"]:
                print(f"     failed: {item['check']}: {str(item['observed'])[:300]}", flush=True)
    print(f"wrote {finish_report(report, a.out)}")


if __name__ == "__main__":
    main()
