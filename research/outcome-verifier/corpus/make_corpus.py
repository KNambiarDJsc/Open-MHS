"""Write the fault corpus in EVALUATION.md as files: one directory per case, with
contract.json, evidence.jsonl and expected.json.

    python research/outcome-verifier/corpus/make_corpus.py          # write
    python research/outcome-verifier/corpus/make_corpus.py --check  # fail if the files differ

Every record here is typed by hand. The cases test the verifier's rules, not a device or ROS 2.
The expected verdicts were written down before the verifier was run on them.
"""

from __future__ import annotations

import copy
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
CASES = HERE / "cases"
T = "2026-11-10T09:00:00.000Z"


def at(ms: int) -> str:
    """T plus ms, as RFC 3339 with milliseconds."""
    base_s = 9 * 3600
    total = base_s * 1000 + ms
    sign = total < 0
    assert not sign
    h, rem = divmod(total, 3600_000)
    m, rem = divmod(rem, 60_000)
    s, frac = divmod(rem, 1000)
    return f"2026-11-10T{h:02d}:{m:02d}:{s:02d}.{frac:03d}Z"


def volts(v: float) -> dict:
    return {"value": v, "unit": "V"}


def metres(v: float) -> dict:
    return {"value": v, "unit": "m"}


# ------------------------------------------------------------------ bench supply

PSU_CONTRACT = {
    "contract": "psu-setpoint",
    "version": "0.1",
    "binding": {"correlation": "attempt-7"},
    "timing": {"anchor": "command", "default_window_ms": [0, 2000], "max_clock_uncertainty_ms": 50},
    "evidence": {
        "command": {"signal": "command:power.output.voltage.setpoint", "kind": "controller"},
        "readback": {"signal": "power.output.voltage.measured", "kind": "device_state", "bound": False},
        "dmm": {"signal": "output.voltage", "kind": "independent_sensor", "calibration": ["dc-voltage"]},
    },
    "conditions": [
        {"name": "device reads back 2 V", "evidence": "readback", "op": "between",
         "min": volts(1.95), "max": volts(2.05)},
        {"name": "the DMM measures 2 V", "evidence": "dmm", "op": "between",
         "min": volts(1.95), "max": volts(2.05)},
    ],
}


def psu_command(state: str = "VERIFIED") -> dict:
    return {"id": "cmd-1", "signal": "command:power.output.voltage.setpoint", "value": state,
            "source": {"system": "openmhs", "device": "psu-1", "kind": "controller"},
            "time": {"source": T, "clock_domain": "gateway-host", "uncertainty_ms": 2},
            "refs": {"correlation": "attempt-7", "command": "c-0001"}}


def psu_readback(v: float = 2.001, ms: int = 300) -> dict:
    return {"id": "rb-1", "signal": "power.output.voltage.measured", "value": volts(v),
            "source": {"system": "openmhs", "device": "psu-1", "kind": "device_state", "provenance": "device"},
            "time": {"source": at(ms), "clock_domain": "gateway-host"}}


def dmm(v=1.998, ms: int = 800, rid: str = "dmm-1", unit: str = "V", **time_overrides) -> dict:
    time = {"source": at(ms), "clock_domain": "bench-host", "uncertainty_ms": 5}
    time.update(time_overrides)
    time = {k: v for k, v in time.items() if v is not None}
    return {"id": rid, "signal": "output.voltage", "value": {"value": v, "unit": unit},
            "source": {"system": "bench", "device": "dmm-1", "kind": "independent_sensor", "provenance": "device"},
            "time": time, "refs": {"correlation": "attempt-7"}}


def dmm_calibration(valid_until: str = "2027-06-01T00:00:00Z") -> dict:
    return {"id": "cal-dmm-1", "signal": "calibration", "value": "dc-voltage",
            "source": {"system": "bench", "device": "dmm-1", "kind": "calibration", "provenance": "device"},
            "calibration": {"name": "dc-voltage", "applies_to": "dmm-1",
                            "valid_from": "2026-06-01T00:00:00Z", "valid_until": valid_until}}


def psu_base() -> list[dict]:
    return [psu_command(), psu_readback(), dmm(), dmm_calibration()]


def replace(records: list[dict], rid: str, new: dict | None) -> list[dict]:
    out = [r for r in records if r["id"] != rid]
    if new is not None:
        out.append(new)
    return out


def with_quality(record: dict, quality: str) -> dict:
    r = copy.deepcopy(record)
    r["quality"] = quality
    return r


