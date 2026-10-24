"""Generate spec/examples/invalid/*.json: each fixture breaks exactly one rule of a valid base.

Usage: python tools/gen_invalid_fixtures.py spec/examples/invalid
Also writes the base to spec/examples/valid/fixture-base.json and the expected codes to
spec/examples/invalid/expected.json.
"""
import copy
import json
import pathlib
import sys

OUT = pathlib.Path(sys.argv[1])
OUT.mkdir(parents=True, exist_ok=True)

BASE = {
    "openmhs": "0.1",
    "device": {"id": "fixture.dev-01", "manufacturer": "Fixture Co", "model": "F-1"},
    "adapter": {"id": "fixture.adapter", "version": "0.1.0"},
    "components": [{"id": "main"}],
    "relationships": [
        {"kind": "depends_on", "target": "fixture.power-01"},
        {"kind": "shares_resource", "resource": "fixture.bench"},
        {"kind": "frame", "component": "main", "frame": "fixture.bench", "capabilities": ["level.setpoint"]},
    ],
    "capabilities": [
        {
            "name": "level.measured",
            "kind": "property",
            "operations": ["read", "subscribe"],
            "value": {"type": "number", "quantity": "electric_potential", "unit": "V"},
            "side_effect": "none",
        },
        {
            "name": "level.setpoint",
            "kind": "property",
            "component": "main",
            "operations": ["read", "write"],
            "value": {"type": "number", "quantity": "electric_potential", "unit": "V", "range": {"min": 0, "max": 12}},
            "side_effect": "reversible",
            "idempotent": True,
            "preconditions": [{"id": "not-faulted", "capability": "fault.active", "op": "eq", "value": False}],
            "verification": {"mode": "readback", "capability": "level.measured", "tolerance": {"abs": 0.05}},
        },
        {
            "name": "fault.active",
            "kind": "property",
            "operations": ["read"],
            "value": {"type": "boolean"},
            "side_effect": "none",
        },
        {
            "name": "mode",
            "kind": "property",
            "operations": ["read"],
            "value": {"type": "enum", "values": ["auto", "manual"]},
            "side_effect": "none",
        },
        {
            "name": "cycle.run",
            "kind": "action",
            "operations": ["invoke"],
            "params": {"target": {"type": "number", "quantity": "electric_potential", "unit": "V"}},
            "side_effect": "reversible",
            "verification": {"mode": "readback", "capability": "level.measured", "expect_param": "target"},
        },
        {
            "name": "fault.raised",
            "kind": "event",
            "operations": ["subscribe"],
            "side_effect": "none",
        },
        {
            "name": "cycle.abort",
            "kind": "action",
            "operations": ["invoke"],
            "params": {},
            "stop": True,
            "side_effect": "reversible",
            "verification": {"mode": "readback", "capability": "mode", "expect": "manual"},
        },
    ],
}

CAP = {c["name"]: i for i, c in enumerate(BASE["capabilities"])}
fixtures = {}


def case(name, code, pointer=None):
    def wrap(fn):
        d = copy.deepcopy(BASE)
        fn(d)
        (OUT / f"{name}.json").write_text(json.dumps(d, indent=2) + "\n", encoding="utf-8")
        fixtures[f"{name}.json"] = {"code": code, **({"pointer": pointer} if pointer else {})}
        return fn
    return wrap


def cap(d, name):
    return d["capabilities"][CAP[name]]


SP = f"/capabilities/{CAP['level.setpoint']}"

@case("missing-device-id", "descriptor.schema", "/device")
def _(d): del d["device"]["id"]

@case("bad-device-id", "descriptor.schema", "/device/id")
def _(d): d["device"]["id"] = "My Device"

@case("unknown-top-level-field", "descriptor.schema")
def _(d): d["permissions"] = {"default": "read-write"}

