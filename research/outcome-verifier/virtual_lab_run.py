"""EVALUATION.md Q4 and Q5: the verifier on OpenMHS's own evidence, from the virtual lab.

For each case: a fresh `openmhs serve --sim`, a scoped agent, the simulator's own fault, a setpoint
write of 2 V with a correlation id, reads of the measured output, `openmhs audit export`, then
`openmhs outcome verify` on the export. Nothing is hand-edited between the gateway and the verifier.

    python research/outcome-verifier/virtual_lab_run.py --out D:/tmp/outcome-vlab

Everything is simulated. The supply's "measured" output is the simulator's model, so where it disagrees
with the setpoint it is because the model says so. The predictions below were written before the first run.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(REPO / "python"))

from openmhs import Client, Quantity  # noqa: E402

PSU = "sim.psu-01"
SETPOINT = "power.output.voltage.setpoint"
MEASURED = "power.output.voltage.measured"
ENABLED = "power.output.enabled"

# name: (output enabled, simulator fault, harness DMM, predicted receipt state, predicted verdict)
CASES = {
    "output_on": (True, None, None, "VERIFIED", "SATISFIED"),
    "output_off": (False, None, None, "VERIFIED", "VIOLATED"),
    "setpoint_readback_offset": (True, "readback-offset=0.3", None, "FAILED", "SATISFIED"),
    "readings_stale_5s": (True, "stale=5000", None, "not VERIFIED", "UNKNOWN"),
    "readings_bad_quality": (True, "bad-quality", None, "not VERIFIED", "UNKNOWN"),
    "applied_then_hung": (True, "apply-then-hang", None, "UNKNOWN_OUTCOME", "SATISFIED"),
    "with_a_simulated_dmm": (True, None, 1.999, "VERIFIED", "SATISFIED"),
}


def binary() -> str:
    if os.environ.get("OPENMHS_BIN"):
        return os.environ["OPENMHS_BIN"]
    target = Path(os.environ.get("CARGO_TARGET_DIR", REPO / "target"))
    exe = target / "debug" / ("openmhs.exe" if sys.platform == "win32" else "openmhs")
    return str(exe) if exe.exists() else (shutil.which("openmhs") or "openmhs")


def contract(attempt: str, with_dmm: bool) -> dict:
    c = {
        "contract": "psu-output-2v", "version": "0.1",
        "binding": {"correlation": attempt},
        "timing": {"anchor": "command", "default_window_ms": [0, 3000]},
        "evidence": {
            "command": {"signal": f"command:{SETPOINT}", "kind": "controller", "device": PSU},
            "output": {"signal": MEASURED, "kind": "device_state", "device": PSU, "bound": False},
        },
        "conditions": [
            {"name": "the output measures 2 V", "evidence": "output", "op": "between",
             "min": {"value": 1.95, "unit": "V"}, "max": {"value": 2.05, "unit": "V"}},
        ],
    }
    if with_dmm:
        c["evidence"]["dmm"] = {"signal": "output.voltage", "kind": "independent_sensor", "calibration": ["dc-voltage"]}
        c["conditions"].append({"name": "the DMM measures 2 V", "evidence": "dmm", "op": "between",
                                "min": {"value": 1.95, "unit": "V"}, "max": {"value": 2.05, "unit": "V"}})
    return c


def dmm_records(attempt: str, volts: float) -> list[dict]:
    """A simulated DMM on this host, so on the gateway's clock. Labelled a simulator, because it is one."""
    now = time.time()
    stamp = time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime(now)) + f".{int(now * 1000) % 1000:03d}Z"
    return [
        {"id": "dmm-1", "signal": "output.voltage", "value": {"value": volts, "unit": "V"},
         "source": {"system": "harness", "device": "dmm-1", "kind": "independent_sensor", "provenance": "simulator"},
         "time": {"source": stamp, "clock_domain": "openmhs-gateway"}, "refs": {"correlation": attempt}},
        {"id": "cal-dmm-1", "signal": "calibration", "value": "dc-voltage",
         "source": {"system": "harness", "device": "dmm-1", "kind": "calibration", "provenance": "simulator"},
         "calibration": {"name": "dc-voltage", "applies_to": "dmm-1",
                         "valid_from": "2026-01-01T00:00:00Z", "valid_until": "2027-12-31T00:00:00Z"}},
    ]


