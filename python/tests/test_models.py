"""The typed models are checked against the JSON Schemas, not just written beside them."""

from __future__ import annotations

import dataclasses
import json
import unittest

from support import REPO

from openmhs import (
    AdapterResult,
    CommandState,
    Decision,
    Descriptor,
    ErrorObject,
    Outcome,
    PolicyRecord,
    Quantity,
    Receipt,
    RuleMatch,
    StateValue,
    Transition,
    Verification,
)

try:
    import jsonschema
    import referencing
except ImportError:  # optional: the client itself needs only the standard library
    jsonschema = None

SCHEMAS = REPO / "spec" / "schemas"
EXAMPLES = REPO / "spec" / "examples"


def load(name: str) -> dict:
    return json.loads((SCHEMAS / name).read_text(encoding="utf-8"))


RECEIPT = load("receipt.schema.json")
COMMON = load("common.schema.json")
DESCRIPTOR = load("descriptor.schema.json")
POLICY = RECEIPT["properties"]["policy"]

#: Each model and the schema object it mirrors.
MIRRORS = [
    (Receipt, RECEIPT),
    (Transition, RECEIPT["properties"]["transitions"]["items"]),
    (PolicyRecord, POLICY),
    (RuleMatch, POLICY["properties"]["rules"]["items"]),
    (AdapterResult, RECEIPT["properties"]["adapter_result"]),
    (Verification, RECEIPT["properties"]["verification"]),
    (ErrorObject, COMMON["$defs"]["error"]),
    (StateValue, COMMON["$defs"]["state_value"]),
    (Descriptor, DESCRIPTOR),
]


def fields(cls: type) -> dict[str, dataclasses.Field]:
    return {f.name: f for f in dataclasses.fields(cls) if f.name != "extra"}


def required(f: dataclasses.Field) -> bool:
    return f.default is dataclasses.MISSING and f.default_factory is dataclasses.MISSING


class SchemaParity(unittest.TestCase):
    def test_fields_match_schema_properties(self) -> None:
        for cls, schema in MIRRORS:
            with self.subTest(model=cls.__name__):
                self.assertEqual(set(fields(cls)), set(schema["properties"]))

    def test_required_fields_match_schema_required(self) -> None:
        for cls, schema in MIRRORS:
            with self.subTest(model=cls.__name__):
                req = {n for n, f in fields(cls).items() if required(f)}
                self.assertEqual(req, set(schema.get("required", [])))

    def test_enums_match_schema_enums(self) -> None:
        self.assertEqual({s.value for s in CommandState}, set(COMMON["$defs"]["command_state"]["enum"]))
        self.assertEqual({o.value for o in Outcome}, set(COMMON["$defs"]["outcome"]["enum"]))
        self.assertEqual({d.value for d in Decision}, set(POLICY["properties"]["decision"]["enum"]))


class RoundTrip(unittest.TestCase):
    def test_example_receipts_round_trip_unchanged(self) -> None:
        paths = sorted((EXAMPLES / "receipts").glob("*.json"))
        self.assertTrue(paths)
        for p in paths:
            with self.subTest(example=p.name):
                doc = json.loads(p.read_text(encoding="utf-8"))
                r = Receipt.from_dict(doc)
                self.assertEqual(r.extra, {})
                self.assertEqual(r.to_dict(), doc)

    def test_example_descriptors_round_trip_unchanged(self) -> None:
        for p in sorted((EXAMPLES / "valid").glob("*.json")):
            doc = json.loads(p.read_text(encoding="utf-8"))
            if "capabilities" not in doc:
                continue
            with self.subTest(example=p.name):
                self.assertEqual(Descriptor.from_dict(doc).to_dict(), doc)

    def test_typed_access(self) -> None:
        doc = json.loads((EXAMPLES / "receipts" / "verified-write.json").read_text(encoding="utf-8"))
        r = Receipt.from_dict(doc)
        self.assertIs(r.state, CommandState.VERIFIED)
        self.assertTrue(r.confirmed)
        self.assertEqual(r.verification.observed.value, Quantity(5.0, "V"))
        denied = Receipt.from_dict(
            json.loads((EXAMPLES / "receipts" / "denied-out-of-range.json").read_text(encoding="utf-8"))
        )
        self.assertTrue(denied.denied)
        self.assertEqual(denied.reasons[0].code, "command.value.out_of_range")

    def test_unknown_fields_are_kept_not_dropped(self) -> None:
        doc = json.loads((EXAMPLES / "receipts" / "verified-write.json").read_text(encoding="utf-8"))
        doc["from_the_future"] = {"x": 1}
        doc["state"] = "SOME_NEW_STATE"
        r = Receipt.from_dict(doc)
        self.assertEqual(r.extra, {"from_the_future": {"x": 1}})
        self.assertEqual(r.state, "SOME_NEW_STATE")
        self.assertEqual(r.to_dict(), doc)


@unittest.skipIf(jsonschema is None, "jsonschema not installed (pip install 'jsonschema>=4.18')")
class SchemaValidation(unittest.TestCase):
    def test_serialized_models_validate(self) -> None:
        validator = receipt_validator()
        for p in sorted((EXAMPLES / "receipts").glob("*.json")):
            with self.subTest(example=p.name):
                r = Receipt.from_dict(json.loads(p.read_text(encoding="utf-8")))
                validator.validate(r.to_dict())


def receipt_validator():
    """A validator for receipts with every spec schema registered by its $id."""
    resources = []
    for p in SCHEMAS.glob("*.schema.json"):
        s = json.loads(p.read_text(encoding="utf-8"))
        resources.append((s["$id"], referencing.Resource.from_contents(s)))
    registry = referencing.Registry().with_resources(resources)
    return jsonschema.Draft202012Validator(RECEIPT, registry=registry)


if __name__ == "__main__":
    unittest.main()