@case("number-without-unit", "descriptor.schema", f"{SP}/value")
def _(d): del cap(d, "level.setpoint")["value"]["unit"]

@case("bad-extension-namespace", "descriptor.schema", "/extensions")
def _(d): d["extensions"] = {"acme.foo": 1}

@case("unsupported-version", "descriptor.version.unsupported", "/openmhs")
def _(d): d["openmhs"] = "9.9"

@case("unknown-unit", "descriptor.unit.unknown", f"{SP}/value/unit")
def _(d): cap(d, "level.setpoint")["value"]["unit"] = "volts"

@case("unknown-quantity", "descriptor.quantity.unknown", f"{SP}/value/quantity")
def _(d): cap(d, "level.setpoint")["value"]["quantity"] = "voltage"

@case("unit-quantity-mismatch", "descriptor.unit.quantity_mismatch", f"{SP}/value/unit")
def _(d): cap(d, "level.setpoint")["value"]["unit"] = "A"

@case("range-inverted", "descriptor.constraint.range_inverted", f"{SP}/value/range")
def _(d): cap(d, "level.setpoint")["value"]["range"] = {"min": 12, "max": 0}

@case("range-on-boolean", "descriptor.constraint.not_applicable")
def _(d): cap(d, "fault.active")["value"]["range"] = {"min": 0, "max": 1}

@case("slew-on-boolean", "descriptor.constraint.not_applicable")
def _(d): cap(d, "fault.active")["slew"] = {"max_change": 1, "per_ms": 1000}

@case("duplicate-capability", "descriptor.capability.duplicate")
def _(d): d["capabilities"].append(copy.deepcopy(cap(d, "mode")))

@case("unknown-component", "descriptor.component.unknown", f"{SP}/component")
def _(d): cap(d, "level.setpoint")["component"] = "nonexistent"

@case("duplicate-component", "descriptor.component.duplicate")
def _(d): d["components"].append({"id": "main"})

@case("property-without-value", "descriptor.capability.value_missing")
def _(d): del cap(d, "mode")["value"]

@case("action-with-write-operation", "descriptor.capability.invalid_operation")
def _(d): cap(d, "cycle.run")["operations"] = ["invoke", "write"]

@case("event-with-read-operation", "descriptor.capability.invalid_operation")
def _(d): cap(d, "fault.raised")["operations"] = ["read"]

@case("readonly-declares-side-effect", "descriptor.capability.side_effect_mismatch")
def _(d): cap(d, "mode")["side_effect"] = "reversible"

@case("writable-declares-no-side-effect", "descriptor.capability.side_effect_mismatch")
def _(d): cap(d, "level.setpoint")["side_effect"] = "none"

@case("writable-without-verification", "descriptor.capability.verification_missing")
def _(d): del cap(d, "level.setpoint")["verification"]

@case("readonly-with-verification", "descriptor.verification.not_applicable")
def _(d): cap(d, "mode")["verification"] = {"mode": "none", "reason": "n/a"}

@case("verification-unknown-capability", "descriptor.verification.unknown_capability")
def _(d): cap(d, "level.setpoint")["verification"]["capability"] = "level.nonexistent"

@case("verification-target-not-readable", "descriptor.verification.not_readable")
def _(d): cap(d, "level.setpoint")["verification"]["capability"] = "fault.raised"

@case("action-readback-without-expectation", "descriptor.verification.expectation_missing")
def _(d): del cap(d, "cycle.run")["verification"]["expect_param"]

@case("action-readback-without-capability", "descriptor.verification.capability_missing")
def _(d): del cap(d, "cycle.run")["verification"]["capability"]

@case("verification-unknown-param", "descriptor.verification.unknown_param")
def _(d): cap(d, "cycle.run")["verification"]["expect_param"] = "nope"

@case("precondition-unknown-capability", "descriptor.precondition.unknown_capability")
def _(d): cap(d, "level.setpoint")["preconditions"][0]["capability"] = "fault.nonexistent"