def with_refs(record: dict, **refs) -> dict:
    r = copy.deepcopy(record)
    r["refs"] = refs
    return r


PSU_CASES = {
    "all_agree": (psu_base(), "SATISFIED", None),
    "dmm_reads_zero": (replace(psu_base(), "dmm-1", dmm(0.0)), "VIOLATED", "condition.false"),
    "readback_high": (replace(psu_base(), "rb-1", psu_readback(2.3)), "VIOLATED", "condition.false"),
    "dmm_calibration_expired": (replace(psu_base(), "cal-dmm-1", dmm_calibration("2026-10-01T00:00:00Z")),
                                "UNKNOWN", "calibration.expired"),
    "dmm_calibration_missing": (replace(psu_base(), "cal-dmm-1", None), "UNKNOWN", "calibration.missing"),
    "dmm_before_command": (replace(psu_base(), "dmm-1", dmm(ms=-500)), "UNKNOWN", "evidence.outside_window"),
    "dmm_too_late": (replace(psu_base(), "dmm-1", dmm(ms=5000)), "UNKNOWN", "evidence.outside_window"),
    "dmm_other_clock_no_uncertainty": (replace(psu_base(), "dmm-1", dmm(uncertainty_ms=None)),
                                       "UNKNOWN", "time.clock_domain"),
    "dmm_clock_uncertainty_too_large": (replace(psu_base(), "dmm-1", dmm(uncertainty_ms=200)),
                                        "UNKNOWN", "time.uncertainty"),
    "dmm_straddles_window_edge": (replace(psu_base(), "dmm-1", dmm(ms=1990, uncertainty_ms=20)),
                                  "UNKNOWN", "time.ambiguous"),
    "dmm_for_another_attempt": (replace(psu_base(), "dmm-1", with_refs(dmm(), correlation="attempt-8")),
                                "UNKNOWN", "evidence.identity_mismatch"),
    "dmm_quality_uncertain": (replace(psu_base(), "dmm-1", with_quality(dmm(), "UNCERTAIN")),
                              "UNKNOWN", "evidence.quality"),
    "dmm_readings_disagree": (psu_base() + [dmm(0.0, ms=900, rid="dmm-2")], "UNKNOWN", "evidence.conflict"),
    "dmm_in_millivolts": (replace(psu_base(), "dmm-1", dmm(1998.0, unit="mV")), "SATISFIED", None),
    "dmm_in_a_unit_with_no_conversion": (replace(psu_base(), "dmm-1", dmm(1.998, unit="A")),
                                         "UNKNOWN", "evidence.unit"),
    "no_command_record": (replace(psu_base(), "cmd-1", None), "UNKNOWN", "timing.anchor_missing"),
    "command_unknown_outcome_but_voltage_is_there": (replace(psu_base(), "cmd-1", psu_command("UNKNOWN_OUTCOME")),
                                                     "SATISFIED", None),
}

# ------------------------------------------------------------------ pick and place (synthetic, ROS 2-shaped)

PICK_CONTRACT = {
    "contract": "place-sample-in-tray",
    "version": "0.1",
    "binding": {"task": "place-104"},
    "timing": {"anchor": "action", "default_window_ms": [0, 1500], "max_clock_uncertainty_ms": 20},
    "evidence": {
        "action": {"signal": "action:/arm/place", "kind": "controller"},
        "gripper": {"signal": "gripper.object_detected", "kind": "device_state", "bound": False},
        "x": {"signal": "object.position.x", "kind": "independent_sensor", "device": "overhead-cam",
              "calibration": ["camera-extrinsics"], "bound": False},
        "y": {"signal": "object.position.y", "kind": "independent_sensor", "device": "overhead-cam",
              "calibration": ["camera-extrinsics"], "bound": False},
        "object": {"signal": "object.id", "kind": "independent_sensor", "device": "overhead-cam", "bound": False},
    },
    "conditions": [
        {"name": "the object is the sample", "evidence": "object", "op": "eq", "value": "sample_104"},
        {"name": "x inside the tray", "evidence": "x", "op": "between", "min": metres(0.40), "max": metres(0.60)},
        {"name": "y inside the tray", "evidence": "y", "op": "between", "min": metres(-0.10), "max": metres(0.10)},
        {"name": "the gripper let go", "evidence": "gripper", "op": "eq", "value": False},
    ],
}


