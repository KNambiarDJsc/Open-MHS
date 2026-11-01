"""MM-01..MM-07 and a latency run: Micro-Manager's demo microscope through OpenMHS.

Each scenario starts a real gateway with the Micro-Manager adapter (adapters/python/
micromanager) as its child. Checks compare receipts with the adapter's evidence (each image
saved with its provenance record) and its state file. Images are synthetic (DemoCamera).

    $env:OPENMHS_BIN = "...\\openmhs.exe"
    <venv-with-pymmcore-plus>\\python run_scenarios.py --mm <dir with the demo adapters> [--out ...]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import statistics
import sys
import threading
import time
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[2] / "shared"))
from harness import REPO, Check, Lab, codes, reached_device, run_all, summary  # noqa: E402

ADAPTER = REPO / "adapters" / "python" / "micromanager" / "mm_adapter.py"
DEVICE = "mm.scope-01"
MM_DIR: Path = Path()
GRANTS = [
    {"id": "read", "device": DEVICE, "capability": "*", "operations": ["read"], "effect": "allow"},
    {"id": "write", "device": DEVICE, "capability": "*", "operations": ["write"], "effect": "allow"},
    {"id": "snap", "device": DEVICE, "capability": "camera.snap", "operations": ["invoke"], "effect": "allow"},
]


class MmLab(Lab):
    def __init__(self, root: Path):
        root.mkdir(parents=True)
        self.state_file, self.fault_file, self.evidence = root / "mm_state.json", root / "mm_fault.json", root / "evidence"
        super().__init__(root, {
            "openmhs_manifest": "0.1",
            "adapter": {"id": "openmhs.py.micromanager", "version": "0.1.0"},
            "runtime": {"command": [sys.executable, str(ADAPTER)],
                        "env": {"PYTHONPATH": str(REPO / "python")}, "protocol": ["0.1"]},
            "capabilities": {"allow": [{"name": "*", "operations": ["read", "write"]},
                                       {"name": "camera.snap", "operations": ["invoke"]}]},
            "filesystem": {"read": [str(MM_DIR)], "write": [str(root)]},
            "limits": {"start_timeout_ms": 60000, "call_timeout_ms": 10000},
            "config": {"adapter_dir": str(MM_DIR), "state_file": str(self.state_file),
                       "fault_file": str(self.fault_file), "evidence_dir": str(self.evidence)},
        })

    def fault(self, name: str) -> None:
        self.fault_file.write_text(json.dumps({"next": name}))

    def sim(self) -> dict:
        return json.loads(self.state_file.read_text()) if self.state_file.exists() else {}


def um(v):
    return {"value": v, "unit": "um"}


def write(client, cap, value, timeout=30):
    return client.call("commands.submit", {"request": {"target": {"device": DEVICE, "capability": cap},
                                                       "operation": "write", "value": value}}, timeout=timeout)


def snap(client, image_id, timeout=30):
    return client.call("commands.submit", {"request": {"target": {"device": DEVICE, "capability": "camera.snap"},
                                                       "operation": "invoke", "params": {"image_id": image_id}}},
                       timeout=timeout)


def value(client, cap):
    v = client.read(DEVICE, cap).to_dict()["state"]["value"]
    return v["value"] if isinstance(v, dict) else v


def provenance_ok(lab: MmLab, r: dict) -> tuple[bool, dict]:
    """The image on disk, its provenance record and the receipt agree."""
    image_id = r["request"]["params"]["image_id"]
    record = json.loads((lab.evidence / f"{image_id}.json").read_text())
    data = np.load(lab.evidence / f"{image_id}.npy")
    file_sha = hashlib.sha256(np.ascontiguousarray(data).tobytes()).hexdigest()
    result = r["adapter_result"]["result"]
    ok = record["sha256"] == file_sha == result["sha256"] and record["command_id"] == r["command_id"]
    return ok, record


# ------------------------------------------------------------------ scenarios


def mm01(lab: MmLab, check: Check) -> dict:
    """A 2 x 2 grid, two channels per position: 12 moves or settings, 8 images."""
    agent = lab.agent("agent:mm", GRANTS)
    check("exposure 20 ms", write(agent, "camera.exposure", {"value": 20, "unit": "ms"})["state"] == "VERIFIED")
    check("focus at 10 um", write(agent, "stage.z.position", um(10.0))["state"] == "VERIFIED")
    images, receipts = [], []
    for x in (0.0, 250.0):
        for y in (0.0, 250.0):
            for cap, v in (("stage.x.position", x), ("stage.y.position", y)):
                r = write(agent, cap, um(v))
                receipts.append(r)
                check(f"move {cap}={v} VERIFIED", r["state"] == "VERIFIED", (r["state"], r.get("verification")))
            for channel in ("FITC", "DAPI"):
                r = write(agent, "channel", channel)
                check(f"channel {channel} VERIFIED", r["state"] == "VERIFIED", (r["state"], r.get("verification")))
                s = snap(agent, f"img-{x:g}-{y:g}-{channel}")
                ok, record = provenance_ok(lab, s) if s["state"] == "VERIFIED" else (False, {})
                state = record.get("software_state", {})
                check(f"image at ({x:g},{y:g}) {channel}: VERIFIED and provenance agrees",
                      s["state"] == "VERIFIED" and ok and abs(state.get("x", 1e9) - x) <= 0.5
                      and abs(state.get("y", 1e9) - y) <= 0.5 and state.get("channel") == channel,
                      (s["state"], state))
                images.append({"image_id": record.get("image_id"), "sha256": record.get("sha256"),
                               "command_id": s["command_id"], "software_state": state})
    distinct = len({i["sha256"] for i in images})
    check("8 images, 8 distinct contents", len(images) == 8 and distinct == 8, distinct)
    check("the camera counted 8 images", lab.sim().get("images") == 8, lab.sim().get("images"))
    return {"images": images}


def mm02(lab: MmLab, check: Check) -> dict:
    agent = lab.agent("agent:mm", GRANTS)
    x0 = value(agent, "stage.x.position")
    bad = {
        "x = 6000 um (envelope 5000)": ("stage.x.position", um(6000.0)),
        "x = 1e9 um (the demo stage would try)": ("stage.x.position", um(1e9)),
        "z = -5 um": ("stage.z.position", um(-5.0)),
        "exposure 5 s": ("camera.exposure", {"value": 5000, "unit": "ms"}),
        "channel UV (not a preset)": ("channel", "UV"),
        "x in an unconvertible unit": ("stage.x.position", {"value": 1, "unit": "V"}),
    }
    out = {}
    for name, (cap, v) in bad.items():
        r = write(agent, cap, v)
        out[name] = summary(r)
        check(f"{name}: DENIED before dispatch", r["state"] == "DENIED" and not reached_device(r), (r["state"], codes(r)))
    check("the stage never moved", value(agent, "stage.x.position") == x0)
    return out


def mm03(lab: MmLab, check: Check) -> dict:
    a, b = lab.agent("agent:a", GRANTS), lab.agent("agent:b", GRANTS)
    # Without a lease, each command is serialized but a sequence is not: B moves between
    # A's move and A's snap, and A's image is taken where B left the stage.
    write(a, "stage.x.position", um(100.0))
    write(b, "stage.x.position", um(2000.0))
    s = snap(a, "img-interleaved")
    _, record = provenance_ok(lab, s)
    check("without a lease, A's image is where B left the stage, and its provenance says so",
          s["state"] == "VERIFIED" and abs(record["software_state"]["x"] - 2000.0) <= 0.5,
          record["software_state"]["x"])
    # With a lease A's sequence is exclusive: B is refused, reads stay open.
    lease = a.acquire_lease(ttl_ms=30000, devices=[DEVICE], reason="grid acquisition").to_dict()
    moved = write(a, "stage.x.position", um(100.0))
    blocked = write(b, "stage.x.position", um(2000.0))
    s2 = snap(a, "img-leased")
    _, record2 = provenance_ok(lab, s2)
    check("B is refused while A holds the lease", blocked["state"] == "DENIED" and "lease.held" in codes(blocked)
          and not reached_device(blocked), (blocked["state"], codes(blocked)))
    check("B can still read", value(b, "stage.x.position") is not None)
    check("A's leased image is where A put the stage", moved["state"] == "VERIFIED"
          and abs(record2["software_state"]["x"] - 100.0) <= 0.5, record2["software_state"]["x"])
    a.release_lease(lease["id"])
    after = write(b, "stage.x.position", um(2000.0))
    check("after release B moves", after["state"] == "VERIFIED", after["state"])
    return {"interleaved_x": record["software_state"]["x"], "leased_x": record2["software_state"]["x"],
            "blocked": summary(blocked)}


def mm04(lab: MmLab, check: Check) -> dict:
    agent = lab.agent("agent:mm", GRANTS)
    lab.fault("camera_offline")  # applied by the next command the adapter executes
    write(agent, "camera.exposure", {"value": 15, "unit": "ms"})
    check("the adapter reports the camera unavailable", value(agent, "camera.available") is False)
    s = snap(agent, "img-no-camera")
    check("snap DENIED by the camera precondition, never dispatched", s["state"] == "DENIED"
          and "command.precondition.failed" in codes(s) and not reached_device(s), (s["state"], codes(s)))
    moved = write(agent, "stage.x.position", um(42.0))
    check("the stage still works without the camera", moved["state"] == "VERIFIED", moved["state"])
    lab.fault("camera_online")
    write(agent, "camera.exposure", {"value": 20, "unit": "ms"})
    s2 = snap(agent, "img-camera-back")
    check("camera back: snap VERIFIED", s2["state"] == "VERIFIED", s2["state"])
    return {"denied": summary(s), "restored": summary(s2)}


def mm05(lab: MmLab, check: Check) -> dict:
    agent = lab.agent("agent:mm", GRANTS)
    # (a) The move happens, the answer is lost, and it is reconciled straight away.
    lab.fault("apply_then_hang")
    lost = write(agent, "stage.x.position", um(500.0))
    check("the stage moved", abs(lab.sim()["devices"]["x"] - 500.0) <= 0.5, lab.sim()["devices"])
    check("UNKNOWN_OUTCOME", lost["state"] == "UNKNOWN_OUTCOME", lost["state"])
    rec = lab.operator.call("commands.reconcile", {"command_id": lost["command_id"]})
    check("reconciled straight away: RECONCILED_APPLIED", rec["state"] == "RECONCILED_APPLIED", rec["state"])
    # (b) Another lost move. A position is idempotent, so a new move is allowed before it is
    # reconciled (spec/lifecycle.md section 4, rule 3), and it supersedes the lost one.
    lab.fault("apply_then_hang")
    lost2 = write(agent, "stage.x.position", um(700.0))
    applied_700 = abs(lab.sim()["devices"]["x"] - 700.0) <= 0.5
    newer = write(agent, "stage.x.position", um(800.0))
    check("a new absolute move is allowed while the first is unknown (idempotent, as specified)",
          lost2["state"] == "UNKNOWN_OUTCOME" and newer["state"] == "VERIFIED", (lost2["state"], newer["state"]))
    rec2 = lab.operator.call("commands.reconcile", {"command_id": lost2["command_id"]})
    check("FINDING: the superseded move reconciles NOT_APPLIED although the stage did reach 700 um",
          applied_700 and rec2["state"] == "RECONCILED_NOT_APPLIED", (applied_700, rec2["state"]))
    return {"lost": summary(lost), "reconciled": summary(rec), "lost_then_superseded": summary(lost2),
            "superseding": summary(newer), "reconciled_after_supersede": summary(rec2)}


def mm06(lab: MmLab, check: Check) -> dict:
    agent = lab.agent("agent:mm", GRANTS)
    write(agent, "stage.x.position", um(123.0))
    write(agent, "channel", "Cy5")
    s = snap(agent, "img-provenance")
    ok, record = provenance_ok(lab, s)
    check("image, provenance record and receipt agree", ok, record.get("sha256"))
    bundle = lab.export_evidence(lab.dir / "evidence-bundle.json")
    check("exported evidence verifies offline", bundle["exit"] == 0 and bundle.get("valid") and bundle.get("complete"),
          bundle)
    exported = json.loads((lab.dir / "evidence-bundle.json").read_text())
    receipt = next(x for x in exported["receipts"] if x["command_id"] == s["command_id"])
    check("the exported receipt holds the image's SHA-256", receipt["adapter_result"]["result"]["sha256"]
          == record["sha256"])
    # The image file is changed afterwards: its hash no longer matches the receipt.
    path = lab.evidence / "img-provenance.npy"
    data = np.load(path)
    data[0, 0] ^= 1
    np.save(path, data)
    tampered = hashlib.sha256(np.ascontiguousarray(np.load(path)).tobytes()).hexdigest()
    check("an edited image no longer matches its receipt", tampered != receipt["adapter_result"]["result"]["sha256"])
    return {"record": record, "offline_verify": {k: bundle.get(k) for k in ("exit", "valid", "complete")}}


def mm07(lab: MmLab, check: Check) -> dict:
    agent = lab.agent("agent:mm", GRANTS)
    lab.fault("apply_then_hang")
    threading.Thread(target=lambda: _quiet(lambda: write(agent, "stage.y.position", um(-300.0))), daemon=True).start()
    deadline = time.time() + 30
    while time.time() < deadline and abs(lab.sim().get("devices", {}).get("y", 0) + 300.0) > 0.5:
        time.sleep(0.05)
    cid = next(c["command_id"] for c in lab.operator.call("commands.list", {"limit": 5})
               if c["request"]["target"]["capability"] == "stage.y.position")
    lab.kill()
    lab.start()
    rec0 = lab.operator.call("commands.get", {"command_id": cid})
    check("after restart: UNKNOWN_OUTCOME", rec0["state"] == "UNKNOWN_OUTCOME", rec0["state"])
    rec = lab.operator.call("commands.reconcile", {"command_id": cid})
    check("RECONCILED_APPLIED: the restored stage is where the move left it", rec["state"] == "RECONCILED_APPLIED",
          rec["state"])
    return {"recovered": summary(rec0), "reconciled": summary(rec)}


def _quiet(fn):
    try:
        fn()
    except Exception:
        pass


def perf(lab: MmLab, check: Check, n: int = 20) -> dict:
    agent = lab.agent("agent:mm", GRANTS)
    gov = []
    for i in range(n):
        t = time.perf_counter()
        m = write(agent, "stage.x.position", um(float(i * 10)))
        s = snap(agent, f"img-perf-{i}")
        gov.append((time.perf_counter() - t) * 1000)
        check(f"position {i}: move and image VERIFIED", m["state"] == s["state"] == "VERIFIED")
    from pymmcore_plus import CMMCorePlus

    core = CMMCorePlus()
    core.setDeviceAdapterSearchPaths([str(MM_DIR)])
    core.loadSystemConfiguration(str(MM_DIR / "MMConfig_demo.cfg"))
    core.setExposure(10)
    nat = []
    for i in range(n):
        t = time.perf_counter()
        core.setXYPosition(float(i * 10), 0.0)
        core.waitForDevice(core.getXYStageDevice())
        core.snap()
        nat.append((time.perf_counter() - t) * 1000)
    q = lambda xs: {"median": round(statistics.median(xs), 1), "p95": round(sorted(xs)[int(0.95 * (n - 1))], 1),
                    "max": round(max(xs), 1)}
    return {"n": n, "openmhs_ms": q(gov), "native_ms": q(nat), "gateway_start_ms": round(lab.start_ms),
            "note": "governed: one verified move and one verified snap with provenance; native: setXYPosition + snap"}


SCENARIOS = {
    "MM-01": ("multi-position, two-channel imaging", mm01, MmLab),
    "MM-02": ("out-of-contract stage, exposure and channel", mm02, MmLab),
    "MM-03": ("two agents: commands serialized, sequences only under a lease", mm03, MmLab),
    "MM-04": ("camera unavailable", mm04, MmLab),
    "MM-05": ("move done, response lost", mm05, MmLab),
    "MM-06": ("image-to-command provenance and evidence", mm06, MmLab),
    "MM-07": ("gateway killed after a move, restarted", mm07, MmLab),
    "PERF": ("latency per position, governed vs native", perf, MmLab),
}


def main() -> None:
    global MM_DIR
    ap = argparse.ArgumentParser()
    ap.add_argument("--mm", required=True, help="directory with MMConfig_demo.cfg and the demo adapters")
    ap.add_argument("--out", default="results.json")
    ap.add_argument("--only", nargs="*")
    ap.add_argument("--work")
    a = ap.parse_args()
    MM_DIR = Path(a.mm).resolve()
    import pymmcore_plus

    run_all(SCENARIOS, {"pymmcore_plus": pymmcore_plus.__version__, "micro_manager": MM_DIR.name},
            a.out, a.only, a.work)


if __name__ == "__main__":
    main()
