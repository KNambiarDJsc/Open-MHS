"""The Thing Description importer, offline (no server needed).

    python -m unittest adapters/python/wot/test_td_import.py

The OpenFlexure fixture is the simulator's real output (research/fixtures/), captured from
`openflexure-microscope-server` 3.0.0-beta1 on LabThings-FastAPI 0.3.0.
"""

from __future__ import annotations

import json
import re
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import td_import as t  # noqa: E402

FIXTURE = HERE.parents[2] / "research" / "fixtures" / "openflexure-sim-thing-descriptions.json"
NAME = re.compile(r"^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$")


def td(**kw) -> dict:
    return {"@context": "https://www.w3.org/2022/wot/td/v1.1", "title": "Lamp", "base": "http://h:1/",
            "properties": kw.get("properties", {}), "actions": kw.get("actions", {})}


def prop(**kw) -> dict:
    return {"forms": [{"href": "/p", "op": ["readproperty"]}], "readOnly": True, **kw}


def act(**kw) -> dict:
    return {"forms": [{"href": "/a", "op": ["invokeaction"]}], **kw}


class Mapping(unittest.TestCase):
    def test_a_property_becomes_a_read_only_capability_and_never_a_writable_one(self) -> None:
        m = t.map_thing(td(properties={"on": {**prop(type="boolean"), "readOnly": False,
                                              "forms": [{"href": "/on", "op": ["readproperty", "writeproperty"]}]}}))
        (c,) = m.descriptor["capabilities"]
        self.assertEqual((c["kind"], c["operations"], c["side_effect"]), ("property", ["read"], "none"))
        self.assertTrue(any(f.category == "writable_not_exposed" for f in m.findings))

    def test_units_come_from_the_description_or_are_reported_missing_never_guessed(self) -> None:
        m = t.map_thing(td(properties={
            "temp": prop(type="number", unit="degC"), "count": prop(type="integer"),
            "dist": prop(type="number"), "odd": prop(type="number", unit="furlongs")}))
        by = {c["name"]: c["value"] for c in m.descriptor["capabilities"]}
        self.assertEqual((by["temp"]["quantity"], by["temp"]["unit"]), ("temperature", "Cel"))
        self.assertNotIn("unit", by["count"])
        self.assertEqual(by["dist"]["unit"], "1")
        cats = {f.category for f in m.findings}
        self.assertIn("unit_missing", cats)
        self.assertIn("unit_unknown", cats)

    def test_nullable_and_compound_values_are_unsupported_not_approximated(self) -> None:
        m = t.map_thing(td(properties={
            "maybe": prop(oneOf=[{"type": "integer"}, {"type": "null"}]),
            "list": prop(type="array", items={"type": "string"}),
            "blob": prop(type="object")}))
        self.assertEqual(m.descriptor["capabilities"], [])
        self.assertEqual({f.category for f in m.findings if f.status == t.UNSUPPORTED},
                         {"nullable", "compound"})

    def test_an_object_of_scalars_is_flattened_one_capability_per_field(self) -> None:
        m = t.map_thing(td(properties={"pos": prop(type="object", properties={
            "x": {"type": "integer"}, "y": {"type": "integer"}})}))
        self.assertEqual([c["name"] for c in m.descriptor["capabilities"]], ["pos.x", "pos.y"])
        self.assertEqual(m.bindings["pos.y"]["field"], "y")

    def test_a_map_without_listed_keys_needs_a_sample_and_says_so(self) -> None:
        p = prop(type="object", additionalProperties={"type": "integer"})
        none = t.map_thing(td(properties={"pos": p}))
        self.assertEqual(none.descriptor["capabilities"], [])
        some = t.map_thing(td(properties={"pos": p}), samples={"pos": {"x": 0, "z": 1}})
        self.assertEqual([c["name"] for c in some.descriptor["capabilities"]], ["pos.x", "pos.z"])
        self.assertTrue(any(f.category == "keys_from_sample" for f in some.findings))

    def test_names_follow_the_openmhs_pattern_and_never_collide(self) -> None:
        m = t.map_thing(td(properties={"ExposureTime": prop(type="integer"), "exposure-time": prop(type="integer"),
                                       "9lives": prop(type="integer")}))
        names = [c["name"] for c in m.descriptor["capabilities"]]
        self.assertEqual(len(set(names)), 3)
        self.assertTrue(all(NAME.match(n) for n in names), names)


