"""Cross-check spec/schemas with an independent JSON Schema implementation (Python `jsonschema`).

The Rust crate is the reference implementation; this script makes sure the schemas
themselves are portable and behave the same in another language:

- every example under spec/examples/{valid,commands/valid,receipts,events,conformance,manifests}
  passes, and so does every adapters/python/*/manifest*.json,
- every command under spec/examples/commands/invalid fails,
- invalid descriptors expected to fail with `descriptor.schema` fail structurally,
  and all other invalid descriptors pass structurally (their errors are semantic).

Usage: python tools/check_schemas.py   (requires: pip install jsonschema)
"""
import json
import pathlib
import sys

from jsonschema import Draft202012Validator, FormatChecker
from referencing import Registry, Resource

ROOT = pathlib.Path(__file__).resolve().parent.parent
SPEC = ROOT / "spec"


def load(path):
    return json.loads(path.read_text(encoding="utf-8"))


schemas = {p.stem.removesuffix(".schema"): load(p) for p in (SPEC / "schemas").glob("*.schema.json")}
registry = Registry().with_resources(
    (s["$id"], Resource.from_contents(s)) for s in schemas.values()
)


def validator(name):
    return Draft202012Validator(schemas[name], registry=registry, format_checker=FormatChecker())


failures = []
checked = 0


def expect(name, path, should_pass):
    global checked
    checked += 1
    errors = list(validator(name).iter_errors(load(path)))
    if should_pass and errors:
        failures.append(f"{path.relative_to(ROOT)} should pass {name}: {errors[0].message}")
    if not should_pass and not errors:
        failures.append(f"{path.relative_to(ROOT)} should fail {name}")


ex = SPEC / "examples"
for p in sorted((ex / "valid").glob("*.json")):
    expect("descriptor", p, True)
for p in sorted((ex / "commands" / "valid").glob("*.json")):
    expect("command", p, True)
for p in sorted((ex / "commands" / "invalid").glob("*.json")):
    expect("command", p, False)
for p in sorted((ex / "receipts").glob("*.json")):
    expect("receipt", p, True)
for p in sorted((ex / "events").glob("*.json")):
    expect("event", p, True)
for p in sorted((ex / "conformance").glob("*.json")):
    expect("conformance-report", p, True)
for p in sorted((ex / "manifests").glob("*.json")) + sorted(ROOT.glob("adapters/python/*/manifest*.json")):
    expect("adapter-manifest", p, True)

expected = load(ex / "invalid" / "expected.json")
for name, exp in sorted(expected.items()):
    expect("descriptor", ex / "invalid" / name, exp["code"] != "descriptor.schema")

if failures:
    print("\n".join(failures))
    sys.exit(1)
print(f"ok: {checked} documents checked against {len(schemas)} schemas")
