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

    python nodes.py liquid_handler --port 2101 --mode openmhs --gateway URL --token T
    python nodes.py liquid_handler --port 2101 --mode native --state-file F
    python nodes.py plate_reader  --port 2102 --state-file F --seed 7
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import math
import sys
import time
import uuid
from pathlib import Path

from madsci.common.types.action_types import ActionFailed, ActionResult, ActionStatus, ActionSucceeded
from madsci.common.types.base_types import Error
from madsci.common.types.node_types import RestNodeConfig
from madsci.node_module.helpers import action
from madsci.node_module.rest_node_module import RestNode

REPO = Path(__file__).resolve().parents[4]
sys.path[:0] = [str(REPO / "python"), str(REPO / "adapters" / "python" / "pylabrobot")]

DEVICE = "plr.lh-01"
EMAX, EC50_UL, HILL, NOISE = 1.0, 42.0, 1.6, 0.01  # the synthetic system's ground truth
APPROVAL_WAIT_S = 60.0


def response(dose_ul: float, seed: int, well: str) -> float:
    """The synthetic measurement: a logistic dose-response with deterministic noise."""
    h = hashlib.sha256(f"{seed}:{well}:{dose_ul:.3f}".encode()).digest()
    noise = (int.from_bytes(h[:4], "big") / 2**32 - 0.5) * 2 * NOISE
    clean = 0.0 if dose_ul <= 0 else EMAX * dose_ul**HILL / (EC50_UL**HILL + dose_ul**HILL)
    return round(clean + noise, 5)


class LiquidHandlerConfig(RestNodeConfig):
    mode: str = "openmhs"
    gateway: str = ""
    token: str = ""
    state_file: str = ""
    native_fault_file: str = ""


class LiquidHandler(RestNode):
    config: LiquidHandlerConfig = LiquidHandlerConfig()
    config_model = LiquidHandlerConfig

    def startup_handler(self) -> None:
        if self.config.mode == "openmhs":
            from openmhs import Client

            self.client = Client(self.config.gateway, self.config.token)
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
        try:
            self.client.info()
            source = self._source(dose_ul)
        except Exception as e:  # the gateway is down: nothing was sent
            return ActionFailed(json_result={"transfer_id": key, "well": well, "dose_ul": dose_ul},
                                errors=[Error(message=f"OpenMHS gateway unreachable, nothing sent: {e}",
                                              error_type="OpenMHSUnreachable")])
        request = {"target": {"device": DEVICE, "capability": "liquid.transfer"}, "operation": "invoke",
                   "idempotency_key": key,
                   "params": {"transfer_id": key, "source": source, "dest": well,
                              "volume": {"value": dose_ul, "unit": "uL"}}}
        try:
            r = self.client.call("commands.submit", {"request": request}, timeout=60)
        except Exception as e:
            # Sent, but the answer did not come back: the transfer may or may not have happened.
            return ActionResult(status=ActionStatus.UNKNOWN,
                                json_result={"transfer_id": key, "well": well, "dose_ul": dose_ul},
                                errors=[Error(message=f"no answer from OpenMHS after submitting ({type(e).__name__}): "
                                                      "reconcile in OpenMHS before retrying this step",
                                              error_type="OpenMHSOutcomeUnknown")])
        deadline = time.time() + APPROVAL_WAIT_S
        while r["state"] == "AWAITING_APPROVAL" and time.time() < deadline:
            time.sleep(0.5)
            r = self.client.call("commands.get", {"command_id": r["command_id"]})
        record = {"command_id": r["command_id"], "openmhs_state": r["state"], "transfer_id": key,
                  "source": request["params"]["source"], "well": well, "dose_ul": dose_ul,
                  "transitions": [t["state"] for t in r["transitions"]]}
        if r["state"] in ("VERIFIED", "RECONCILED_APPLIED"):
            return ActionSucceeded(json_result=record)
        if r["state"] in ("UNKNOWN_OUTCOME", "TIMED_OUT", "DISPATCHED", "ACCEPTED", "APPLIED"):
            # The transfer may or may not have happened. Say so; never report success or retry.
            return ActionResult(status=ActionStatus.UNKNOWN, json_result=record,
                                errors=[Error(message=f"OpenMHS command {r['command_id']} is {r['state']}: "
                                                      "reconcile it in OpenMHS before retrying this step",
                                              error_type="OpenMHSOutcomeUnknown")])
        errors = r["policy"].get("errors") or [{"code": r["state"], "message": json.dumps(r.get("adapter_result"))}]
        return ActionFailed(json_result=record,
                            errors=[Error(message=f"{e['code']}: {e.get('message', '')}", error_type="OpenMHS")
                                    for e in errors])

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
    ap.add_argument("--token", default="")
    ap.add_argument("--state-file", default="")
    ap.add_argument("--native-fault-file", default="")
    ap.add_argument("--seed", type=int, default=7)
    a = ap.parse_args()
    url = f"http://127.0.0.1:{a.port}"
    if a.node == "liquid_handler":
        node = LiquidHandler(LiquidHandlerConfig(node_name="liquid_handler", node_url=url, mode=a.mode,
                                                 gateway=a.gateway, token=a.token, state_file=a.state_file,
                                                 native_fault_file=a.native_fault_file))
    else:
        node = PlateReader(PlateReaderConfig(node_name="plate_reader", node_url=url, state_file=a.state_file,
                                             seed=a.seed))
    node.start_node()


if __name__ == "__main__":
    main()