def robot(rid: str, signal: str, value, ms: int, kind: str, device: str, provenance: str = "device", **extra) -> dict:
    r = {"id": rid, "signal": signal, "value": value,
         "source": {"system": "ros2", "device": device, "kind": kind, "provenance": provenance},
         "time": {"source": at(ms), "clock_domain": "robot-host"}}
    r.update(extra)
    return r


def action(status: str = "SUCCEEDED") -> dict:
    return robot("act-1", "action:/arm/place", status, 0, "controller", "arm",
                 refs={"task": "place-104", "external": "ros:goal:8f23"})


def camera(x=0.51, y=0.02, ms=400, provenance="device") -> list[dict]:
    return [robot("cam-x", "object.position.x", metres(x), ms, "independent_sensor", "overhead-cam", provenance),
            robot("cam-y", "object.position.y", metres(y), ms, "independent_sensor", "overhead-cam", provenance),
            robot("cam-id", "object.id", "sample_104", ms, "independent_sensor", "overhead-cam", provenance)]


def camera_calibration(valid_until: str = "2027-01-01T00:00:00Z") -> dict:
    return {"id": "cal-cam", "signal": "calibration", "value": "camera-extrinsics",
            "source": {"system": "ros2", "device": "overhead-cam", "kind": "calibration", "provenance": "device"},
            "calibration": {"name": "camera-extrinsics", "applies_to": "overhead-cam",
                            "valid_from": "2026-11-01T00:00:00Z", "valid_until": valid_until}}


def gripper(detected: bool = False) -> dict:
    return robot("grip-1", "gripper.object_detected", detected, 350, "device_state", "gripper")


def pick_base(**camera_args) -> list[dict]:
    return [action(), gripper(), camera_calibration(), *camera(**camera_args)]


def set_value(records: list[dict], rid: str, value) -> list[dict]:
    out = copy.deepcopy(records)
    for r in out:
        if r["id"] == rid:
            r["value"] = value
    return out


PICK_CASES = {
    "placed": (pick_base(), "SATISFIED", None),
    "never_grasped": (pick_base(x=0.10, y=0.30), "VIOLATED", "condition.false"),
    "dropped_outside_the_tray": (pick_base(x=0.75), "VIOLATED", "condition.false"),
    "camera_has_no_pose": ([r for r in pick_base() if r["id"] not in ("cam-x", "cam-y")],
                           "UNKNOWN", "evidence.missing"),
    "camera_pose_before_the_action_ended": (pick_base(ms=-200), "UNKNOWN", "evidence.outside_window"),
    "camera_calibration_expired": (replace(pick_base(), "cal-cam", camera_calibration("2026-11-05T00:00:00Z")),
                                   "UNKNOWN", "calibration.expired"),
    "wrong_object": (set_value(pick_base(), "cam-id", "sample_105"), "VIOLATED", "condition.false"),
    "gripper_still_closed": (replace(pick_base(), "grip-1", gripper(True)), "VIOLATED", "condition.false"),
    "action_aborted_but_object_in_tray": (replace(pick_base(), "act-1", action("ABORTED")), "SATISFIED", None),
    "camera_is_a_simulator": (pick_base(provenance="simulator"), "SATISFIED", None),
}


def files() -> dict[Path, str]:
    out: dict[Path, str] = {}
    for domain, contract, cases in (("psu", PSU_CONTRACT, PSU_CASES), ("pick_place", PICK_CONTRACT, PICK_CASES)):
        for name, (records, verdict, code) in cases.items():
            d = CASES / domain / name
            out[d / "contract.json"] = json.dumps(contract, indent=2, sort_keys=True) + "\n"
            out[d / "evidence.jsonl"] = "".join(json.dumps(r, sort_keys=True) + "\n"
                                                for r in sorted(records, key=lambda r: r["id"]))
            expected = {"verdict": verdict}
            if code:
                expected["reason"] = code
            out[d / "expected.json"] = json.dumps(expected, indent=2, sort_keys=True) + "\n"
    return out


def main() -> int:
    check = "--check" in sys.argv[1:]
    wanted = files()
    stale = []
    for path, text in sorted(wanted.items()):
        current = path.read_text(encoding="utf-8") if path.exists() else None
        if current != text:
            stale.append(path)
            if not check:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text, encoding="utf-8", newline="\n")
    extra = [p for p in CASES.rglob("*") if p.is_file() and p not in wanted] if CASES.exists() else []
    if check and (stale or extra):
        print("corpus out of date:", *[str(p) for p in stale + extra], sep="\n  ")
        return 1
    print(f"{len(wanted) // 3} cases" + ("" if check else f", {len(stale)} file(s) written"))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
