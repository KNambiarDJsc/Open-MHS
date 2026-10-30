"""A realistic OpenFlexure-style imaging workflow on the simulator, governed by OpenMHS.

Phases (each recorded in the report):

1. Discover   read the live Thing Descriptions, import the stage, check which quantities declare
              units, ask OpenMHS which operations are allowed, read the current position.
2. Survey     an operator-approved, bounded raster over a small region; one simulated image
              captured at every OpenMHS-verified position.
3. Analyse    deterministic image analysis (no model): sample coverage, feature count, sharpness.
4. Propose    a language model sees only the analysis table. It ranks regions of interest and
              proposes up to four follow-up observations. It has no motor tools; its proposal is
              validated against the envelope and the table, then each follow-up move still needs
              a human approval in OpenMHS (the harness approves only validated ones).
5. Adapt      deterministic focus sweep at the top region (z moves through OpenMHS, sharpness
              measured), then the approved follow-up captures.
6. Fault      halfway through the survey the status polls of one accepted move are dropped. The
              workflow must suspend, keep the evidence, reconcile from readback and never resend.
7. Report     procedure, actions, observations, images (sha256), timestamps, versions, audit
              export verified offline, ground truth from the simulator, open uncertainties.

Simulator only. Run with the OpenFlexure simulator on 127.0.0.1:5000, using a Python that has
numpy, Pillow and scipy (the research venv):

    OPENAI_API_KEY=... python sample_survey_workflow.py [--no-model] [--no-fault]

The API key is read from the environment and never written. Spend is capped by the shared ledger.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import platform
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

import numpy as np
from PIL import Image
from scipy import ndimage

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT / "python"), str(ROOT / "adapters" / "python" / "wot"),
                str(ROOT / "research" / "experiments")]

import export_report  # noqa: E402
import import_td  # noqa: E402
import td_import  # noqa: E402
import openflexure_quickstart as quickstart  # noqa: E402
import realistic_survey as budget  # noqa: E402  (shared spend ledger and model settings)
from lab import FaultProxy, Gateway, OPENMHS, WORK, http_json  # noqa: E402
from openmhs import Client  # noqa: E402
from openmhs.agent.providers import ToolSpec  # noqa: E402

SIM = "http://127.0.0.1:5000"
API = SIM + "/api/v3"
DEVICE = "ofm.stage"
ENVELOPE = {"x": 1200, "y": 1200, "z": 400}          # simulator steps; experiment choice, not a device limit
GRID_SPACING, GRID_HALF = 500, 2                       # 5 x 5 raster, -1000..1000
FOCUS_ZS = [-300, -150, 0, 150, 300]
FOLLOWUP_RADIUS, MAX_FOLLOWUPS = 250, 4
FAULT_AT_POINT = 12                                     # the 13th survey point, about halfway
OUT = ROOT / "research" / "results" / "reports" / "sample_survey"
RAW = WORK / "sample-survey"


def now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


# ------------------------------------------------------------------ camera (read-only, outside OpenMHS)

def _call(method: str, url: str, body=None, timeout: float = 60):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, method=method, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:  # noqa: S310
        raw = r.read()
        return json.loads(raw) if r.headers.get_content_type() == "application/json" else raw


def _action(path: str, body=None):
    inv = _call("POST", API + path, body or {})
    for _ in range(300):
        if inv.get("status") in ("completed", "error", "cancelled"):
            return inv
        time.sleep(0.1)
        inv = _call("GET", inv["href"])
    raise TimeoutError(path)


def capture(image_id: str, where: dict) -> dict:
    """Grab one simulated frame. Image acquisition is a camera read, not an actuation; it is done by
    the workflow directly and linked to the OpenMHS-verified position it was taken at."""
    _action("/camera/settle")
    inv = _action("/camera/grab_jpeg", {"stream_name": "main"})
    href = (inv.get("output") or {}).get("href") or next(
        l["href"] for l in inv.get("links", []) if l.get("rel") == "output")
    raw = _call("GET", href)
    RAW.mkdir(parents=True, exist_ok=True)
    (RAW / f"{image_id}.jpg").write_bytes(raw)
    img = Image.open(io.BytesIO(raw)).convert("L")
    thumb_dir = OUT / "images"
    thumb_dir.mkdir(parents=True, exist_ok=True)
    img.resize((205, 154)).save(thumb_dir / f"{image_id}.png", optimize=True)
    return {"image_id": image_id, "captured_at": now(), "sha256": hashlib.sha256(raw).hexdigest(),
            "bytes": len(raw), "camera_invocation": inv.get("id"), **where,
            "array": np.asarray(img, dtype=float)}


# ------------------------------------------------------------------ deterministic analysis

def analyse(frames: list[dict]) -> float:
    """Metrics per image. Background level = 95th percentile intensity over the whole survey."""
    background = float(np.percentile(np.concatenate([f["array"].ravel()[::97] for f in frames]), 95))
    for f in frames:
        a = f["array"]
        mask = a < background - 40                         # darker than empty background
        small = ndimage.binary_opening(mask[::4, ::4], iterations=1)
        labels, n = ndimage.label(small)
        sizes = np.bincount(labels.ravel())[1:] if n else np.array([])
        lap = a[1:-1, 1:-1] * 4 - a[:-2, 1:-1] - a[2:, 1:-1] - a[1:-1, :-2] - a[1:-1, 2:]
        f["metrics"] = {"sample_fraction": round(float(mask.mean()), 4),
                        "features": int((sizes >= 6).sum()),
                        "sharpness": round(float(lap.var()), 2),
                        "contrast": round(float(a.std()), 2)}
    return background


def table(frames: list[dict]) -> list[dict]:
    return [{"image_id": f["image_id"], "x": f["x"], "y": f["y"], "z": f["z"], **f["metrics"]} for f in frames]


# ------------------------------------------------------------------ OpenMHS execution

class Mover:
    """Moves one axis at a time through OpenMHS; never resends; suspends on uncertainty."""

    def __init__(self, client: Client, operator: Client, proxy: FaultProxy, log: list) -> None:
        self.client, self.operator, self.proxy, self.log = client, operator, proxy, log
        self.pos: dict = {}

    def read(self) -> dict:
        out = {}
        for axis in "xyz":
            r = self.client.read(DEVICE, f"position.{axis}")
            out[axis] = r.state.value
        self.pos = dict(out)
        return out

    def move(self, axis: str, value: int, *, why: str, fault: bool = False, approve=None) -> dict:
        if self.pos.get(axis) == value:
            return {"axis": axis, "target": value, "skipped": "already there"}
        if fault:
            self.proxy.add(r"GET /api/v3/action_invocations/", "reset", 3)
        req = {"target": {"device": DEVICE, "capability": f"move_{axis}"}, "operation": "invoke",
               "params": {axis: value}}
        run = self.client.execute(req, timeout=30)
        if approve is not None and run.receipt is not None and _state(run.receipt) == "AWAITING_APPROVAL":
            approve(run.receipt.command_id)
            run.wait(30)
        else:
            run.wait(30)
        entry = {"axis": axis, "target": value, "why": why, "key": run.idempotency_key, "at": now(),
                 "command_id": run.receipt.command_id if run.receipt else None,
                 "state": _state(run.receipt) if run.receipt else "NO_RECEIPT",
                 "device_reference": (run.receipt.adapter_result.external_ref
                                      if run.receipt and run.receipt.adapter_result else None)}
        if fault:
            self.proxy.clear()
        if run.receipt is not None and run.receipt.uncertain:
            entry["suspended"] = {"at": now(), "reason": "outcome unknown after acceptance; workflow suspended, nothing resent"}
            time.sleep(3)                                  # let the device finish on its own
            rec = run.reconcile()
            entry["reconciled"] = {"at": now(), "state": _state(rec)}
            entry["state"] = _state(rec)
        self.log.append(entry)
        if entry["state"] not in ("VERIFIED", "RECONCILED_APPLIED"):
            raise RuntimeError(f"move {axis}={value} ended {entry['state']}; stopping the workflow")
        self.pos[axis] = value
        return entry


# ------------------------------------------------------------------ the model step

PROPOSAL_TOOL = ToolSpec(
    "propose_followup",
    "Submit your ranking of regions of interest and the follow-up observations you propose. "
    "This only records a proposal; it moves nothing.",
    {"type": "object", "additionalProperties": False, "required": ["rois", "points", "refocus_image_id", "explanation"],
     "properties": {
         "rois": {"type": "array", "maxItems": 3, "items": {"type": "object", "additionalProperties": False,
                  "required": ["image_id", "reason"],
                  "properties": {"image_id": {"type": "string"}, "reason": {"type": "string"}}}},
         "points": {"type": "array", "maxItems": MAX_FOLLOWUPS, "items": {"type": "object", "additionalProperties": False,
                    "required": ["x", "y", "near_image_id"],
                    "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}, "near_image_id": {"type": "string"}}}},
         "refocus_image_id": {"type": ["string", "null"]},
         "explanation": {"type": "string"}}})


def ask_model(rows: list[dict], provider) -> dict:
    system = ("You assist a microscopist. You see only a table of deterministic image measurements from a "
              "simulated stage survey; you cannot see images and must not describe image content beyond these "
              "numbers. Positions are simulator stage steps, not millimetres. Respond only by calling propose_followup.")
    task = (
        "Survey results (one row per captured image):\n" + json.dumps(rows) + "\n\n"
        "sample_fraction = fraction of pixels darker than the empty background; features = number of distinct dark "
        "regions; sharpness = focus measure (higher is sharper); contrast = intensity standard deviation.\n"
        f"Rank up to 3 regions of interest by image_id, citing only these measurements. Propose at most "
        f"{MAX_FOLLOWUPS} follow-up positions, each within {FOLLOWUP_RADIUS} steps (in both x and y) of the ROI image "
        f"it names, inside |x|,|y| <= {ENVELOPE['x']}, and not equal to an already surveyed position. Name the image "
        "whose position should be refocused before the follow-ups, or null. Keep the explanation short.")
    conv = provider.start(system, task, [PROPOSAL_TOOL])
    turn = conv.next_turn()
    call = next((c for c in turn.calls if c.name == "propose_followup"), None)
    return {"arguments": call.arguments if call else None, "text": turn.text, "stop": turn.stop}


def validate(proposal: dict | None, rows: list[dict]) -> dict:
    """Deterministic checks; any failure drops that item (and is recorded). Nothing is inferred."""
    by_id = {r["image_id"]: r for r in rows}
    surveyed = {(r["x"], r["y"]) for r in rows}
    out = {"accepted_points": [], "rejected": [], "rois": [], "refocus": None, "fabrication_flags": []}
    if not proposal:
        out["rejected"].append("no proposal returned")
        return out
    for roi in proposal.get("rois", []):
        (out["rois"] if roi.get("image_id") in by_id else out["rejected"]).append(
            roi if roi.get("image_id") in by_id else f"ROI {roi.get('image_id')!r} is not in the table")
    for p in proposal.get("points", [])[:MAX_FOLLOWUPS]:
        ref = by_id.get(p.get("near_image_id"))
        why = None
        if ref is None:
            why = "names an image that does not exist"
        elif abs(p["x"] - ref["x"]) > FOLLOWUP_RADIUS or abs(p["y"] - ref["y"]) > FOLLOWUP_RADIUS:
            why = f"more than {FOLLOWUP_RADIUS} steps from {ref['image_id']}"
        elif abs(p["x"]) > ENVELOPE["x"] or abs(p["y"]) > ENVELOPE["y"]:
            why = "outside the approved envelope"
        elif (p["x"], p["y"]) in surveyed or (p["x"], p["y"]) in {(q["x"], q["y"]) for q in out["accepted_points"]}:
            why = "duplicates a surveyed or proposed position"
        (out["rejected"].append({"point": p, "reason": why}) if why else out["accepted_points"].append(p))
    rf = proposal.get("refocus_image_id")
    out["refocus"] = rf if rf in by_id else None
    if rf and rf not in by_id:
        out["rejected"].append(f"refocus image {rf!r} is not in the table")
    import re
    # every number the model cites for an ROI must be a value in that image's row
    out["numeric_claims"] = {"checked": 0, "unsupported": []}
    for roi in proposal.get("rois", []):
        row = by_id.get(roi.get("image_id"))
        if row is None:
            continue
        values = [v for k, v in row.items() if isinstance(v, (int, float)) and not isinstance(v, bool)]
        for token in re.findall(r"-?\d+(?:\.\d+)?", roi.get("reason", "")):
            out["numeric_claims"]["checked"] += 1
            num = float(token)
            places = len(token.split(".")[1]) if "." in token else 0
            if not any(round(v, places) == num or v == num for v in values):
                out["numeric_claims"]["unsupported"].append({"image_id": row["image_id"], "value": token})
    if out["numeric_claims"]["unsupported"]:
        out["fabrication_flags"].append("cites numbers not in the table: " + json.dumps(out["numeric_claims"]["unsupported"]))
    text = json.dumps(proposal)
    for mentioned in set(re.findall(r"\bimg-[0-9a-z\-]+\b", text)):
        if mentioned not in by_id:
            out["fabrication_flags"].append(f"mentions {mentioned}, which does not exist")
    return out


# ------------------------------------------------------------------ the workflow

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--no-model", action="store_true", help="skip the model step (no API use)")
    ap.add_argument("--no-fault", action="store_true")
    args = ap.parse_args()
    if not args.no_model and not os.environ.get("OPENAI_API_KEY"):
        raise SystemExit("set OPENAI_API_KEY in the environment, or use --no-model")
    OUT.mkdir(parents=True, exist_ok=True)
    rep: dict = {"started_at": now(), "simulator_only": True, "hardware_accessed": False}
    actions: list = []
    proxy = FaultProxy(upstream=SIM)
    gateway = None
    try:
        # ---------------- 1. discover
        tds = http_json("GET", f"{API}/thing_descriptions/")[1]
        units = {}
        for thing in ("stage", "camera"):
            for kind in ("properties", "actions"):
                for name, aff in (tds[thing].get(kind) or {}).items():
                    schema = aff if kind == "properties" else ((aff.get("input") or {}).get("properties") or {})
                    items = {name: schema} if kind == "properties" else schema
                    for field, s in items.items():
                        if isinstance(s, dict) and s.get("type") in ("number", "integer"):
                            units[f"{thing}.{kind[:-1]}.{name}" + ("" if kind == "properties" else f".{field}")] = s.get("unit")
        samples = {}
        for name, prop in (tds["stage"].get("properties") or {}).items():
            if prop.get("type") == "object" and isinstance(prop.get("additionalProperties"), dict):
                form = td_import._form(prop.get("forms"), "readproperty")
                if form:
                    st, val = http_json("GET", SIM + form["href"])
                    if st == 200 and isinstance(val, dict):
                        samples[name] = val
        override = quickstart.stage_override(bound=ENVELOPE["x"])
        act = override["actions"]["move_absolute"]
        act["reason"] = "Experiment-only survey envelope in simulator steps; not a device or safety limit."
        act["variants"] = {f"move_{a}": {
            "params": {a: {"type": "integer", "range": {"min": -b, "max": b}}},
            "verification": {"mode": "readback", "capability": f"position.{a}", "expect_param": a,
                             "tolerance": {"abs": 0}, "settle_ms": 100, "timeout_ms": 10000}} for a, b in ENVELOPE.items()}
        act["timing"] = {"expected_ms": 3000, "timeout_ms": 60000}
        mapping = td_import.map_thing(tds["stage"], base_url=proxy.base + "/", device_id=DEVICE,
                                      overrides=override, samples=samples)
        mdir = RAW / "adapter"
        mdir.mkdir(parents=True, exist_ok=True)
        manifest = import_td.build_manifest(mapping, proxy.base + "/", mdir, source="simulator",
                                            timeout_ms=4000, stamp="received")
        (mdir / "manifest.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")
        home = RAW / "gateway"
        if home.exists():
            import shutil
            shutil.rmtree(home, ignore_errors=True)
        gateway = Gateway(home, mdir / "manifest.json", grants=False).start()
        op = gateway.op
        for g in ({"id": "survey-read", "principal": "agent:survey", "device": DEVICE, "capability": "*",
                   "operations": ["read"], "effect": "allow"},
                  *({"id": f"survey-{a}", "principal": "agent:survey", "device": DEVICE, "capability": f"move_{a}",
                     "operations": ["invoke"], "effect": "allow"} for a in ENVELOPE),
                  {"id": "adaptive-read", "principal": "agent:adaptive", "device": DEVICE, "capability": "*",
                   "operations": ["read"], "effect": "allow"},
                  *({"id": f"adaptive-{a}", "principal": "agent:adaptive", "device": DEVICE, "capability": f"move_{a}",
                     "operations": ["invoke"], "effect": "require_approval"} for a in ENVELOPE)):
            op.add_grant(g)
        survey = Client.from_home(home, token=op.create_token("agent:survey", any_scope=True).token)
        adaptive = Client.from_home(home, token=op.create_token("agent:adaptive", any_scope=True).token)
        _action("/stage/move_absolute", {"x": 0, "y": 0, "z": 0})      # outside the experiment: a known start
        seen = {i["id"] for i in (http_json("GET", f"{API}/action_invocations/")[1] or [])}
        mover = Mover(survey, op, proxy, actions)
        start = mover.read()
        allowed = survey.call("access.allowed_operations", {"device": DEVICE})
        rep["discover"] = {
            "things": sorted(tds), "stage_capabilities": sorted(c["name"] for c in mapping.descriptor["capabilities"]),
            "numeric_fields": len(units), "numeric_fields_with_unit": sorted(k for k, v in units.items() if v),
            "numeric_fields_without_unit": sorted(k for k, v in units.items() if not v)[:40],
            "importer_findings": mapping.by_category(), "allowed_operations": allowed, "start_position": start,
            "note": "No stage quantity declares a unit, so positions are simulator steps throughout."}

        # ---------------- 2. survey (the approved procedure)
        xs = [start["x"] + i * GRID_SPACING for i in range(-GRID_HALF, GRID_HALF + 1)]
        ys = [start["y"] + j * GRID_SPACING for j in range(-GRID_HALF, GRID_HALF + 1)]
        points = [(x, y) for j, y in enumerate(ys) for x in (xs if j % 2 == 0 else xs[::-1])]
        procedure = {"kind": "raster survey, serpentine", "points": points, "z": start["z"],
                     "envelope": ENVELOPE, "approved_by": "experiment operator (the procedure, before execution)"}
        procedure["sha256"] = hashlib.sha256(json.dumps(procedure, sort_keys=True).encode()).hexdigest()
        rep["procedure"] = procedure
        frames = []
        for k, (x, y) in enumerate(points):
            inject = not args.no_fault and k == FAULT_AT_POINT
            y_moves = mover.pos.get("y") != y                # inject into exactly one real move
            mover.move("y", y, why=f"survey point {k}", fault=inject and y_moves)
            mover.move("x", x, why=f"survey point {k}", fault=inject and not y_moves)
            frames.append(capture(f"img-s{k:02d}", {"x": x, "y": y, "z": mover.pos["z"], "phase": "survey"}))
            print(f"survey {k:02d} at ({x},{y})", flush=True)

        # ---------------- 3. analyse
        background = analyse(frames)
        rows = table(frames)
        ranked = sorted(rows, key=lambda r: (r["sample_fraction"], r["features"]), reverse=True)
        rep["analysis"] = {"method": "threshold = 95th-percentile background - 40; opening; connected components "
                                     "(>= 6 px at 1/4 scale); Laplacian variance for sharpness",
                           "background_level": round(background, 1), "rows": rows,
                           "deterministic_top3": [r["image_id"] for r in ranked[:3]]}

        # ---------------- 4. propose (model) and validate
        if args.no_model:
            proposal = {"arguments": {"rois": [{"image_id": ranked[0]["image_id"], "reason": "deterministic top"}],
                                      "points": [], "refocus_image_id": ranked[0]["image_id"],
                                      "explanation": "model step skipped"}, "text": "", "stop": "skipped"}
            provider = None
        else:
            provider = budget.make_provider()
            proposal = ask_model(rows, provider)
        checked = validate(proposal["arguments"], rows)
        rep["proposal"] = {"model": None if provider is None else provider.model, "raw": proposal, "validation": checked,
                           "usage": None if provider is None else {"input_tokens": provider.input_tokens,
                                                                   "output_tokens": provider.output_tokens,
                                                                   "estimated_cost_usd": round(provider.estimated_cost_usd, 6)}}
        if provider is not None:
            budget.add_spend(provider.estimated_cost_usd, "sample_survey_workflow/proposal")

        # ---------------- 5. adapt: focus sweep, then approved follow-ups
        approvals: list = []

        def approve(command_id: str) -> None:              # the human, for validated proposals only
            approvals.append({"command_id": command_id, "at": now(), "by": "experiment operator"})
            op.approve(command_id)

        adapt = Mover(adaptive, op, proxy, actions)
        adapt.read()
        best_z = start["z"]
        focus = []
        if checked["refocus"]:
            ref = next(r for r in rows if r["image_id"] == checked["refocus"])
            adapt.move("y", ref["y"], why="refocus target", approve=approve)
            adapt.move("x", ref["x"], why="refocus target", approve=approve)
            for z in FOCUS_ZS:
                adapt.move("z", z, why="focus sweep", approve=approve)
                f = capture(f"img-f{z:+d}".replace("+", "p").replace("-", "m"), {"x": ref["x"], "y": ref["y"], "z": z, "phase": "focus"})
                focus.append(f)
            analyse(frames + focus)                         # same background reference
            best = max(focus, key=lambda f: f["metrics"]["sharpness"])
            best_z = best["z"]
            adapt.move("z", best_z, why="best focus", approve=approve)
        follow = []
        for i, p in enumerate(checked["accepted_points"]):
            adapt.move("y", p["y"], why=f"follow-up {i}", approve=approve)
            adapt.move("x", p["x"], why=f"follow-up {i}", approve=approve)
            follow.append(capture(f"img-a{i}", {"x": p["x"], "y": p["y"], "z": best_z, "phase": "follow-up"}))
        if follow or focus:
            analyse(frames + focus + follow)
        rep["adapt"] = {"focus_sweep": table(focus), "best_z": best_z, "follow_ups": table(follow), "approvals": approvals}

        # return to start, through OpenMHS
        for axis in "zyx":
            adapt.move(axis, start[axis], why="return to start", approve=approve)

        # ---------------- 6/7. ground truth, audit, report
        time.sleep(2)
        invs = [i for i in (http_json("GET", f"{API}/action_invocations/")[1] or [])
                if i["id"] not in seen and str(i.get("action", "")).endswith("move_absolute")]
        executed = []
        for i in sorted(invs, key=lambda i: i.get("timeRequested") or ""):
            d = http_json("GET", f"{API}/action_invocations/{i['id']}")[1] or {}
            executed.append({"id": i["id"], "input": {k: v for k, v in (d.get("input") or {}).items() if k in "xyz"},
                             "status": d.get("status")})
        refs = {a["device_reference"].split(":", 1)[1] for a in actions if a.get("device_reference")}
        faulted = [a for a in actions if "suspended" in a]
        rep["ground_truth"] = {
            "moves_executed_by_simulator": len(executed), "moves_recorded_by_openmhs": len([a for a in actions if "command_id" in a]),
            "faulted_moves": faulted,
            "faulted_move_executions": [_executions_of(f, executed) for f in faulted],
            "executions_without_openmhs_reference": [e for e in executed if e["id"] not in refs],
            "final_position": http_json("GET", f"{API}/stage/position")[1], "start_position": start}
        report = export_report.build(op, DEVICE)
        exp = OUT / "evidence.json"
        chk = OUT / "checkpoint.json"
        for p in (exp, chk):
            p.unlink(missing_ok=True)
        subprocess.run([OPENMHS, "--home", str(home), "audit", "export", "--out", str(exp), "--checkpoint-out", str(chk)],
                       check=True, capture_output=True, text=True)
        ver = subprocess.run([OPENMHS, "--json", "audit", "verify-file", str(exp), "--checkpoint", str(chk)],
                             capture_output=True, text=True)
        rep["audit"] = {"chain_valid": report["audit"]["valid"], "records": report["audit"]["records"],
                        "verify_file_exit": ver.returncode, "verify_file": (ver.stdout or ver.stderr)[:2000],
                        "receipts_inconsistent_with_chain": report["receipts_inconsistent_with_chain"]}
        rep["actions"] = actions
        rep["images"] = [{k: v for k, v in f.items() if k != "array"} for f in frames + focus + follow]
        rep["versions"] = {"openmhs_commit": subprocess.run(["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"],
                                                            capture_output=True, text=True).stdout.strip(),
                           "openflexure_microscope_server": "3.0.0b1 (simulation config)", "labthings_fastapi": "0.3.0",
                           "python": platform.python_version(), "os": platform.platform()}
        unresolved = [a for a in actions if a["state"] not in ("VERIFIED", "RECONCILED_APPLIED")]
        rep["uncertainties"] = [
            "Simulator only: images are synthetic and stage motion is a dummy stage; nothing here is physical evidence.",
            "Stage quantities declare no unit; all positions are simulator steps.",
            "Image capture is a camera read performed by the workflow, linked by time and position to "
            "OpenMHS-verified moves; OpenMHS does not itself attest the image content.",
            "RECONCILED_APPLIED means a readback after the fault matched the target; it does not prove when the move "
            "finished." if faulted else "No fault was injected in this run.",
            f"{len(unresolved)} actions ended outside VERIFIED/RECONCILED_APPLIED." if unresolved else
            "No action remained unresolved.",
            "Analysis thresholds are fixed choices for this simulator, not validated for real samples."]
        rep["finished_at"] = now()
        (OUT / "workflow-report.json").write_text(json.dumps(rep, indent=1, default=str) + "\n", encoding="utf-8")
        (OUT / "audit-report.md").write_text(export_report.to_markdown(report).rstrip() + "\n", encoding="utf-8")
        write_markdown(rep)
        print(f"done: {len(frames)} survey, {len(focus)} focus, {len(follow)} follow-up images; "
              f"audit valid={report['audit']['valid']} verify-file exit={ver.returncode}", flush=True)
        return 0
    finally:
        if gateway:
            gateway.stop()
        proxy.stop()


def _state(receipt) -> str:
    st = getattr(receipt, "state", None)
    return str(getattr(st, "value", st))


def _executions_of(entry: dict, executed: list[dict]) -> int:
    """How many times the simulator ran the faulted move: its own invocation plus any identical
    invocation immediately after it (a resend). 1 means it ran once and was not repeated."""
    ref = (entry.get("device_reference") or "").split(":", 1)[-1]
    ids = [e["id"] for e in executed]
    if ref not in ids:
        return sum(1 for e in executed if e["input"] == {entry["axis"]: entry["target"]})
    i = ids.index(ref)
    n = 1
    while i + n < len(executed) and executed[i + n]["input"] == executed[i]["input"]:
        n += 1
    return n


def write_markdown(rep: dict) -> None:
    a, g = rep["analysis"], rep["ground_truth"]
    lines = ["# Simulated sample survey: workflow report", "",
             f"Run {rep['started_at']} to {rep['finished_at']}. **Simulator only; no hardware was accessed.**", "",
             "## 1. Discovery", "",
             f"- Things on the server: {', '.join(rep['discover']['things'])}",
             f"- Stage capabilities imported: {', '.join(rep['discover']['stage_capabilities'])}",
             f"- Numeric fields with a declared unit: {len(rep['discover']['numeric_fields_with_unit'])} of "
             f"{rep['discover']['numeric_fields']} ({', '.join(rep['discover']['numeric_fields_with_unit']) or 'none'})",
             f"- Start position (OpenMHS readback): {rep['discover']['start_position']}", "",
             "## 2. Approved procedure", "",
             f"{rep['procedure']['kind']}, {len(rep['procedure']['points'])} points, envelope {rep['procedure']['envelope']} "
             f"(simulator steps), procedure sha256 `{rep['procedure']['sha256'][:16]}`.", "",
             "## 3. Survey and deterministic analysis", "",
             f"Method: {a['method']}. Background level {a['background_level']}.", "",
             "| image | x | y | sample fraction | features | sharpness |", "|---|---|---|---|---|---|"]
    for r in a["rows"]:
        lines.append(f"| ![{r['image_id']}](images/{r['image_id']}.png) `{r['image_id']}` | {r['x']} | {r['y']} | "
                     f"{r['sample_fraction']} | {r['features']} | {r['sharpness']} |")
    p = rep["proposal"]
    v = p["validation"]
    lines += ["", f"Deterministic top three: {', '.join(a['deterministic_top3'])}.", "",
              "## 4. Model proposal and validation", "",
              f"Model: {p['model'] or 'skipped'}; usage {p['usage']}.", "",
              "```json", json.dumps(p["raw"]["arguments"], indent=1), "```", "",
              f"- Accepted follow-up points: {v['accepted_points']}",
              f"- Rejected: {v['rejected'] or 'none'}",
              f"- Numbers cited for ROIs checked against the table: {v.get('numeric_claims', {}).get('checked')}, unsupported: {v.get('numeric_claims', {}).get('unsupported') or 'none'}",
              f"- Fabrication flags: {v['fabrication_flags'] or 'none'}",
              f"- Refocus target accepted: {v['refocus']}", "",
              "## 5. Adaptation", "",
              f"Focus sweep at {v['refocus']}: " + ", ".join(f"z={r['z']} sharpness {r['sharpness']}" for r in rep["adapt"]["focus_sweep"])
              + f". Best z = {rep['adapt']['best_z']}.", "",
              f"Follow-up captures: {[(r['image_id'], r['x'], r['y'], r['features']) for r in rep['adapt']['follow_ups']]}. "
              f"Each follow-up move was held for approval; approvals: {len(rep['adapt']['approvals'])}.", "",
              "## 6. Fault and reconciliation", ""]
    for f in g["faulted_moves"]:
        lines.append(f"- `{f['axis']}={f['target']}` ({f['why']}): suspended at {f['suspended']['at']}, "
                     f"{f['reconciled']['state']} at {f['reconciled']['at']}; device reference `{f['device_reference']}`.")
    lines += [f"- Simulator executions of each faulted move: {g['faulted_move_executions']} (1 means it was not repeated).",
              f"- Moves executed by the simulator: {g['moves_executed_by_simulator']}; recorded by OpenMHS: "
              f"{g['moves_recorded_by_openmhs']}; executions with no OpenMHS reference: {len(g['executions_without_openmhs_reference'])}.",
              f"- Final position {g['final_position']} vs start {g['start_position']}.", "",
              "## 7. Audit integrity", "",
              f"- Chain valid: {rep['audit']['chain_valid']} ({rep['audit']['records']} records); offline `audit verify-file` exit "
              f"{rep['audit']['verify_file_exit']} (0 = valid and complete).",
              f"- Receipts inconsistent with chain: {rep['audit']['receipts_inconsistent_with_chain'] or 'none'}", "",
              "## 8. Versions", "", f"`{json.dumps(rep['versions'])}`", "", "## 9. Uncertainties", ""]
    lines += [f"- {u}" for u in rep["uncertainties"]]
    lines += ["", "Images: thumbnails in `images/`; full frames were kept outside the repository with their sha256 "
              "listed in `workflow-report.json`."]
    (OUT / "README.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


if __name__ == "__main__":
    raise SystemExit(main())