@case("precondition-target-not-readable", "descriptor.precondition.not_readable")
def _(d): cap(d, "level.setpoint")["preconditions"][0]["capability"] = "fault.raised"

@case("precondition-ordering-on-boolean", "descriptor.precondition.type_mismatch")
def _(d): cap(d, "level.setpoint")["preconditions"][0]["op"] = "lt"

@case("precondition-enum-value-not-allowed", "descriptor.precondition.type_mismatch")
def _(d): cap(d, "level.setpoint")["preconditions"] = [{"id": "auto", "capability": "mode", "op": "eq", "value": "turbo"}]

@case("precondition-number-without-unit", "descriptor.precondition.type_mismatch")
def _(d): cap(d, "level.setpoint")["preconditions"] = [{"id": "low", "capability": "level.measured", "op": "lt", "value": 5}]

@case("precondition-duplicate-id", "descriptor.precondition.duplicate_id")
def _(d): cap(d, "level.setpoint")["preconditions"].append({"id": "not-faulted", "capability": "mode", "op": "eq", "value": "auto"})

@case("property-with-params", "descriptor.capability.field_not_applicable")
def _(d): cap(d, "level.setpoint")["params"] = {}

@case("verification-ambiguous-expectation", "descriptor.verification.ambiguous_expectation")
def _(d): cap(d, "cycle.run")["verification"]["expect"] = {"value": 1, "unit": "V"}

@case("verification-param-type-mismatch", "descriptor.verification.type_mismatch")
def _(d): cap(d, "cycle.run")["params"]["target"] = {"type": "number", "quantity": "electric_current", "unit": "A"}

@case("relationship-duplicate", "descriptor.relationship.duplicate", "/relationships/3")
def _(d): d["relationships"].append({"kind": "depends_on", "target": "fixture.power-01"})

@case("relationship-unknown-component", "descriptor.component.unknown", "/relationships/2/component")
def _(d): d["relationships"][2]["component"] = "nonexistent"

@case("relationship-without-target", "descriptor.relationship.field_missing", "/relationships/0/target")
def _(d): del d["relationships"][0]["target"]

@case("relationship-field-not-applicable", "descriptor.relationship.field_not_applicable", "/relationships/1/target")
def _(d): d["relationships"][1]["target"] = "fixture.power-01"

@case("relationship-with-itself", "descriptor.relationship.self", "/relationships/0/target")
def _(d): d["relationships"][0]["target"] = d["device"]["id"]

@case("relationship-unknown-capability", "descriptor.relationship.unknown_capability", "/relationships/2/capabilities/0")
def _(d): d["relationships"][2]["capabilities"] = ["level.nonexistent"]

@case("relationship-unknown-kind", "descriptor.schema", "/relationships/0/kind")
def _(d): d["relationships"][0]["kind"] = "near"

ABORT = f"/capabilities/{CAP['cycle.abort']}"

@case("stop-on-property", "descriptor.stop.not_action", f"{SP}/stop")
def _(d): cap(d, "level.setpoint")["stop"] = True

@case("stop-with-precondition", "descriptor.stop.guarded", f"{ABORT}/preconditions")
def _(d): cap(d, "cycle.abort")["preconditions"] = [{"id": "not-faulted", "capability": "fault.active", "op": "eq", "value": False}]

@case("stop-with-rate-limit", "descriptor.stop.guarded", f"{ABORT}/rate_limit")
def _(d): cap(d, "cycle.abort")["rate_limit"] = {"max": 1, "per_ms": 1000}

(OUT.parent / "valid" / "fixture-base.json").write_text(json.dumps(BASE, indent=2) + "\n", encoding="utf-8")
(OUT / "expected.json").write_text(json.dumps(dict(sorted(fixtures.items())), indent=2) + "\n", encoding="utf-8")
print(f"wrote {len(fixtures)} invalid fixtures")
