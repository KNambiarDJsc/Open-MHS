"""A generic SiLA 2 client as an out-of-process OpenMHS adapter.

The adapter connects to one SiLA 2 server, reads the Feature Definitions the server reports
(``SiLAService.GetFeatureDefinition``), and builds the OpenMHS descriptor from them:

- a property becomes a readable property, its type, range, set and unit taken from the
  definition's constraints;
- a command becomes an action **only if a reviewed override file names it**.

A Feature Definition does not say whether a command is safe, idempotent, reversible or how
its effect can be checked. Those come from the override, never from a command's name.
Anything that cannot be mapped (a unit with no OpenMHS equivalent, a Real without a unit, an
observable command, a structure) is left out and listed with its reason in the mapping report
(``config.report_file``).

Units are matched by structure, not by label. A SiLA unit is a product of SI base units with
a factor and an offset, so (Kelvin^1, factor 1, offset 273.15) is OpenMHS ``Cel`` whatever
its label says.

Errors are not hidden:
- a SiLA validation error means the server refused the parameters, so nothing was applied
  (``Rejected``);
- a defined execution error is ``Rejected`` only if the override says it is raised before
  any effect; otherwise it is a device failure;
- a lost connection or a timeout leaves the outcome uncertain.
"""

from __future__ import annotations

import asyncio
import json
import re
import xml.etree.ElementTree as ET
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from urllib.parse import urlsplit

from sila2.client import SilaClient
from sila2.framework.errors.defined_execution_error import DefinedExecutionError
from sila2.framework.errors.validation_error import ValidationError

from openmhs.adapter import Adapter, AdapterError, Applied, Rejected, quantity, reading, run

NS = {"s": "http://www.sila-standard.org"}

# (SI base units with exponents, factor, offset) -> (OpenMHS quantity, unit)
UNITS = {
    ((("Kelvin", 1),), 1.0, 0.0): ("temperature", "K"),
    ((("Kelvin", 1),), 1.0, 273.15): ("temperature", "Cel"),
    ((("Meter", 1),), 1.0, 0.0): ("length", "m"),
    ((("Meter", 1),), 1e-3, 0.0): ("length", "mm"),
    ((("Meter", 1),), 1e-6, 0.0): ("length", "um"),
    ((("Second", 1),), 1.0, 0.0): ("time", "s"),
    ((("Second", 1),), 1e-3, 0.0): ("time", "ms"),
    ((("Meter", 3),), 1e-3, 0.0): ("volume", "L"),
    ((("Meter", 3),), 1e-6, 0.0): ("volume", "mL"),
    ((("Meter", 3),), 1e-9, 0.0): ("volume", "uL"),
}


#: The SDK's conformance faults (spec/conformance.md §3). They act here, between the adapter
#: and a real SiLA server, and only under `openmhs conformance`. A server-side fault would need
#: every server to implement it, which a generic client cannot ask for.
FIXTURE = {"no_response": "hang_before_apply", "lost_ack": "apply_then_hang",
           "garbled": "apply_then_malformed", "refuse": "reject"}


def snake(name: str) -> str:
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


class Unmappable(Exception):
    pass


def text(node, tag) -> str | None:
    found = node.find(f"s:{tag}", NS)
    return found.text.strip() if found is not None and found.text else None


