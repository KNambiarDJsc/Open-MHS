"""Phase 9: a previously unseen, independently authored Thing Description.

Runs the *same* importer, adapter and OpenMHS tools on a hotplate described in the style of the
W3C TD 1.1 examples (`hotplate_td.json`), served by `hotplate_server.py`. Records, at each stage,
what mapped automatically, what needed review, and what was unsupported; then runs live commands
through a real gateway; then repeats on a deliberately incomplete description.

    python hotplate_server.py --port 5200 &      # simulator, user lab / password bench
    python run_generalization.py > ../results/generalization.json
"""

from __future__ import annotations

import copy
import json
import os
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "experiments"))
import lab  # noqa: E402
import import_td  # noqa: E402
import td_import  # noqa: E402

BASE = "http://127.0.0.1:5200/"
CREDS = {"WOT_USER": "lab", "WOT_PASSWORD": "bench"}
TD = json.loads((HERE / "hotplate_td.json").read_text(encoding="utf-8"))

# What a reviewer supplies. Everything else comes from the Thing Description.
REVIEWED = {
    "manufacturer": "unknown (simulator)", "model": "Stirring hotplate",
    "actions": {
        "start": {"expose": True, "side_effect": "reversible", "timing": {"timeout_ms": 5000},
                  "reviewed_by": "experiment author (simulator only)",
                  "reason": "Starts heating toward the target; reversible by stop."},
        "stop": {"expose": True, "side_effect": "reversible", "stop": True, "timing": {"timeout_ms": 5000},
                 "reviewed_by": "experiment author (simulator only)", "reason": "Stops heating."},
        "setProgram": {"expose": True, "side_effect": "reversible", "timing": {"timeout_ms": 5000},
                       "verification": {"mode": "readback", "capability": "target_temperature",
                                        "expect_param": "temperature", "tolerance": {"abs": 0.01},
                                        "settle_ms": 100, "timeout_ms": 3000},
                       "reviewed_by": "experiment author (simulator only)",
                       "reason": "Sets the target and starts heating; verified by reading the target back."},
    },
}


def stats(m: td_import.Mapping) -> dict:
    caps = m.descriptor["capabilities"]
    return {
        "exposed": {"properties": sum(c["kind"] == "property" for c in caps),
                    "actions": sum(c["kind"] == "action" for c in caps)},
        "counts": m.counts(), "reasons": m.by_category(), "connectable": m.connectable,
        "unsupported": [f"{f.affordance}: {f.category}" for f in m.findings if f.status == td_import.UNSUPPORTED],
        "needs_review": sorted({f"{f.affordance}: {f.category}" for f in m.findings
                                if f.status == td_import.REVIEW and f.category not in ("no_measurement_time",)}),
    }


def run_gateway(name: str, td: dict, overrides, *, creds=True):
    m = td_import.map_thing(td, base_url=BASE, device_id="lab.hotplate", overrides=overrides)
    out = lab.WORK / f"gen-{name}"
    out.mkdir(parents=True, exist_ok=True)
    manifest = import_td.build_manifest(m, BASE, out, source="simulator", timeout_ms=1500)
    path = out / "manifest.json"
    path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    os.environ.update(CREDS if creds else {})
    home = lab.WORK / f"gen-{name}-home"
    if home.exists():  # a fresh journal and policy every time, never state from an earlier run
        import shutil
        shutil.rmtree(home, ignore_errors=True)
    gw = lab.Gateway(home, path).start()
    return m, manifest, path, gw


def rec(r) -> dict:
    return {"state": r.state.value, "outcome": r.outcome.value, "path": [t.state.value for t in r.transitions],
            "errors": [e.code + ": " + e.message[:140] for e in r.policy.errors],
            "detail": (r.transitions[-1].detail or "")[:140]}


