"""One description per ecosystem, so one set of scenarios can run against all of them.

Each ecosystem says how to build its lab (reusing the experiment's own lab class), which command
is a valid one, how to make the next one distinct, how to arrange a lost answer, and how to read
how many times the simulator actually did the thing, outside OpenMHS.

Imported only in the virtual environment that has that ecosystem installed.
"""

from __future__ import annotations

import importlib.util
import json
import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

HERE = Path(__file__).resolve()
LAB = HERE.parents[1]
sys.path[:0] = [str(LAB / "shared")]

from harness import REPO, Lab  # noqa: E402


def load_experiment(folder: str):
    """An experiment's own run_scenarios.py, under a name of its own."""
    path = LAB / folder / "experiments" / "run_scenarios.py"
    spec = importlib.util.spec_from_file_location(f"exp_{folder}", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@dataclass
class Ecosystem:
    name: str
    device: str
    make_lab: Callable[[Path], Lab]
    #: A valid request; ``n`` makes it distinct from the others.
    request: Callable[[int], dict]
    #: A request the contract must refuse before dispatch.
    invalid: dict
    #: The next command loses its answer after applying.
    lost_answer: Callable[[Lab], None]
    #: How many times the simulator did it, read outside OpenMHS.
    executed: Callable[[Lab], int]
    grants: list[dict]
    #: A readable capability, for read and staleness checks.
    readable: str
    #: Whether the command's capability is idempotent, so a newer write may overtake it.
    idempotent: bool


def _wrap(lab: Lab, stale_flag: Path) -> None:
    """Run the adapter through stale_adapter.py, which can age its readings on request."""
    m = json.loads(lab.manifest.read_text())
    cmd = m["runtime"]["command"]
    m["runtime"]["command"] = [cmd[0], str(HERE.parent / "stale_adapter.py"), cmd[1]]
    m["runtime"].setdefault("env", {})["OPENMHS_STALE_FLAG"] = str(stale_flag)
    lab.manifest.write_text(json.dumps(m, indent=2))
    lab.stale_flag = stale_flag


def unwrap(lab: Lab) -> None:
    """Back to the adapter on its own: conformance tests the adapter, not the test hook."""
    m = json.loads(lab.manifest.read_text())
    cmd = m["runtime"]["command"]
    if len(cmd) == 3 and cmd[1].endswith("stale_adapter.py"):
        m["runtime"]["command"] = [cmd[0], cmd[2]]
    m["runtime"]["env"].pop("OPENMHS_STALE_FLAG", None)
    lab.manifest.write_text(json.dumps(m, indent=2))


def _invoke(device: str, capability: str, params: dict) -> dict:
    return {"target": {"device": device, "capability": capability}, "operation": "invoke", "params": params}


def pylabrobot() -> Ecosystem:
    device, transfer = "plr.lh-01", "liquid.transfer"
    adapter = REPO / "adapters" / "python" / "pylabrobot" / "plr_adapter.py"

    def make(root: Path) -> Lab:
        root.mkdir(parents=True)
        state, fault = root / "plr_state.json", root / "plr_fault.json"
        lab = Lab(root, {
            "openmhs_manifest": "0.1",
            "adapter": {"id": "openmhs.py.pylabrobot", "version": "0.1.0"},
            "runtime": {"command": [sys.executable, str(adapter)], "env": {"PYTHONPATH": str(REPO / "python")},
                        "protocol": ["0.1"]},
            "capabilities": {"allow": [{"name": "*", "operations": ["read"]},
                                       {"name": transfer, "operations": ["invoke"]}]},
            "filesystem": {"write": [str(root)]},
            "config": {"state_file": str(state), "fault_file": str(fault), "tips": 96,
                       "source_ul": {"A1": 360, "A2": 360, "A3": 360}},
        })
        lab.state_file, lab.fault_file = state, fault
        _wrap(lab, root / "stale.flag")
        return lab

    return Ecosystem(
        name="pylabrobot", device=device, make_lab=make,
        request=lambda n: _invoke(device, transfer, {"transfer_id": f"gap-{n}", "source": "A1", "dest": "B1",
                                                     "volume": {"value": 10.0, "unit": "uL"}}),
        invalid=_invoke(device, transfer, {"transfer_id": "gap-bad", "source": "A1", "dest": "B1",
                                           "volume": {"value": 5000.0, "unit": "uL"}}),
        lost_answer=lambda lab: lab.fault_file.write_text(json.dumps({"next_transfer": "apply_then_hang"})),
        executed=lambda lab: json.loads(lab.state_file.read_text())["count"],
        grants=[{"id": "read", "device": device, "capability": "*", "operations": ["read"], "effect": "allow"},
                {"id": "go", "device": device, "capability": transfer, "operations": ["invoke"], "effect": "allow"}],
        readable="transfer.count", idempotent=False)


def opentrons() -> Ecosystem:
    exp = load_experiment("02_opentrons")
    device, run = exp.DEVICE, exp.RUN

    def make(root: Path) -> Lab:
        lab = exp.OtLab(root)
        _wrap(lab, root / "stale.flag")
        return lab

    def params(n: int, **over) -> dict:
        d = exp.DEFAULTS
        return {"run_id": f"gap-{n}", "protocol": "serial_dilution", "n_wells": d["n_wells"],
                "transfer_ul": {"value": d["transfer_ul"], "unit": "uL"},
                "diluent_ul": {"value": d["diluent_ul"], "unit": "uL"}, "mix_reps": d["mix_reps"], **over}

    return Ecosystem(
        name="opentrons", device=device, make_lab=make,
        request=lambda n: _invoke(device, run, params(n)),
        invalid=_invoke(device, run, params(0, run_id="gap-bad", transfer_ul={"value": 5000.0, "unit": "uL"})),
        lost_answer=lambda lab: lab.fault_file.write_text(json.dumps({"next_transfer": "apply_then_hang"})),
        executed=lambda lab: json.loads(lab.state_file.read_text())["count"] if lab.state_file.exists() else 0,
        grants=exp.GRANTS, readable="run.count", idempotent=False)


def micromanager() -> Ecosystem:
    exp = load_experiment("03_micromanager")
    exp.MM_DIR = Path(os.environ["MM_DIR"]).resolve()
    device = exp.DEVICE

    def make(root: Path) -> Lab:
        lab = exp.MmLab(root)
        _wrap(lab, root / "stale.flag")
        return lab

    def request(n: int) -> dict:
        return {"target": {"device": device, "capability": "stage.x.position"}, "operation": "write",
                "value": {"value": 10.0 + 5 * n, "unit": "um"}}

    def executed(lab: Lab) -> int:
        # Moves leave no counter in Micro-Manager. Each one is recognisable by its target.
        return 0

    return Ecosystem(
        name="micromanager", device=device, make_lab=make, request=request,
        invalid={"target": {"device": device, "capability": "stage.x.position"}, "operation": "write",
                 "value": {"value": 1e9, "unit": "um"}},
        lost_answer=lambda lab: lab.fault("apply_then_hang"),
        executed=executed, grants=exp.GRANTS, readable="stage.x.position", idempotent=True)


def sila2() -> Ecosystem:
    exp = load_experiment("04_sila2")
    exp.SERVER = os.environ.get("SILA_SERVER", "heater_a")
    device = exp.DEVICE

    def make(root: Path) -> Lab:
        lab = exp.SilaLab(root)
        _wrap(lab, root / "stale.flag")
        return lab

    def request(n: int) -> dict:
        return _invoke(device, exp.SET, {"target_temperature": exp.degc(20.0 + n)})

    return Ecosystem(
        name="sila2", device=device, make_lab=make, request=request,
        invalid=_invoke(device, exp.SET, {"target_temperature": exp.degc(500.0)}),
        lost_answer=lambda lab: lab.fault({"next_set": "apply_then_delay", "delay_s": 8}),
        executed=lambda lab: 0, grants=exp.GRANTS, readable=exp.TARGET, idempotent=True)


ALL: dict[str, Callable[[], Ecosystem]] = {"pylabrobot": pylabrobot, "opentrons": opentrons,
                                           "micromanager": micromanager, "sila2": sila2}


def describe_request(request: dict, key: str | None = None) -> dict[str, Any]:
    return {**request, **({"idempotency_key": key} if key else {})}