def map_type(data_type) -> dict:
    """An FDL <DataType> element as an OpenMHS value spec, or Unmappable with the reason."""
    basic = text(data_type, "Basic")
    constraints = None
    constrained = data_type.find("s:Constrained", NS)
    if constrained is not None:
        basic = text(constrained.find("s:DataType", NS), "Basic")
        constraints = constrained.find("s:Constraints", NS)
    if basic is None:
        raise Unmappable("a list, structure or data type identifier: no OpenMHS value type")
    spec: dict = {}
    unit = None
    if constraints is not None:
        u = constraints.find("s:Unit", NS)
        if u is not None:
            parts = tuple(sorted((text(c, "SIUnit"), int(text(c, "Exponent"))) for c in u.findall("s:UnitComponent", NS)))
            key = (parts, float(text(u, "Factor")), float(text(u, "Offset")))
            if key not in UNITS:
                raise Unmappable(f"SiLA unit {text(u, 'Label')!r} {key} has no OpenMHS equivalent")
            unit = UNITS[key]
        lo = text(constraints, "MinimalInclusive") or text(constraints, "MinimalExclusive")
        hi = text(constraints, "MaximalInclusive") or text(constraints, "MaximalExclusive")
        if lo is not None or hi is not None:
            spec["range"] = {k: float(v) for k, v in (("min", lo), ("max", hi)) if v is not None}
        values = constraints.find("s:Set", NS)
        if values is not None:
            return {"type": "enum", "values": [v.text for v in values.findall("s:Value", NS)]}
        if text(constraints, "MaximalLength"):
            spec["max_length"] = int(text(constraints, "MaximalLength"))
    if basic == "Real":
        if unit is None:
            raise Unmappable("a Real without a unit: OpenMHS numbers always carry one")
        return {"type": "number", "quantity": unit[0], "unit": unit[1], **spec}
    if basic == "Integer":
        return {"type": "integer", **({"quantity": unit[0], "unit": unit[1]} if unit else {}), **spec}
    if basic == "Boolean":
        return {"type": "boolean"}
    if basic == "String":
        return {"type": "string", **{k: v for k, v in spec.items() if k == "max_length"}}
    raise Unmappable(f"SiLA basic type {basic} has no OpenMHS value type")


def within(override: dict, native: dict) -> bool:
    lo, hi = native.get("range", {}).get("min", float("-inf")), native.get("range", {}).get("max", float("inf"))
    r = override.get("range", {})
    return lo <= r.get("min", lo) and r.get("max", hi) <= hi


