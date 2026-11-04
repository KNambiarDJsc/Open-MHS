"""Generate the OpenMHS Modbus TCP profile from ``register_map.py``.

    python build_profile.py            # rewrite battery-test-instrument.profile.json
    python build_profile.py --check    # exit 1 if the committed file is out of date

The profile is labelled EXAMPLE-ONLY and SIMULATED. It is not a standard and is not any real
instrument's register map.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import register_map as rm  # noqa: E402
PROFILE = HERE / "battery-test-instrument.profile.json"
DEFAULT_ADDRESS = "127.0.0.1:15020"

# Readings older than this are flagged by the gateway. Modbus replies are stamped when the
# adapter reads them, so this guards against a slow link, not against an instrument that has
# stopped measuring (that is `telemetry.sample_time`, judged by the agent code).
FRESHNESS_MS = 5000


def _value_spec(r: rm.Reg) -> dict[str, Any]:
    spec: dict[str, Any] = {"type": r.ty}
    if r.quantity:
        spec["quantity"] = r.quantity
    if r.unit:
        spec["unit"] = r.unit
    if r.range:
        lo, hi = r.range
        spec["range"] = {"min": lo, "max": hi}
    if r.values:
        spec["values"] = list(r.values)
    return spec


def _capability(r: rm.Reg) -> dict[str, Any]:
    cap: dict[str, Any] = {"name": r.name}
    if r.action:
        cap.update(kind="action", description=r.description, operations=["invoke"],
                   side_effect=r.side_effect, idempotent=r.action == "stop",
                   timing={"expected_ms": 50, "timeout_ms": 2000},
                   verification={"mode": "none",
                                 "reason": "EXAMPLE-ONLY: the simulated instrument reports no test state"})
        return cap
    cap.update(kind="property", description=r.description,
               operations=["read", "write"] if r.writable else ["read"], value=_value_spec(r),
               side_effect=r.side_effect)
    if r.writable:
        cap.update(idempotent=True, timing={"expected_ms": 50, "timeout_ms": 2000},
                   verification={"mode": "readback", "tolerance": {"abs": 0.001}, "timeout_ms": 2000})
    elif r.name.startswith("telemetry.") and r.name != "telemetry.sample_time":
        cap["freshness"] = {"max_age_ms": FRESHNESS_MS}
    return cap


def _mapping(r: rm.Reg) -> dict[str, Any]:
    m: dict[str, Any] = {"table": r.table, "address": r.address}
    if r.action:
        m["write"] = True
        return m
    if r.table in ("input", "holding"):
        if r.kind != "u16":
            m["type"] = r.kind
        if r.scale != 1.0:
            m["scale"] = r.scale
    return m


def _initial() -> dict[str, Any]:
    """Seed values for the built-in `--profile-sim` simulator (a quick-start fallback)."""
    return {
        "instrument.simulated": True,
        "instrument.state": "idle",
        "telemetry.data_quality": "ok",
        "telemetry.voltage": {"value": 3.7, "unit": "V"},
        "telemetry.current": {"value": 0.0, "unit": "A"},
        "telemetry.temperature": {"value": 23.5, "unit": "Cel"},
        "telemetry.sample_time": 1767225600,
        "session.count": 0,
        "limit.voltage_max": {"value": 4.2, "unit": "V"},
    }


def build(address: str = DEFAULT_ADDRESS) -> dict[str, Any]:
    regs = rm.registers()
    return {
        "openmhs_profile": "0.1",
        "protocol": "modbus-tcp",
        "descriptor": {
            "openmhs": "0.1",
            "device": {
                "id": rm.DEVICE_ID,
                "name": "SIMULATED battery test instrument (EXAMPLE-ONLY)",
                "manufacturer": "OpenMHS example (SIMULATED)",
                "model": "EXAMPLE-ONLY-BT1",
                "description": (
                    "A simulated programmable battery test instrument for the OpenMHS battery "
                    "equipment example. The register map is EXAMPLE-ONLY: it is not a standard "
                    "and not any real product's. All values are SIMULATED."
                ),
            },
            "adapter": {"id": "openmhs.modbus", "version": "0.1.0", "conformance": "L1"},
            "capabilities": [_capability(r) for r in regs],
        },
        "connection": {"address": address, "unit_id": 1, "reply_timeout_ms": 1000, "simulated": True},
        "mapping": {r.name: _mapping(r) for r in regs},
        "simulator": {"initial": _initial()},
    }


def render(address: str = DEFAULT_ADDRESS) -> str:
    return json.dumps(build(address), indent=2) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()
    text = render()
    if args.check:
        if not PROFILE.exists() or PROFILE.read_text(encoding="utf-8") != text:
            print(f"{PROFILE.name} is out of date: run `python build_profile.py`", file=sys.stderr)
            return 1
        return 0
    PROFILE.write_text(text, encoding="utf-8", newline="\n")
    print(f"wrote {PROFILE.name} ({len(rm.registers())} capabilities)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