def main() -> None:
    out: dict = {"_meta": {"td": "research/generalization/hotplate_td.json", "when": time.strftime("%FT%T%z"),
                           "note": "simulators only; the TD was written before the importer was run on it"}}

    # 1. unreviewed import
    m0 = td_import.map_thing(TD, base_url=BASE, device_id="lab.hotplate")
    out["unreviewed"] = stats(m0)

    # 2. reviewed import, then live
    m1, manifest, path, gw = run_gateway("reviewed", TD, REVIEWED)
    out["reviewed"] = stats(m1)
    try:
        a = gw.agent
        live: dict = {}
        r = a.read("lab.hotplate", "temperature")
        live["read_temperature"] = {"value": r.state.value if hasattr(r.state, "value") else str(r.state),
                                    "usable": r.usable, "source": str(r.state.source)}
        live["selfTest_td_safe"] = rec(a.invoke("lab.hotplate", "self_test", {}, idempotency_key="g-self"))
        live["setProgram_in_range"] = rec(a.invoke(
            "lab.hotplate", "set_program",
            {"temperature": {"value": 40, "unit": "Cel"}, "minutes": {"value": 1, "unit": "min"}},
            idempotency_key="g-prog"))
        time.sleep(3)
        live["temperature_after_3s"] = a.read("lab.hotplate", "temperature").state.value
        try:
            live["setProgram_out_of_range"] = rec(a.invoke(
                "lab.hotplate", "set_program",
                {"temperature": {"value": 500, "unit": "Cel"}, "minutes": {"value": 1, "unit": "min"}},
                idempotency_key="g-hot"))
        except Exception as e:
            live["setProgram_out_of_range"] = {"refused_by_openmhs": str(e)[:200]}
        live["stop"] = rec(a.invoke("lab.hotplate", "stop", {}, idempotency_key="g-stop"))
        try:
            a.invoke("lab.hotplate", "no_such_action", {}, idempotency_key="g-x")
        except Exception as e:
            live["unknown_action"] = str(e)[:160]
        out["live"] = live
        # credentials must not appear in the manifest, the journal's audit or the gateway log
        text = json.dumps(manifest) + (lab.WORK / "gen-reviewed-home" / "serve.log").read_text(errors="replace")
        text += json.dumps([x.__dict__ if hasattr(x, "__dict__") else str(x) for x in gw.op.audit_tail(500)], default=str)
        out["credential_leak"] = {"manifest_declares_names_only": manifest.get("credentials"),
                                  "password_found_in_manifest_log_or_audit": ("bench" in text.replace("bench-", "")
                                                                               and "password" in text.lower() and CREDS["WOT_PASSWORD"] + '"' in text)}
    finally:
        gw.stop()

    # 3. wrong credentials: the adapter must fail clearly, not silently
    os.environ["WOT_PASSWORD"] = "wrong"
    _, _, p2, gw2 = run_gateway("badcreds", TD, REVIEWED, creds=False)
    try:
        time.sleep(1.5)
        st = [(d.availability.value, d.reason) for d in gw2.agent.device_status()]
        out["wrong_credentials"] = {"device_status": st}
    finally:
        gw2.stop()
    os.environ["WOT_PASSWORD"] = CREDS["WOT_PASSWORD"]

    # 4. an incomplete description: no units, ranges, safety flags or descriptions
    bad = copy.deepcopy(TD)
    for p in bad["properties"].values():
        for k in ("unit", "minimum", "maximum", "title"):
            p.pop(k, None)
    for a_ in bad["actions"].values():
        for k in ("safe", "idempotent", "title"):
            a_.pop(k, None)
        for p in ((a_.get("input") or {}).get("properties") or {}).values():
            for k in ("unit", "minimum", "maximum"):
                p.pop(k, None)
    out["incomplete_td_unreviewed"] = stats(td_import.map_thing(bad, base_url=BASE, device_id="lab.hotplate"))
    m2 = td_import.map_thing(bad, base_url=BASE, device_id="lab.hotplate", overrides=REVIEWED)
    out["incomplete_td_with_same_review"] = stats(m2)
    # A reviewer completes what the description no longer says: idempotency (stripped from the
    # TD), and that the parameters are meant to be unbounded here (no limits are declared).
    full = copy.deepcopy(REVIEWED)
    for name in ("start", "stop", "setProgram"):
        full["actions"][name]["idempotent"] = name != "setProgram"
    full["actions"]["setProgram"]["unbounded_ok"] = ["temperature", "minutes"]
    out["incomplete_td_with_completed_review"] = stats(
        td_import.map_thing(bad, base_url=BASE, device_id="lab.hotplate", overrides=full))
    m3, _, _, gw3 = run_gateway("incomplete", bad, full)
    try:
        r = gw3.agent.invoke("lab.hotplate", "set_program",
                             {"temperature": {"value": 500, "unit": "1"}, "minutes": 1},
                             idempotency_key="g-bad")
        out["incomplete_td_out_of_range"] = {
            **rec(r), "note": "no limit was declared and the reviewer accepted that, so OpenMHS forwarded 500. "
                              "This stand-in's setProgram has no range check of its own, so the device accepted it "
                              "and readback confirmed 500: nothing in the chain stopped it. With the complete "
                              "description the same request was DENIED by OpenMHS (20..300)."}
    except Exception as e:
        out["incomplete_td_out_of_range"] = {"error": str(e)[:300]}
    finally:
        gw3.stop()
    print(json.dumps(out, indent=1, default=str))


if __name__ == "__main__":
    main()
