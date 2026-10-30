"""Check unit metadata through LabThings TD generation and OpenMHS import.

Run with a Python environment that has ``labthings-fastapi`` installed:

    python research/experiments/labthings_units_probe.py

This is an in-memory Thing Description probe. It starts no server and accesses no hardware.
"""

from __future__ import annotations

import json
import sys
from importlib.metadata import version
from pathlib import Path
from typing import Annotated

from pydantic import Field

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "adapters" / "python" / "wot"))

import td_import  # noqa: E402
import labthings_fastapi as lt  # noqa: E402
from labthings_fastapi.testing import create_thing_without_server  # noqa: E402


class UnitProbeThing(lt.Thing):
    position_x: Annotated[float, Field(json_schema_extra={"unit": "mm"})] = lt.property(
        default=0.0, readonly=True
    )

    @lt.property
    def focus_position(self) -> Annotated[float, Field(json_schema_extra={"unit": "mm"})]:
        return 0.0

    @lt.action
    def move_x(self, x: Annotated[float, Field(json_schema_extra={"unit": "mm"})]):
        return x


def main() -> int:
    td = create_thing_without_server(UnitProbeThing).thing_description_dict()
    property_units = {
        name: affordance.get("unit")
        for name, affordance in (td.get("properties") or {}).items()
    }
    action_x = td["actions"]["move_x"]["input"]["properties"]["x"]
    mapping = td_import.map_thing(
        td, base_url="http://127.0.0.1:5000", device_id="units.probe"
    )
    imported = next(
        cap for cap in mapping.descriptor["capabilities"] if cap["name"] == "position_x"
    )
    report = {
        "labthings_fastapi_version": version("labthings-fastapi"),
        "property_units": property_units,
        "action_input_x_unit": action_x.get("unit"),
        "openmhs_import_position_x": imported["value"],
        "server_started": False,
        "hardware_accessed": False,
    }
    assert property_units == {"position_x": "mm", "focus_position": "mm"}, report
    assert action_x.get("unit") == "mm", report
    assert imported["value"].get("quantity") == "length", report
    assert imported["value"].get("unit") == "mm", report
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
