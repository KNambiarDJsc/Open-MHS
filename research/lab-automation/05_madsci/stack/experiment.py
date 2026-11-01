"""The closed-loop experiment: find the dose with half the maximal response (the EC50).

A deterministic bisection proposes a dose; MADSci runs a two-step workflow (dispense it into a
fresh well, measure the well); the measured response narrows the bracket. The synthetic system
has a known EC50 (42 uL in nodes.py), so the result can be checked against the truth.

Used by run_scenarios.py; also runnable on its own against a running workcell:

    python experiment.py --workcell http://127.0.0.1:2100 --iterations 6 --tag run1
"""

from __future__ import annotations

import argparse
import json
import os
import time

from madsci.client.workcell_client import WorkcellClient
from madsci.common.types.workflow_types import WorkflowDefinition

WELLS = [f"B{i}" for i in range(1, 13)]
LO, HI = 5.0, 150.0


def workflow(well: str, dose_ul: float, key: str) -> WorkflowDefinition:
    return WorkflowDefinition(name=f"dose {dose_ul} uL into {well}", steps=[
        {"name": "dispense", "key": "dispense", "node": "liquid_handler", "action": "dispense_dose",
         "args": {"well": well, "dose_ul": dose_ul, "key": key}},
        {"name": "measure", "key": "measure", "node": "plate_reader", "action": "measure", "args": {"well": well}},
    ])


def run_iteration(client: WorkcellClient, i: int, dose: float, tag: str, well_offset: int = 0) -> dict:
    """One iteration; returns what each layer recorded. Does not retry."""
    well, key = WELLS[well_offset + i], f"{tag}-iter{i}-dispense"
    t = time.perf_counter()
    wf = client.start_workflow(workflow(well, dose, key), await_completion=True, prompt_on_error=False,
                               raise_on_failed=False, raise_on_cancelled=False)
    record = {"iteration": i, "well": well, "planned_dose_ul": dose, "key": key, "workflow_id": wf.workflow_id,
              "workflow_status": status_of(wf), "seconds": round(time.perf_counter() - t, 2)}
    record.update(steps_of(wf))
    return record


def result_of(step):
    """A step's JSON result. The workcell stores it in MADSci's data manager as a datapoint and
    keeps only its id in the step, so it is read back from there."""
    if step.result is None:
        return None
    if step.result.json_result is not None:
        return step.result.json_result
    ids = step.result.datapoints.model_dump(mode="json") if step.result.datapoints else {}
    if "json_result" not in ids:
        return None
    from madsci.client.data_client import DataClient

    return DataClient(os.environ["DATA_SERVER_URL"]).get_datapoint_value(ids["json_result"])


def steps_of(wf) -> dict:
    out = {}
    for name in ("dispense", "measure"):
        step = wf.get_step_by_key(name)
        out[name] = {"status": str(step.status.value),
                     "result": result_of(step),
                     "errors": [e.message for e in (step.result.errors if step.result else [])],
                     "results_recorded": len(step.history)}  # status updates MADSci kept, not retries
    return out


def status_of(wf) -> str:
    s = wf.status
    for flag in ("completed", "failed", "cancelled", "paused"):
        if getattr(s, flag, False):
            return flag
    return "running"


def bisect(records: list[dict], dose: float, response: float, lo: float, hi: float) -> tuple[float, float]:
    return (dose, hi) if response < 0.5 else (lo, dose)


def run(workcell: str, iterations: int, tag: str, well_offset: int = 0) -> dict:
    client = WorkcellClient(workcell)
    lo, hi, records = LO, HI, []
    for i in range(iterations):
        dose = round((lo + hi) / 2, 1)
        rec = run_iteration(client, i, dose, tag, well_offset)
        records.append(rec)
        if rec["workflow_status"] != "completed":
            break
        lo, hi = bisect(records, dose, rec["measure"]["result"]["response"], lo, hi)
    return {"tag": tag, "records": records, "ec50_estimate_ul": round((lo + hi) / 2, 2), "bracket": [lo, hi]}


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--workcell", required=True)
    ap.add_argument("--iterations", type=int, default=6)
    ap.add_argument("--tag", default="run")
    a = ap.parse_args()
    print(json.dumps(run(a.workcell, a.iterations, a.tag), indent=1, default=str))
