"""MADSci nodes for the closed-loop experiment.

``liquid_handler`` dispenses a dose of stock into a well. In ``openmhs`` mode every transfer
goes through an OpenMHS gateway, which governs the PyLabRobot simulator: authorization, the
contract, an idempotency key the workflow step carries, unknown outcomes, approval and the
audit trail. In ``native`` mode the node drives the same PyLabRobot simulator in-process,
with no gateway and no identity for a transfer: baseline A.

``plate_reader`` measures a well. It is a plain MADSci node (no OpenMHS). It reads the dose
actually in the well from the PyLabRobot simulator's saved state, not from what was planned,
so a lost or doubled transfer changes the measurement. The response is a known logistic of
the dose plus seeded noise: a synthetic system with a ground truth, not a chemical model.

    OPENMHS_TOKEN=T python nodes.py liquid_handler --port 2101 --mode openmhs --gateway URL
    python nodes.py liquid_handler --port 2101 --mode native --state-file F
    python nodes.py plate_reader  --port 2102 --state-file F --seed 7
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
import sys
import uuid
from pathlib import Path

from madsci.common.types.action_types import ActionFailed, ActionResult, ActionStatus, ActionSucceeded
from madsci.common.types.base_types import Error
from madsci.common.types.node_types import RestNodeConfig
from madsci.node_module.helpers import action
from madsci.node_module.rest_node_module import RestNode

REPO = Path(__file__).resolve().parents[4]
sys.path[:0] = [str(REPO / "python"), str(REPO / "adapters" / "python" / "pylabrobot"),
                str(Path(__file__).resolve().parent)]

from governed import FAILED, SUCCEEDED, GovernedStep, StepResult  # noqa: E402

DEVICE = "plr.lh-01"
EMAX, EC50_UL, HILL, NOISE = 1.0, 42.0, 1.6, 0.01  # the synthetic system's ground truth


def response(dose_ul: float, seed: int, well: str) -> float:
    """The synthetic measurement: a logistic dose-response with deterministic noise."""
    h = hashlib.sha256(f"{seed}:{well}:{dose_ul:.3f}".encode()).digest()
    noise = (int.from_bytes(h[:4], "big") / 2**32 - 0.5) * 2 * NOISE
    clean = 0.0 if dose_ul <= 0 else EMAX * dose_ul**HILL / (EC50_UL**HILL + dose_ul**HILL)
    return round(clean + noise, 5)


class LiquidHandlerConfig(RestNodeConfig):
    mode: str = "openmhs"
    gateway: str = ""
    state_file: str = ""
    native_fault_file: str = ""
    approval_wait_s: float = 60.0


class LiquidHandler(RestNode):
    config: LiquidHandlerConfig = LiquidHandlerConfig()
    config_model = LiquidHandlerConfig

    def startup_handler(self) -> None:
        if self.config.mode == "openmhs":
            from openmhs import Client

            # The timeout also covers the source-well reads made while building a request.
            self.client = Client(self.config.gateway, os.environ["OPENMHS_TOKEN"], timeout=30)
            self.step = GovernedStep(self.client, DEVICE, approval_wait_s=self.config.approval_wait_s)
        else:
            self._native_start()

    # ---------------------------------------------------------------- governed

    def _source(self, dose_ul: float) -> str:
        """The first source well holding enough stock, read through OpenMHS."""
        for well in ("A1", "A2", "A3"):
            v = self.client.read(DEVICE, f"source.{well.lower()}.volume").to_dict()["state"]["value"]["value"]
            if v >= dose_ul:
                return well
        return "A1"  # let the contract or PyLabRobot refuse it

    def _governed(self, well: str, dose_ul: float, key: str) -> ActionResult:
        def request() -> dict:
            # Built only for a new command, so a retry keeps the source its command named
            # even if that well has since run low: a different one would be a different
            # request under the same key, which OpenMHS refuses.
            return {"target": {"device": DEVICE, "capability": "liquid.transfer"}, "operation": "invoke",
                    "params": {"transfer_id": key, "source": self._source(dose_ul), "dest": well,
                               "volume": {"value": dose_ul, "unit": "uL"}}}

        return as_action_result(self.step.run(key, {"transfer_id": key, "well": well, "dose_ul": dose_ul}, request))

    # ---------------------------------------------------------------- native

    def _native_start(self) -> None:
        from openmhs.adapter import Permissions
        from plr_adapter import PyLabRobotAdapter

        state = Path(self.config.state_file)
        self.sim = PyLabRobotAdapter()
        config = {"state_file": str(state), "tips": 96, "source_ul": {"A1": 360, "A2": 360, "A3": 360},
                  "dest_wells": [f"B{i}" for i in range(1, 13)]}
        asyncio.run(self.sim.initialize(config, Permissions(filesystem_write=(str(state.parent),))))

    def _native(self, well: str, dose_ul: float) -> ActionResult:
        from openmhs.adapter import Command

        class _Progress:
            def accepted(self, external_ref=None):
                pass

        source = next((w for w in ("A1", "A2", "A3")
                       if self.sim.source.get_well(w).tracker.get_used_volume() >= dose_ul), "A1")
        # PyLabRobot alone has no identity for a transfer: every call is a new one.
        command = Command(command_id=str(uuid.uuid4()), capability="liquid.transfer", operation="invoke", value=None,
                          params={"transfer_id": str(uuid.uuid4()), "source": source, "dest": well,
                                  "volume": {"value": dose_ul, "unit": "uL"}},
                          idempotent=False, deadline="")
        result = asyncio.run(self.sim.execute(command, _Progress()))
        record = {"source": source, "well": well, "dose_ul": dose_ul, "result": type(result).__name__}
        fault = Path(self.config.native_fault_file) if self.config.native_fault_file else None
        if fault and fault.exists():
            fault.unlink()
            # The transfer happened; the answer to MADSci is lost (the lost-ack analogue).
            return ActionFailed(json_result=record,
                                errors=[Error(message="simulated: connection lost after the transfer",
                                              error_type="ConnectionLost")])
        if type(result).__name__ == "Applied":
            return ActionSucceeded(json_result=record)
        return ActionFailed(json_result=record, errors=[Error(message=str(result), error_type="PyLabRobot")])

    @action
    def dispense_dose(self, well: str, dose_ul: float, key: str):  # returns an ActionResult (MADSci refuses it as an annotation)
        """Dispense dose_ul of stock into well. key identifies this step's transfer."""
        if self.config.mode == "openmhs":
            return self._governed(well, float(dose_ul), key)
        return self._native(well, float(dose_ul))