class Actions(unittest.TestCase):
    def test_an_action_is_not_exposed_just_because_it_is_listed(self) -> None:
        m = t.map_thing(td(actions={"reboot": act(), "get_position": act()}))
        self.assertEqual(m.descriptor["capabilities"], [])
        self.assertEqual({f.category for f in m.findings}, {"action_unreviewed"})

    def test_a_name_that_sounds_harmless_is_not_trusted(self) -> None:
        m = t.map_thing(td(actions={"get_xyz_position": act(), "read_sensor": act()}))
        self.assertEqual(m.descriptor["capabilities"], [])

    def test_only_a_declared_safe_action_is_exposed_without_review(self) -> None:
        m = t.map_thing(td(actions={"snapshot": act(safe=True, input={"type": "object", "properties": {}})}))
        (c,) = m.descriptor["capabilities"]
        # OpenMHS reserves side_effect "none" for read-only capabilities; an invoke is at least
        # "reversible", and the TD's safe: true claim is kept as provenance.
        self.assertEqual((c["side_effect"], c["extensions"]["x-wot"]["td_declared_safe"]), ("reversible", True))
        self.assertTrue(any(f.category == "mapped_td_safe" for f in m.findings))

    def test_a_reviewed_override_supplies_what_the_description_omits(self) -> None:
        a = act(input={"type": "object", "properties": {"mode": {"type": "string", "enum": ["a", "b"]}}})
        ov = {"actions": {"go": {"expose": True, "side_effect": "reversible", "idempotent": False,
                                 "timing": {"timeout_ms": 5000}}}}
        m = t.map_thing(td(actions={"go": a}), overrides=ov)
        (c,) = m.descriptor["capabilities"]
        self.assertEqual((c["side_effect"], c["idempotent"], c["timing"]["timeout_ms"]), ("reversible", False, 5000))
        self.assertEqual(c["params"]["mode"]["values"], ["a", "b"])
        self.assertTrue(c["extensions"]["x-wot"]["reviewed"])

    def test_an_incomplete_override_does_not_expose_the_action(self) -> None:
        a = act()
        for ov in ({"side_effect": "reversible"},                      # no timing, no idempotent
                   {"side_effect": "harmless", "idempotent": True, "timing": {"timeout_ms": 1}},
                   {"expose": False, "side_effect": "none", "idempotent": True, "timing": {"timeout_ms": 1}}):
            m = t.map_thing(td(actions={"go": a}), overrides={"actions": {"go": ov}})
            self.assertEqual(m.descriptor["capabilities"], [], ov)

    def test_parameters_with_defaults_are_left_to_the_device(self) -> None:
        a = act(input={"type": "object", "properties": {"level": {"type": "integer", "minimum": 0, "maximum": 9},
                                                         "quick": {"type": "boolean", "default": False}}})
        ov = {"actions": {"go": {"expose": True, "side_effect": "none", "idempotent": True,
                                 "timing": {"timeout_ms": 1000}}}}
        m = t.map_thing(td(actions={"go": a}), overrides=ov)
        self.assertEqual(list(m.descriptor["capabilities"][0]["params"]), ["level"])
        self.assertTrue(any(f.category == "optional_param_omitted" for f in m.findings))

    def test_undeclared_keyword_arguments_are_reported(self) -> None:
        a = act(input={"type": "object", "properties": {}, "additionalProperties": True})
        m = t.map_thing(td(actions={"move": a}))
        self.assertTrue(any(f.category == "undeclared_params" for f in m.findings))

    def test_variants_expose_one_action_as_several_narrow_capabilities(self) -> None:
        a = act(input={"type": "object", "properties": {}, "additionalProperties": True})
        ov = {"actions": {"move": {"expose": True, "side_effect": "reversible", "idempotent": True,
                                   "timing": {"timeout_ms": 9000},
                                   "variants": {"move_x": {"params": {"x": {"type": "integer", "range": {"min": -9, "max": 9}}}},
                                                "move_y": {"params": {"y": {"type": "integer", "range": {"min": -9, "max": 9}}}}}}}}
        m = t.map_thing(td(actions={"move": a}), overrides=ov)
        self.assertEqual([c["name"] for c in m.descriptor["capabilities"]], ["move_x", "move_y"])
        self.assertEqual(m.bindings["move_y"]["wire"], {"y": "y"})


