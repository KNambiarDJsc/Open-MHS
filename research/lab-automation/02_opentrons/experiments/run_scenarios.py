"""OT-01..OT-07 and a latency run: Opentrons protocol simulation through OpenMHS.

Each scenario starts a real gateway with the Opentrons adapter (adapters/python/opentrons)
as its child and checks receipts against the simulator's own records: the adapter's state
file (its run counter) and the per-run command logs it writes. Neither is a physical
measurement; the Opentrons simulator does not track liquid.

    $env:OPENMHS_BIN = "...\\openmhs.exe"
    <venv-with-opentrons>\\python run_scenarios.py [--out results.json] [--only OT-04]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import statistics
import sys
import threading
import time
import types
from pathlib import Path

HERE = Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[2] / "shared"))
from harness import REPO, Check, Lab, codes, reached_device, run_all, summary  # noqa: E402

ADAPTER_DIR = REPO / "adapters" / "python" / "opentrons"
PROTOCOL = ADAPTER_DIR / "protocols" / "serial_dilution.py"
DEVICE = "ot.flex-01"
RUN = "protocol.run"
DEFAULTS = {"n_wells": 6, "transfer_ul": 50.0, "diluent_ul": 100.0, "mix_reps": 3}
GRANTS = [
    {"id": "read", "device": DEVICE, "capability": "*", "operations": ["read"], "effect": "allow"},
    {"id": "run", "device": DEVICE, "capability": RUN, "operations": ["invoke"], "effect": "allow"},
]


def sha256(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


class OtLab(Lab):
    def __init__(self, root: Path, timeout_ms: int = 5000):
        root.mkdir(parents=True)
        self.protocol = root / "protocols" / "serial_dilution.py"
        self.protocol.parent.mkdir()
        shutil.copy(PROTOCOL, self.protocol)
        self.state_file, self.fault_file, self.evidence = root / "ot_state.json", root / "ot_fault.json", root / "evidence"
        super().__init__(root, {
            "openmhs_manifest": "0.1",
            "adapter": {"id": "openmhs.py.opentrons", "version": "0.1.0"},
            "runtime": {"command": [sys.executable, str(ADAPTER_DIR / "ot_adapter.py")],
                        "env": {"PYTHONPATH": str(REPO / "python"),
                                "OT_API_CONFIG_DIR": str(root / "ot-config")},
                        "protocol": ["0.1"]},
            "capabilities": {"allow": [{"name": "run.*", "operations": ["read"]},
                                       {"name": "protocol.*", "operations": ["read"]},
                                       {"name": RUN, "operations": ["invoke"]}]},
            "filesystem": {"write": [str(root)]},
            "limits": {"call_timeout_ms": 30000, "start_timeout_ms": 60000},
            "config": {"protocols": {"serial_dilution": {"path": str(self.protocol),
                                                         "sha256": sha256(PROTOCOL.read_bytes())}},
                       "state_file": str(self.state_file), "fault_file": str(self.fault_file),
                       "evidence_dir": str(self.evidence), "run_timeout_ms": timeout_ms},
        })

    def sim(self) -> dict:
        if not self.state_file.exists():
            return {"count": 0, "last_id": "", "last_commands": 0}
        s = json.loads(self.state_file.read_text())
        return {k: s[k] for k in ("count", "last_id", "last_commands")}

    def fault(self, name: str) -> None:
        self.fault_file.write_text(json.dumps({"next_transfer": name}))


def ul(v: float) -> dict:
    return {"value": v, "unit": "uL"}


def run(client, run_id: str, key=None, timeout=60, **over) -> dict:
    p = {**DEFAULTS, **over}
    request = {"target": {"device": DEVICE, "capability": RUN}, "operation": "invoke",
               "params": {"run_id": run_id, "protocol": p.pop("protocol", "serial_dilution"),
                          "n_wells": p["n_wells"], "transfer_ul": ul(p["transfer_ul"]),
                          "diluent_ul": ul(p["diluent_ul"]), "mix_reps": p["mix_reps"]}}
    if key:
        request["idempotency_key"] = key
    return client.call("commands.submit", {"request": request}, timeout=timeout)


def native(params: dict) -> list[str]:
    """The same simulation without OpenMHS, for comparison and replay."""
    from opentrons import simulate

    module = types.ModuleType("native_protocol")
    exec(compile(PROTOCOL.read_bytes(), str(PROTOCOL), "exec"), module.__dict__)
    ctx = simulate.get_protocol_api("2.20", robot_type="Flex")
    module.dilute(ctx, params)
    return list(ctx.commands())


# ------------------------------------------------------------------ scenarios


def ot01(lab: OtLab, check: Check) -> dict:
    agent = lab.agent("agent:ot", GRANTS)
    r = run(agent, "r-ot01")
    sim = lab.sim()
    result = (r.get("adapter_result") or {}).get("result") or {}
    evidence = json.loads((lab.evidence / "r-ot01.json").read_text())
    check("receipt VERIFIED", r["state"] == "VERIFIED", r["state"])
    check("simulator ran once", sim["count"] == 1, sim)
    check("the receipt carries the reviewed protocol's digest",
          result.get("protocol_sha256") == sha256(PROTOCOL.read_bytes()), result.get("protocol_sha256"))
    check("the run log in the receipt is the one on disk",
          result.get("runlog_sha256") == sha256(json.dumps(evidence["commands"]).encode()), result)
    expected = native(DEFAULTS)
    check("same commands as the simulator run without OpenMHS", evidence["commands"] == expected,
          (len(evidence["commands"]), len(expected)))
    return {"receipt": summary(r), "simulator": sim, "commands": len(expected)}


def ot02(lab: OtLab, check: Check) -> dict:
    agent = lab.agent("agent:ot", GRANTS)
    bad = {
        "transfer 400 uL (simulator accepts it)": dict(transfer_ul=400.0),
        "negative transfer (simulator accepts it)": dict(transfer_ul=-5.0),
        "14 wells (the protocol would silently use 12)": dict(n_wells=14),
        "diluent 190 uL (overfills; simulator accepts it)": dict(diluent_ul=190.0),
        "an unreviewed protocol name": dict(protocol="anything_else"),
    }
    receipts = {name: run(agent, f"r-bad-{i}", **over) for i, (name, over) in enumerate(bad.items())}
    for name, r in receipts.items():
        check(f"{name}: DENIED before dispatch", r["state"] == "DENIED" and not reached_device(r),
              (r["state"], codes(r)))
    # The protocol file is edited after review: the gateway refuses before dispatch.
    original = lab.protocol.read_bytes()
    lab.protocol.write_bytes(original + b"\n# an unreviewed edit\n")
    tampered = run(agent, "r-tampered")
    check("edited protocol: DENIED by the reviewed-digest precondition",
          tampered["state"] == "DENIED" and "command.precondition.failed" in codes(tampered),
          (tampered["state"], codes(tampered)))
    lab.protocol.write_bytes(original)
    restored = run(agent, "r-restored")
    check("restored protocol runs", restored["state"] == "VERIFIED", restored["state"])
    check("only the restored run reached the simulator", lab.sim()["count"] == 1, lab.sim())
    return {"denied": {k: summary(v) for k, v in receipts.items()}, "tampered": summary(tampered)}


def ot03(lab: OtLab, check: Check) -> dict:
    held_grants = [GRANTS[0], {**GRANTS[1], "id": "run-needs-approval", "effect": "require_approval"}]
    agent = lab.agent("agent:ot", held_grants)
    held = run(agent, "r-approve")
    check("held for approval, not run", held["state"] == "AWAITING_APPROVAL" and lab.sim()["count"] == 0,
          (held["state"], lab.sim()["count"]))
    approved = lab.operator.call("commands.approve", {"command_id": held["command_id"]}, timeout=60)
    check("approved by the operator: run and VERIFIED", approved["state"] == "VERIFIED", approved["state"])
    other = run(agent, "r-cancel")
    cancelled = lab.operator.call("commands.cancel", {"command_id": other["command_id"]})
    check("a held run that is cancelled never runs", cancelled["state"] == "CANCELLED"
          and not reached_device(cancelled) and lab.sim()["count"] == 1, (cancelled["state"], lab.sim()))
    approval = (approved.get("policy") or {})
    return {"held": summary(held), "approved": summary(approved), "cancelled": summary(cancelled),
            "approved_by": approval.get("approved_by") or approval.get("approval")}


def ot04(lab: OtLab, check: Check) -> dict:
    agent = lab.agent("agent:ot", GRANTS)
    lab.fault("apply_then_hang")
    t = time.perf_counter()
    lost = run(agent, "r-lost")
    waited = (time.perf_counter() - t) * 1000
    check("the simulator completed the run", lab.sim()["last_id"] == "r-lost", lab.sim())
    check("receipt is UNKNOWN_OUTCOME", lost["state"] == "UNKNOWN_OUTCOME", lost["state"])
    held = run(agent, "r-next")
    check("the next run is held, not sent", held["state"] == "DENIED" and not reached_device(held),
          (held["state"], codes(held)))
    reconciled = lab.operator.call("commands.reconcile", {"command_id": lost["command_id"]})
    check("RECONCILED_APPLIED from the simulator's record", reconciled["state"] == "RECONCILED_APPLIED",
          reconciled["state"])
    check("run once, never repeated", lab.sim()["count"] == 1, lab.sim())
    return {"lost": summary(lost), "held": summary(held), "reconciled": summary(reconciled),
            "waited_ms": round(waited)}


def ot05(lab: OtLab, check: Check) -> dict:
    agent = lab.agent("agent:ot", GRANTS)
    first, again = run(agent, "r-dup", key="k1"), run(agent, "r-dup", key="k1")
    check("same key, same command, one run", again["command_id"] == first["command_id"]
          and lab.sim()["count"] == 1, (first["command_id"], again["command_id"], lab.sim()))
    reused = run(agent, "r-dup", key="k2")
    check("same run_id under a new key: REJECTED, nothing run", reused["state"] == "REJECTED"
          and lab.sim()["count"] == 1, (reused["state"], lab.sim()))
    return {"first": summary(first), "again": summary(again), "reused": summary(reused)}


def ot06(lab: OtLab, check: Check) -> dict:
    agent = lab.agent("agent:ot", GRANTS)
    params = {"n_wells": 8, "transfer_ul": 40.0, "diluent_ul": 80.0, "mix_reps": 2}
    r = run(agent, "r-evidence", **params)
    bundle = lab.export_evidence(lab.dir / "evidence-bundle.json")
    check("exported evidence verifies offline, valid and complete",
          bundle["exit"] == 0 and bundle.get("valid") and bundle.get("complete"), bundle)
    exported = json.loads((lab.dir / "evidence-bundle.json").read_text())
    receipt = next(x for x in exported["receipts"] if x["command_id"] == r["command_id"])
    result = receipt["adapter_result"]["result"]
    run_file = json.loads((lab.evidence / "r-evidence.json").read_text())
    replay = native(params)
    check("replay: the same protocol and parameters give the same command log",
          sha256(json.dumps(replay).encode()) == result["runlog_sha256"], len(replay))
    check("the evidence names protocol, digest, parameters and the gateway command",
          run_file["protocol_sha256"] == result["protocol_sha256"] and run_file["params"] == params
          and run_file["command_id"] == r["command_id"], run_file["params"])
    return {"receipt": summary(r), "evidence": {"protocol_sha256": result["protocol_sha256"],
            "runlog_sha256": result["runlog_sha256"], "commands": result["commands"],
            "requested": receipt["request"]["params"], "offline_verify": bundle}}


def ot07(lab: OtLab, check: Check) -> dict:
    agent = lab.agent("agent:ot", GRANTS)
    lab.fault("apply_then_hang")
    threading.Thread(target=lambda: _quiet(lambda: run(agent, "r-crash")), daemon=True).start()
    deadline = time.time() + 60
    while lab.sim()["last_id"] != "r-crash" and time.time() < deadline:
        time.sleep(0.05)
    command_id = next(c["command_id"] for c in lab.operator.call("commands.list", {"limit": 5})
                      if c["request"]["params"]["run_id"] == "r-crash")
    lab.kill()
    count = lab.sim()["count"]
    lab.start()
    recovered = lab.operator.call("commands.get", {"command_id": command_id})
    check("after the restart: UNKNOWN_OUTCOME, not re-run", recovered["state"] == "UNKNOWN_OUTCOME"
          and lab.sim()["count"] == count, (recovered["state"], lab.sim()))
    reconciled = lab.operator.call("commands.reconcile", {"command_id": command_id})
    check("RECONCILED_APPLIED", reconciled["state"] == "RECONCILED_APPLIED", reconciled["state"])
    return {"recovered": summary(recovered), "reconciled": summary(reconciled)}


def _quiet(fn):
    try:
        fn()
    except Exception:
        pass


def perf(lab: OtLab, check: Check, n: int = 20) -> dict:
    agent = lab.agent("agent:ot", GRANTS)
    gov, nat = [], []
    for i in range(n):
        t = time.perf_counter()
        r = run(agent, f"r-perf-{i}")
        gov.append((time.perf_counter() - t) * 1000)
        check(f"run {i} VERIFIED", r["state"] == "VERIFIED", r["state"])
        t = time.perf_counter()
        native(DEFAULTS)
        nat.append((time.perf_counter() - t) * 1000)
    q = lambda xs: {"median": round(statistics.median(xs), 1), "p95": round(sorted(xs)[int(0.95 * (n - 1))], 1),
                    "max": round(max(xs), 1)}
    return {"n": n, "openmhs_ms": q(gov), "native_ms": q(nat), "gateway_start_ms": round(lab.start_ms),
            "journal_bytes": (lab.home / "journal.sqlite").stat().st_size}


SCENARIOS = {
    "OT-01": ("a valid serial dilution, simulated", ot01, OtLab),
    "OT-02": ("invalid parameters and an edited protocol", ot02, OtLab),
    "OT-03": ("approval before execution", ot03, OtLab),
    "OT-04": ("run completed, response lost", ot04, OtLab),
    "OT-05": ("duplicate submission", ot05, OtLab),
    "OT-06": ("evidence export and replay", ot06, OtLab),
    "OT-07": ("gateway killed after the run, restarted", ot07, OtLab),
    "PERF": ("latency, governed vs native", perf, OtLab),
}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="results.json")
    ap.add_argument("--only", nargs="*")
    ap.add_argument("--work")
    a = ap.parse_args()
    import opentrons

    run_all(SCENARIOS, {"opentrons": opentrons.__version__,
                        "protocol_sha256": sha256(PROTOCOL.read_bytes())}, a.out, a.only, a.work)


if __name__ == "__main__":
    main()