def as_action_result(result: StepResult) -> ActionResult:
    """A step's status, as MADSci's. ``unknown`` ends a MADSci workflow without saying the
    transfer did not happen, which is what ``failed`` would say."""
    record = dict(result.record)
    source = (record.get("request_params") or {}).get("source")
    if source:
        record["source"] = source
    errors = [Error(message=message, error_type=kind) for kind, message in result.errors]
    if result.status == SUCCEEDED:
        return ActionSucceeded(json_result=record)
    if result.status == FAILED:
        return ActionFailed(json_result=record, errors=errors)
    return ActionResult(status=ActionStatus.UNKNOWN, json_result=record, errors=errors)


class PlateReaderConfig(RestNodeConfig):
    state_file: str = ""
    seed: int = 7


class PlateReader(RestNode):
    config: PlateReaderConfig = PlateReaderConfig()
    config_model = PlateReaderConfig

    @action
    def measure(self, well: str):  # returns an ActionResult
        """Measure a well: the dose actually in it, from the simulator's state, and the response."""
        state = json.loads(Path(self.config.state_file).read_text())
        dose = float(state["deck"][f"dest_well_{well}"]["volume"])
        return ActionSucceeded(json_result={"well": well, "dose_ul": dose,
                                            "response": response(dose, self.config.seed, well)})


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("node", choices=["liquid_handler", "plate_reader"])
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--mode", default="openmhs")
    ap.add_argument("--gateway", default="")
    ap.add_argument("--state-file", default="")
    ap.add_argument("--native-fault-file", default="")
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--approval-wait-s", type=float, default=60.0)
    a = ap.parse_args()
    url = f"http://127.0.0.1:{a.port}"
    if a.node == "liquid_handler":
        node = LiquidHandler(LiquidHandlerConfig(node_name="liquid_handler", node_url=url, mode=a.mode,
                                                 gateway=a.gateway, state_file=a.state_file,
                                                 native_fault_file=a.native_fault_file,
                                                 approval_wait_s=a.approval_wait_s))
    else:
        node = PlateReader(PlateReaderConfig(node_name="plate_reader", node_url=url, state_file=a.state_file,
                                             seed=a.seed))
    node.start_node()


if __name__ == "__main__":
    main()