class Limits(unittest.TestCase):
    OV = {"actions": {"go": {"expose": True, "side_effect": "reversible", "idempotent": True,
                             "timing": {"timeout_ms": 1000}}}}

    def test_a_parameter_with_no_declared_limit_blocks_exposure_until_a_reviewer_decides(self) -> None:
        a = act(input={"type": "object", "properties": {"level": {"type": "number"}}})
        m = t.map_thing(td(actions={"go": a}), overrides=self.OV)
        self.assertEqual(m.descriptor["capabilities"], [])
        self.assertTrue(any(f.category == "unbounded_param" for f in m.findings))

    def test_a_limit_from_the_description_or_the_reviewer_lets_it_through(self) -> None:
        bounded = act(input={"type": "object", "properties": {"level": {"type": "number", "minimum": 0, "maximum": 5}}})
        self.assertEqual(len(t.map_thing(td(actions={"go": bounded}), overrides=self.OV).descriptor["capabilities"]), 1)
        unbounded = act(input={"type": "object", "properties": {"level": {"type": "number"}}})
        ov = {"actions": {"go": {**self.OV["actions"]["go"], "unbounded_ok": ["level"]}}}
        self.assertEqual(len(t.map_thing(td(actions={"go": unbounded}), overrides=ov).descriptor["capabilities"]), 1)


class SecurityAndEvents(unittest.TestCase):
    def test_a_thing_that_requires_authentication_says_so_instead_of_importing_silently(self) -> None:
        m = t.map_thing({**td(), "securityDefinitions": {"b": {"scheme": "basic", "in": "header"}},
                         "security": ["b"]})
        self.assertEqual(m.auth, {"scheme": "basic"})
        self.assertTrue(m.connectable)
        self.assertTrue(any(f.category == "credentials_required" for f in m.findings))

    def test_nosec_needs_nothing(self) -> None:
        m = t.map_thing({**td(), "securityDefinitions": {"n": {"scheme": "nosec"}}, "security": "n"})
        self.assertIsNone(m.auth)

    def test_schemes_this_adapter_cannot_meet_block_the_import(self) -> None:
        for definition in ({"scheme": "oauth2", "flow": "client"}, {"scheme": "digest"},
                           {"scheme": "apikey", "in": "query", "name": "k"},
                           {"scheme": "basic", "in": "query"}):
            m = t.map_thing({**td(), "securityDefinitions": {"x": definition}, "security": ["x"]})
            self.assertFalse(m.connectable, definition)
            self.assertTrue(any(f.category == "security_scheme_unsupported" for f in m.findings))

    def test_events_are_reported_not_dropped(self) -> None:
        m = t.map_thing({**td(), "events": {"overheat": {"forms": [{"href": "/e", "op": "subscribeevent"}]}}})
        self.assertEqual([(f.affordance, f.status) for f in m.findings], [("events/overheat", t.UNSUPPORTED)])


@unittest.skipUnless(FIXTURE.exists(), "needs research/fixtures")
class OpenFlexureSimulator(unittest.TestCase):
    """Coverage on a real LabThings server's descriptions, pinned so changes are visible."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.tds = json.loads(FIXTURE.read_text(encoding="utf-8"))
        cls.maps = {n: t.map_thing(d, base_url="http://127.0.0.1:5000/", device_id=f"ofm.{n}")
                    for n, d in cls.tds.items()}

    def test_the_fixture_is_the_simulators_fourteen_things(self) -> None:
        self.assertEqual(len(self.tds), 14)
        self.assertEqual(sum(len(d["properties"]) for d in self.tds.values()), 95)
        self.assertEqual(sum(len(d["actions"]) for d in self.tds.values()), 50)

    def test_nothing_in_the_descriptions_declares_units_or_action_safety(self) -> None:
        props = [p for d in self.tds.values() for p in d["properties"].values()]
        acts = [a for d in self.tds.values() for a in d["actions"].values()]
        self.assertFalse(any("unit" in p for p in props))
        self.assertFalse(any("safe" in a or "idempotent" in a for a in acts))

    def test_all_fifty_actions_need_review_and_none_is_exposed(self) -> None:
        caps = [c for m in self.maps.values() for c in m.descriptor["capabilities"]]
        self.assertEqual([c for c in caps if c["kind"] == "action"], [])
        review = [f for m in self.maps.values() for f in m.findings if f.category == "action_unreviewed"]
        self.assertEqual(len(review), 50)

    def test_property_coverage_is_pinned(self) -> None:
        mapped = sum(1 for m in self.maps.values() for f in m.findings
                     if f.affordance.startswith("properties/") and f.status == t.MAPPED)
        unsupported = sum(1 for m in self.maps.values() for f in m.findings
                          if f.affordance.startswith("properties/") and f.status == t.UNSUPPORTED)
        self.assertEqual((mapped, unsupported), (71, 27))

    def test_every_generated_name_is_valid_and_unique_per_device(self) -> None:
        for n, m in self.maps.items():
            names = [c["name"] for c in m.descriptor["capabilities"]]
            self.assertEqual(len(names), len(set(names)), n)
            self.assertTrue(all(NAME.match(x) for x in names), n)


if __name__ == "__main__":
    unittest.main()