def run_case(name: str, spec: tuple, out: Path) -> dict:
    enabled, fault, dmm_volts, predicted_state, predicted_verdict = spec
    attempt = f"attempt-{name}"
    case_dir = out / name
    case_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="openmhs-outcome-") as tmp:
        home = Path(tmp) / "home"
        log = open(Path(tmp) / "gateway.log", "wb")
        gw = subprocess.Popen([binary(), "--home", str(home), "serve", "--sim", "--port", "0"], stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 30
            while not (home / "gateway.json").exists():
                if gw.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError("gateway did not start")
                time.sleep(0.05)
            time.sleep(0.2)
            op = Client.from_home(home)
            op.add_grant({"id": "agent-psu", "principal": "agent:*", "device": PSU, "capability": "*",
                          "operations": ["read", "write"], "effect": "allow"})
            token = op.create_token("agent:outcome", scopes=[{"device": PSU, "capability": "*",
                                                              "operations": ["read", "write"]}]).token
            agent = Client.from_home(home, token=token)
            agent.write(PSU, ENABLED, enabled, correlation_id=f"setup-{name}")
            if fault:
                op.sim_fault(PSU, fault)
            try:
                receipt = agent.write(PSU, SETPOINT, Quantity(2.0, "V"), correlation_id=attempt, deadline_ms=3000)
                state = str(getattr(receipt.state, "value", receipt.state))
            except Exception as e:  # noqa: BLE001 - a refusal is a result here
                state = f"error {getattr(e, 'code', type(e).__name__)}"
            for _ in range(2):
                time.sleep(0.15)
                try:
                    agent.read(PSU, MEASURED)
                except Exception:  # noqa: BLE001, S110 - a failed read is audited, not evidence
                    pass
            bundle = case_dir / "audit_export.json"
            if bundle.exists():
                bundle.unlink()
            subprocess.run([binary(), "--home", str(home), "audit", "export", "--out", str(bundle)],
                           check=True, capture_output=True)
        finally:
            gw.terminate()
            gw.wait(timeout=15)
            log.close()
    (case_dir / "contract.json").write_text(json.dumps(contract(attempt, dmm_volts is not None), indent=2) + "\n",
                                            encoding="utf-8")
    args = [binary(), "--json", "outcome", "verify", "--contract", str(case_dir / "contract.json"),
            "--audit", str(bundle)]
    if dmm_volts is not None:
        dmm_file = case_dir / "dmm.jsonl"
        dmm_file.write_text("".join(json.dumps(r) + "\n" for r in dmm_records(attempt, dmm_volts)), encoding="utf-8")
        args += ["--evidence", str(dmm_file)]
    result = subprocess.run(args, capture_output=True, text=True)
    (case_dir / "report.json").write_text(result.stdout, encoding="utf-8")
    report = json.loads(result.stdout) if result.stdout.strip().startswith("{") else {}
    return {
        "case": name, "fault": fault, "output_enabled": enabled,
        "receipt_state": state, "predicted_receipt_state": predicted_state,
        "verdict": report.get("verdict"), "predicted_verdict": predicted_verdict,
        "exit": result.returncode, "reasons": sorted({r["code"] for r in report.get("reasons", [])}),
        "observed_output": [c["observed"] for c in report.get("conditions", [])],
        "stderr": result.stderr.strip()[-300:],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    rows = []
    for name, spec in CASES.items():
        row = run_case(name, spec, args.out)
        rows.append(row)
        print(f"{name:28} receipt {row['receipt_state']:16} verdict {row['verdict']} {row['reasons']}", flush=True)
    (args.out / "summary.json").write_text(json.dumps(rows, indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
