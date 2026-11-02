"""OT native baseline (no OpenMHS): the reviewed serial dilution through opentrons.simulate.

Prints a JSON record: the run log size and per-command counts, and what the simulator
refuses for a set of bad parameter values. Nothing here touches a robot.

    python baseline.py
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import time
import sys
from collections import Counter
from pathlib import Path

import opentrons
from opentrons import simulate

PROTOCOL = Path(__file__).resolve().parents[4] / "adapters" / "python" / "opentrons" / "protocols" / "serial_dilution.py"


def load_protocol():
    spec = importlib.util.spec_from_file_location("serial_dilution", PROTOCOL)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run(params: dict) -> dict:
    """One simulated run through the documented interactive API."""
    protocol = load_protocol()
    ctx = simulate.get_protocol_api("2.20", robot_type="Flex")
    t = time.perf_counter()
    try:
        protocol.dilute(ctx, params)
    except Exception as e:
        return {"params": params, "ok": False, "error": f"{type(e).__name__}: {str(e)[:300]}",
                "commands_before_error": len(ctx.commands())}
    commands = ctx.commands()
    kinds = Counter(c.split(" ")[0] for c in commands)
    return {"params": params, "ok": True, "commands": len(commands), "ms": round((time.perf_counter() - t) * 1000),
            "verbs": dict(kinds.most_common(8))}


def run_file() -> dict:
    """The stock path: opentrons.simulate.simulate on the file, with its defaults."""
    with open(PROTOCOL, encoding="utf-8") as f:
        runlog, _ = simulate.simulate(f, file_name=PROTOCOL.name)
    return {"commands": len(runlog), "top_level": sum(1 for e in runlog if e["level"] == 0)}


def main() -> None:
    good = {"n_wells": 6, "transfer_ul": 50.0, "diluent_ul": 100.0, "mix_reps": 3}
    probes = {
        "valid": good,
        "transfer above the pipette (400 uL)": {**good, "transfer_ul": 400.0},
        "more wells than a row (14)": {**good, "n_wells": 14},
        "overfill a 200 uL well (diluent 190 + transfer 150)": {**good, "diluent_ul": 190.0, "transfer_ul": 150.0},
        "negative volume": {**good, "transfer_ul": -5.0},
        "zero wells": {**good, "n_wells": 0},
    }
    print(json.dumps({
        "opentrons": opentrons.__version__,
        "python": sys.version.split()[0],
        "protocol_sha256": hashlib.sha256(PROTOCOL.read_bytes()).hexdigest(),
        "simulate_file_defaults": run_file(),
        "runs": {name: run(p) for name, p in probes.items()},
    }, indent=2))


if __name__ == "__main__":
    main()