class SilaAdapter(Adapter):
    adapter_id = "openmhs.py.sila2"
    version = "0.1.0"
    fixture_faults = ("link_down", "no_response", "lost_ack", "garbled", "refuse")
    fixture_counts_deliveries = True

    def __init__(self) -> None:
        self.client = None
        # gRPC calls block, and one that never returns keeps its thread. Reads get their own
        # threads: reconciling a hung command needs a read, and hung commands must not be able
        # to use them all up.
        self.pool = ThreadPoolExecutor(max_workers=4, thread_name_prefix="sila-read")
        self.commands_pool = ThreadPoolExecutor(max_workers=4, thread_name_prefix="sila-command")
        self.deliveries = 0
        self.link_down = False
        self.fixture: str | None = None
        self.reading_source = "device"
        self.properties: dict[str, tuple[str, str, dict]] = {}  # name -> (feature, identifier, spec)
        self.commands: dict[str, tuple[str, str, dict, dict]] = {}  # name -> (feature, id, params, override)
        self.descriptor: dict = {}
        self.report: dict = {}

    async def initialize(self, config, permissions):
        await super().initialize(config, permissions)
        url = config["server"]  # sila://host:port
        permissions.check_network(url)
        parts = urlsplit(url)
        self.host, self.port = parts.hostname, parts.port
        self.call_timeout_s = float(config.get("call_timeout_ms", 3000)) / 1000
        self.overrides = json.loads(Path(permissions.check_read(config["overrides"])).read_text(encoding="utf-8"))
        self.report_file = config.get("report_file")
        # What the server on the other end is. The adapter cannot tell a simulated SiLA server
        # from an instrument, so whoever configures it says; conformance always runs on a test
        # server. Nothing may claim "simulator" for a device, or "device" for a simulator.
        self.reading_source = "simulator" if self.fixture_mode else config.get("reading_source", "device")
        await self._call(self._connect)
        self._build()
        if self.report_file:
            Path(self.report_file).write_text(json.dumps(self.report, indent=1), encoding="utf-8")

    def _connect(self) -> None:
        self.client = SilaClient(self.host, self.port, insecure=True)
        self.client.SiLAService.ServerName.get()

    async def _call(self, fn, *args):
        loop = asyncio.get_running_loop()
        return await asyncio.wait_for(loop.run_in_executor(self.pool, fn, *args), self.call_timeout_s)

    def _build(self) -> None:
        svc = self.client.SiLAService
        o = self.overrides
        report = {"server": {"name": svc.ServerName.get(), "type": svc.ServerType.get(),
                             "uuid": svc.ServerUUID.get(), "version": svc.ServerVersion.get()},
                  "mapped": [], "excluded": [], "from_override": []}
        caps = []
        for fq in svc.ImplementedFeatures.get():
            fdl = ET.fromstring(svc.GetFeatureDefinition(FeatureIdentifier=fq).FeatureDefinition.encode())
            feature = text(fdl, "Identifier")
            if feature not in o.get("features", {}):
                report["excluded"].append({"element": fq, "reason": "feature not named in the override file"})
                continue
            fo = o["features"][feature]
            prefix = snake(feature)
            for p in fdl.findall("s:Property", NS):
                ident = text(p, "Identifier")
                name = f"{prefix}.{snake(ident)}"
                try:
                    spec = map_type(p.find("s:DataType", NS))
                except Unmappable as e:
                    report["excluded"].append({"element": f"{feature}.{ident}", "reason": str(e)})
                    continue
                self.properties[name] = (feature, ident, spec)
                caps.append({"name": name, "kind": "property", "operations": ["read"],
                             "description": text(p, "Description"), "value": spec, "side_effect": "none",
                             "freshness": {"max_age_ms": fo.get("freshness_ms", 2000)}})
                report["mapped"].append({"element": f"{feature}.{ident}", "as": name,
                                         "observable": text(p, "Observable") == "Yes",
                                         "note": "observable: exposed as read only (no subscription)"
                                         if text(p, "Observable") == "Yes" else None})
            for c in fdl.findall("s:Command", NS):
                ident = text(c, "Identifier")
                co = fo.get("commands", {}).get(ident)
                if co is None:
                    report["excluded"].append({"element": f"{feature}.{ident}",
                                               "reason": "command not reviewed in the override file: not exposed"})
                    continue
                if text(c, "Observable") == "Yes":
                    report["excluded"].append({"element": f"{feature}.{ident}",
                                               "reason": "observable command: not supported by this adapter"})
                    continue
                params, sila_params = {}, {}
                try:
                    for prm in c.findall("s:Parameter", NS):
                        pid = text(prm, "Identifier")
                        native = map_type(prm.find("s:DataType", NS))
                        narrowed = co.get("params", {}).get(pid)
                        if narrowed:
                            if not within(narrowed, native):
                                raise Unmappable(f"override for {pid} widens the server's own range")
                            native = {**native, **narrowed}
                        params[snake(pid)] = native
                        sila_params[snake(pid)] = pid
                except Unmappable as e:
                    report["excluded"].append({"element": f"{feature}.{ident}", "reason": str(e)})
                    continue
                name = f"{prefix}.{snake(ident)}"
                self.commands[name] = (feature, ident, sila_params, co)
                caps.append({"name": name, "kind": "action", "operations": ["invoke"],
                             "description": text(c, "Description"), "params": params,
                             **{k: co[k] for k in ("side_effect", "idempotent", "timing", "verification",
                                                   "preconditions") if k in co}})
                report["mapped"].append({"element": f"{feature}.{ident}", "as": name})
                report["from_override"].append({"element": f"{feature}.{ident}",
                                                "fields": sorted(k for k in co if k != "params"),
                                                "narrowed_params": sorted(co.get("params", {}))})
        self.report = report
        self.descriptor = {
            "openmhs": "0.1",
            "device": {"id": o["device_id"], "name": report["server"]["name"],
                       "manufacturer": o.get("manufacturer", "unknown (SiLA 2 server)"),
                       "model": report["server"]["type"], "serial": report["server"]["uuid"],
                       "firmware": report["server"]["version"]},
            "adapter": {"id": self.adapter_id, "version": self.version},
            "capabilities": caps,
        }

    async def describe(self):
        return self.descriptor

    def _link(self) -> None:
        if self.link_down:
            raise AdapterError("disconnected", "simulated disconnect")

    async def connect(self):
        self._link()
        try:
            await self._call(lambda: self.client.SiLAService.ServerName.get())
        except asyncio.TimeoutError:
            raise AdapterError("disconnected", f"no answer from the SiLA server within {self.call_timeout_s} s")
        except Exception as e:
            raise AdapterError("disconnected", f"SiLA: {type(e).__name__}: {e}")

    async def fixture_inject(self, fault):
        if fault == "link_down":
            self.link_down = True
        else:
            self.fixture = FIXTURE[fault]

    async def fixture_clear(self):
        self.link_down = False
        self.fixture = None

    async def shutdown(self):
        for pool in (self.pool, self.commands_pool):
            pool.shutdown(wait=False, cancel_futures=True)

    async def health(self):
        if self.link_down:
            return ("disconnected", "simulated disconnect")
        try:
            await self._call(lambda: self.client.SiLAService.ServerName.get())
            return "healthy"
        except Exception as e:
            return ("disconnected", f"{type(e).__name__}: {e}")

    async def read(self, capability):
        self._link()
        if capability not in self.properties:
            raise AdapterError("unsupported", f"no readable capability {capability!r}")
        feature, ident, spec = self.properties[capability]
        try:
            value = await self._call(lambda: getattr(getattr(self.client, feature), ident).get())
        except asyncio.TimeoutError:
            raise AdapterError("timeout", f"no answer from the SiLA server within {self.call_timeout_s} s")
        except Exception as e:
            raise AdapterError("disconnected", f"SiLA: {type(e).__name__}: {e}")
        if spec["type"] == "number" or (spec["type"] == "integer" and "unit" in spec):
            value = quantity(value, spec["unit"])
        return reading(value, source=self.reading_source)

    async def execute(self, command, progress):
        self.deliveries += 1
        self.report_deliveries(self.deliveries)
        self._link()
        if command.capability not in self.commands:
            raise AdapterError("unsupported", f"cannot execute {command.capability!r}")
        feature, ident, sila_params, override = self.commands[command.capability]
        args = {}
        for name, pid in sila_params.items():
            v = command.params.get(name)
            args[pid] = v["value"] if isinstance(v, dict) else v
        timeout = override.get("timing", {}).get("timeout_ms", 3000) / 1000
        call = lambda: getattr(getattr(self.client, feature), ident)(**args)
        loop = asyncio.get_running_loop()
        fault = self.fixture
        if fault == "hang_before_apply":
            await asyncio.Event().wait()
        if fault == "reject":
            return Rejected("simulated refusal (conformance fixture)")
        try:
            await asyncio.wait_for(loop.run_in_executor(self.commands_pool, call), timeout)
        except asyncio.TimeoutError:
            # The server may still be executing it: the outcome is not known.
            raise AdapterError("timeout", f"{feature}.{ident}: no answer within {timeout} s")
        except ValidationError as e:
            return Rejected(f"SiLA ValidationError: {e.message}")
        except DefinedExecutionError as e:
            detail = f"SiLA DefinedExecutionError {e.fully_qualified_identifier}: {e.message}"
            if e.identifier in override.get("defined_errors_before_effect", []):
                return Rejected(detail)
            raise AdapterError("device_failure", detail)
        except Exception as e:
            raise AdapterError("disconnected", f"SiLA: {type(e).__name__}: {e}")
        progress.accepted()
        if fault == "apply_then_malformed":
            raise AdapterError("malformed_response", "simulated garbled response (conformance fixture)")
        if fault == "apply_then_hang":
            await asyncio.Event().wait()
        return Applied({})


if __name__ == "__main__":
    run(SilaAdapter())
