# WoT units through LabThings and OpenMHS

Status: implementation investigation for discussion only. Nothing has been proposed upstream
or sent to a maintainer.

## What the current sources say

Checked on 2026-10-30 against the default branch of
[LabThings-FastAPI](https://github.com/labthings/labthings-fastapi):

- Its Thing Description [`DataSchema`](https://github.com/labthings/labthings-fastapi/blob/main/src/labthings_fastapi/thing_description/_model.py)
  already has an optional `unit: str` field. Its action-affordance model also has optional
  `safe` and `idempotent` fields. These are not missing from the WoT model and should not be
  proposed as new vocabulary fields.
- The [`jsonschema_to_dataschema` / `type_to_dataschema` path](https://github.com/labthings/labthings-fastapi/blob/main/src/labthings_fastapi/thing_description/__init__.py)
  converts Pydantic-generated schemas and retains schema keys it does not specially rewrite.
  The [`DataSchema` model](https://github.com/labthings/labthings-fastapi/blob/main/src/labthings_fastapi/thing_description/_model.py)
  can serialize the standard WoT `unit` member.
- The documented [`lt.property` API](https://github.com/labthings/labthings-fastapi/blob/main/docs/source/properties.rst)
  supports Pydantic constraints and metadata, but does not document a first-class `unit=`
  argument. The runtime probe below confirms that Pydantic's
  `Field(json_schema_extra={"unit": "mm"})` reaches generated TD schemas for data properties,
  functional properties and action inputs in LabThings-FastAPI 0.3.0.
- W3C Thing Description 1.1 defines `unit` on `DataSchema` as an optional string and recommends
  semantic definitions for unambiguous units ([W3C TD 1.1 §5.3.2.1](https://www.w3.org/TR/wot-thing-description11/#dataschema)).

The upstream model and property-schema path already represent units. The open question is
whether this Pydantic metadata convention is discoverable enough for Thing authors, or whether
the project would prefer a more explicit helper.

## Verified backwards-compatible authoring path

The in-memory probe in [`experiments/labthings_units_probe.py`](experiments/labthings_units_probe.py)
uses the `Annotated`/Pydantic metadata pattern and calls LabThings' `thing_description_dict()`;
it starts no server and accesses no hardware:

```python
from typing import Annotated
from pydantic import Field
import labthings_fastapi as lt

class Stage(lt.Thing):
    position_x: Annotated[
        float,
        Field(json_schema_extra={"unit": "mm"}),
    ] = lt.property(default=0.0, readonly=True)
```

The runtime result on 2026-10-30 (LabThings-FastAPI 0.3.0) was:

| Generated schema | Unit |
|---|---|
| data property `position_x` | `mm` |
| functional property `focus_position` | `mm` |
| action input `move_x.input.properties.x` | `mm` |

The same generated description was passed through OpenMHS' Thing Description importer. It
mapped `position_x` to `{ "type": "number", "quantity": "length", "unit": "mm" }`.
No LabThings API addition is needed to carry the metadata. If maintainers want to improve
discoverability, documentation for this existing pattern is the smallest compatible change.
The captured probe output is in
[`results/reports/labthings_units_probe.json`](results/reports/labthings_units_probe.json).

This validates in-memory Thing Description generation, not an HTTP endpoint response. Avoid an
implicit unit registry, unit inference from names/docstrings, or value conversion in LabThings.
A device description should report the author's chosen unit string; a consumer can then accept
known codes, preserve unknown metadata for review, and refuse unsafe conversions.

## OpenMHS handling

OpenMHS already maps known TD unit codes to its quantity/unit contract. Known compatible
aliases are normalized, and unknown unit strings are reported for review rather than guessed.
The OpenFlexure simulator TDs currently declare no units for the stage coordinates. The demo
therefore treats them as integer simulator coordinates, never as millimetres. A request that
adds `"unit": "mm"` is rejected by the type contract.

The W3C `unit` value is a free string. OpenMHS uses a constrained UCUM table, so an upstream
TD may validly use a unit string OpenMHS cannot safely convert. That should remain a visible
review finding; the importer must not silently reinterpret it.

## Maintainer discussion

The concise technical brief is [COLLABORATION_BRIEF.md](COLLABORATION_BRIEF.md). It asks
whether documenting the existing unit annotation path would help LabThings users and what
approach would fit the project. No issue, pull request, upstream change, or message is created
by this document.
