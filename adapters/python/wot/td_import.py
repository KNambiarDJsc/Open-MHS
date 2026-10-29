"""W3C Thing Description -> OpenMHS capability contract (experimental).

``map_thing`` turns one Thing Description (WoT TD 1.0/1.1, HTTP binding) into

  * a draft OpenMHS device descriptor (spec/descriptor.md),
  * the runtime *bindings* the adapter needs to call it (hrefs, parameter names),
  * a *findings* list that says, for every affordance, whether it was mapped, needs human
    review, or is unsupported, and exactly why.

A Thing Description is a description of an interface, not a statement of what is safe. So
this module never exposes an affordance because it is listed:

  * Properties become **read-only** capabilities. Writing a property changes the world, so a
    writable property stays read-only until a reviewed override says otherwise.
  * An action is exposed only if the Thing Description itself declares ``safe: true``, or a
    reviewed override supplies what a TD usually omits: the physical effect (``side_effect``),
    whether repeating it is harmless (``idempotent``), a time bound, and the limits of its
    parameters. Everything else is reported as *needs review* and not exposed.
  * Anything that cannot be represented faithfully (compound values, nullable values,
    undeclared parameters) is reported as *unsupported* with the reason, never approximated.

Granting access is separate and unchanged: OpenMHS policy is deny-by-default, so an exposed
capability can still do nothing until an operator grants it.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Any, Optional
from urllib.parse import urljoin

#: Quantity kind for each unit OpenMHS accepts (spec/units.md). Only exact matches and the
#: aliases below are mapped; any other unit string is reported, not guessed.
_UNITS = {
    "dimensionless": ["1", "%"],
    "length": ["m", "km", "cm", "mm", "um"],
    "mass": ["kg", "g", "mg"],
    "time": ["s", "ms", "us", "min", "h"],
    "frequency": ["Hz", "kHz", "MHz"],
    "temperature": ["K", "Cel", "[degF]"],
    "electric_potential": ["V", "mV", "kV"],
    "electric_current": ["A", "mA", "uA"],
    "electric_resistance": ["Ohm", "kOhm", "MOhm"],
    "power": ["W", "mW", "kW"],
    "energy": ["J", "kJ", "W.h", "kW.h"],
    "pressure": ["Pa", "kPa", "bar", "mbar"],
    "angle": ["rad", "deg"],
    "velocity": ["m/s", "mm/s"],
    "angular_velocity": ["rad/s", "deg/s"],
    "acceleration": ["m/s2", "mm/s2"],
    "force": ["N", "kN"],
    "torque": ["N.m"],
    "volume": ["m3", "L", "mL", "uL"],
    "volumetric_flow": ["m3/s", "L/min", "mL/min", "uL/min"],
    "illuminance": ["lx"],
}
_QUANTITY_OF = {u: q for q, us in _UNITS.items() for u in us}
_ALIASES = {"degC": "Cel", "°C": "Cel", "µm": "um", "ohm": "Ohm", "Ω": "Ohm"}

MAPPED, REVIEW, UNSUPPORTED = "mapped", "review", "unsupported"
SIDE_EFFECTS = ("none", "reversible", "irreversible", "hazardous")


@dataclass
class Finding:
    """What happened to one affordance."""

    affordance: str  # "properties/moving", "actions/move_absolute"
    status: str  # MAPPED, REVIEW or UNSUPPORTED
    category: str  # a short stable reason code, for counting
    detail: str
    capability: Optional[str] = None


@dataclass
class Mapping:
    descriptor: dict
    bindings: dict
    findings: list[Finding] = field(default_factory=list)
    #: How the adapter authenticates to the Thing: ``None`` (the Thing declares none), or
    #: ``{"scheme": "basic" | "bearer"}`` (credentials come from the gateway's environment),
    #: or ``{"scheme": "unsupported", ...}`` when the Thing needs something this adapter cannot do.
    auth: Optional[dict] = None

    @property
    def connectable(self) -> bool:
        return not (self.auth and self.auth.get("scheme") == "unsupported")

    def counts(self) -> dict:
        out: dict = {}
        for f in self.findings:
            kind = f.affordance.split("/", 1)[0]
            out.setdefault(kind, {MAPPED: 0, REVIEW: 0, UNSUPPORTED: 0})[f.status] += 1
        return out

    def by_category(self) -> dict:
        out: dict = {}
        for f in self.findings:
            out[f.category] = out.get(f.category, 0) + 1
        return dict(sorted(out.items(), key=lambda kv: -kv[1]))


def slug(text: str) -> str:
    """A lowercase OpenMHS name segment: ``^[a-z][a-z0-9_]*$``."""
    s = re.sub(r"(?<=[a-z0-9])(?=[A-Z])", "_", str(text))
    s = re.sub(r"[^A-Za-z0-9]+", "_", s).strip("_").lower()
    if not s or not s[0].isalpha():
        s = "x" + s
    return s


def _ops(forms: list, default: list[str]) -> list[str]:
    ops: list[str] = []
    for f in forms or []:
        o = f.get("op", default)
        ops.extend([o] if isinstance(o, str) else o)
    return ops or default


def _form(forms: list, op: str) -> Optional[dict]:
    for f in forms or []:
        o = f.get("op")
        o = [o] if isinstance(o, str) else (o or [])
        if op in o:
            return f
    return None


def _non_null(schema: dict) -> tuple[dict, bool]:
    """(schema without a null alternative, whether null was allowed)."""
    for key in ("oneOf", "anyOf"):
        alts = schema.get(key)
        if alts:
            rest = [a for a in alts if a.get("type") != "null"]
            if len(rest) == 1 and len(rest) != len(alts):
                merged = {**{k: v for k, v in schema.items() if k != key}, **rest[0]}
                return merged, True
    return schema, False


def value_spec(schema: dict) -> tuple[Optional[dict], list[tuple[str, str]]]:
    """An OpenMHS ValueSpec for a JSON-schema fragment, plus notes ``(category, detail)``.

    ``None`` means it cannot be represented faithfully; the note says why.
    """
    notes: list[tuple[str, str]] = []
    base, nullable = _non_null(schema)
    if nullable:
        return None, [("nullable", "the value may be null, which an OpenMHS value cannot be")]
    if "oneOf" in base or "anyOf" in base or "allOf" in base:
        return None, [("compound", "a choice between different schemas")]
    t = base.get("type")
    if t == "boolean":
        return {"type": "boolean"}, notes
    if t in ("integer", "number"):
        spec: dict[str, Any] = {"type": "number" if t == "number" else "integer"}
        unit = base.get("unit")
        unit = _ALIASES.get(unit, unit) if unit else unit
        if unit and unit in _QUANTITY_OF:
            spec["quantity"], spec["unit"] = _QUANTITY_OF[unit], unit
        else:
            if unit:
                notes.append(("unit_unknown", f"unit '{unit}' is not in the OpenMHS unit table"))
            else:
                notes.append(("unit_missing", "the Thing Description declares no unit"))
            if t == "number":
                spec["quantity"], spec["unit"] = "dimensionless", "1"
        lo, hi = base.get("minimum"), base.get("maximum")
        if lo is not None or hi is not None:
            spec["range"] = {k: v for k, v in (("min", lo), ("max", hi)) if v is not None}
        return spec, notes
    if t == "string":
        if "enum" in base and all(isinstance(v, str) for v in base["enum"]):
            return {"type": "enum", "values": list(base["enum"])}, notes
        return {"type": "string", "max_length": int(base.get("maxLength", 256))}, notes
    if t == "object":
        return None, [("compound", "an object value")]
    if t == "array":
        return None, [("compound", "an array value")]
    if "enum" in base:
        return None, [("compound", "an enum of non-string values")]
    return None, [("untyped", "no JSON-schema type is declared")]


def _flatten(schema: dict) -> Optional[dict[str, dict]]:
    """Fields of an object schema whose fields are all scalar; ``None`` otherwise."""
    props = schema.get("properties")
    if schema.get("type") != "object" or not props:
        return None
    fields = {k: v for k, v in props.items()}
    if all(value_spec(v)[0] is not None for v in fields.values()):
        return fields
    return None


def _security(td: dict, findings: list) -> Optional[dict]:
    """The Thing's security requirement, as far as this adapter can meet it.

    Supported: none (``nosec``), ``basic`` and ``bearer`` in a header. Anything else
    (OAuth2, digest, API keys in a query string, combinations) is reported and blocks the
    import: a contract that can only ever be refused is worse than none."""
    names = td.get("security") or []
    names = [names] if isinstance(names, str) else list(names)
    defs = td.get("securityDefinitions") or {}
    schemes = [(n, defs.get(n) or {}) for n in names]
    if all(d.get("scheme") in (None, "nosec") for _, d in schemes):
        return None
    if (len(schemes) == 1 and schemes[0][1].get("scheme") in ("basic", "bearer")
            and schemes[0][1].get("in", "header") == "header"):
        scheme = schemes[0][1]["scheme"]
        envs = "WOT_USER and WOT_PASSWORD" if scheme == "basic" else "WOT_TOKEN"
        findings.append(Finding(
            "security", REVIEW, "credentials_required",
            f"the Thing requires {scheme} authentication; the gateway's environment must provide "
            f"{envs}. They are passed to the adapter by name and never written to the manifest"))
        return {"scheme": scheme}
    findings.append(Finding(
        "security", UNSUPPORTED, "security_scheme_unsupported",
        "the Thing requires " + ", ".join(f"{n} ({d.get('scheme')})" for n, d in schemes) +
        ", which this adapter cannot do; no manifest is written"))
    return {"scheme": "unsupported", "schemes": [d.get("scheme") for _, d in schemes]}


def map_thing(
    td: dict,
    *,
    base_url: Optional[str] = None,
    device_id: Optional[str] = None,
    overrides: Optional[dict] = None,
    adapter_id: str = "openmhs.py.wot",
    samples: Optional[dict] = None,
) -> Mapping:
    """Map one Thing Description. ``overrides`` is the reviewed file (see ``review_template``)."""
    overrides = overrides or {}
    base = urljoin(base_url or td.get("base") or "", "") if (base_url or td.get("base")) else ""
    title = td.get("title") or td.get("id") or "thing"
    ident = device_id or "wot." + slug(td.get("id") or title)
    caps: list[dict] = []
    bindings: dict[str, dict] = {}
    findings: list[Finding] = []
    taken: set[str] = set()

    def name_for(raw: str, *suffix: str) -> str:
        n = ".".join([slug(raw), *map(slug, suffix)])
        base_n, i = n, 2
        while n in taken:
            n, i = f"{base_n}_{i}", i + 1
        taken.add(n)
        return n

    def href(form: dict) -> str:
        return urljoin(base + ("" if base.endswith("/") or not base else "/"), form["href"])

    # ------------------------------------------------------------------ properties
    for pname, prop in (td.get("properties") or {}).items():
        aff = f"properties/{pname}"
        form = _form(prop.get("forms"), "readproperty") or (
            None if prop.get("writeOnly") else (prop.get("forms") or [None])[0]
        )
        if form is None or prop.get("writeOnly"):
            findings.append(Finding(aff, UNSUPPORTED, "not_readable", "no readproperty form"))
            continue
        fields = _flatten(_non_null(prop)[0]) if prop.get("type") == "object" else None
        sample = (samples or {}).get(pname)
        extra = prop.get("additionalProperties")
        if (fields is None and prop.get("type") == "object" and isinstance(extra, dict)
                and isinstance(sample, dict) and sample
                and value_spec(extra)[0] is not None):
            # A map keyed by names the description does not list (LabThings: axis names).
            fields = {k: extra for k in sample if isinstance(k, str)}
            findings.append(Finding(aff, REVIEW, "keys_from_sample",
                                    "the description declares a map without its keys; they were taken "
                                    f"from one live read ({', '.join(fields)}) and may change"))
        targets = (
            [(name_for(pname, fk), fv, fk) for fk, fv in fields.items()]
            if fields
            else [(name_for(pname), prop, None)]
        )
        for cname, schema, fieldkey in targets:
            spec, notes = value_spec(schema)
            if spec is None:
                taken.discard(cname)
                cat, why = notes[0]
                findings.append(Finding(aff, UNSUPPORTED, cat, why))
                continue
            writable = (
                not prop.get("readOnly") and _form(prop.get("forms"), "writeproperty") is not None
            )
            cap = {
                "name": cname,
                "kind": "property",
                "operations": ["read"],
                "value": spec,
                "side_effect": "none",
                "extensions": {"x-wot": {"affordance": aff, **({"field": fieldkey} if fieldkey else {})}},
            }
            desc = schema.get("description") or prop.get("description")
            if desc:
                cap["description"] = str(desc).strip()[:300]
            caps.append(cap)
            bindings[cname] = {"kind": "property", "href": href(form), "field": fieldkey}
            findings.append(Finding(aff, MAPPED, "mapped", "read-only", cname))
            for cat, why in notes:
                findings.append(Finding(aff, REVIEW, cat, why, cname))
            findings.append(
                Finding(aff, REVIEW, "no_measurement_time",
                        "a read returns a bare value; the adapter can stamp only when it received it", cname)
            )
            if writable:
                findings.append(
                    Finding(aff, REVIEW, "writable_not_exposed",
                            "the property is writable; writing changes the world, so it stays read-only "
                            "until a reviewed override exposes it", cname)
                )

    # ------------------------------------------------------------------ actions
    for aname, act in (td.get("actions") or {}).items():
        aff = f"actions/{aname}"
        form = _form(act.get("forms"), "invokeaction") or (act.get("forms") or [None])[0]
        if form is None:
            findings.append(Finding(aff, UNSUPPORTED, "no_form", "no invokeaction form"))
            continue
        ov = (overrides.get("actions") or {}).get(aname)
        if ov is not None and not ov.get("expose", True):
            ov = None
        td_safe = act.get("safe") is True
        hidden = (act.get("input") or {}).get("additionalProperties") is True
        if hidden and not (ov or {}).get("params") and not any(
                (v or {}).get("params") for v in ((ov or {}).get("variants") or {}).values()):
            findings.append(Finding(aff, REVIEW, "undeclared_params",
                                    "the action accepts extra keyword arguments the Thing Description "
                                    "does not list; a reviewer must declare them to make it callable"))
        if not ov and not td_safe:
            missing = ["side_effect", "idempotent", "timing"]
            if act.get("idempotent") is not None:
                missing.remove("idempotent")
            findings.append(Finding(
                aff, REVIEW, "action_unreviewed",
                "not exposed: the Thing Description does not say what this action does to the "
                "world; a reviewer must supply " + ", ".join(missing) + " and parameter limits"))
            continue

        variants = (ov or {}).get("variants") or {None: {}}
        for vname, vov in variants.items():
            cur = {**(ov or {}), **vov}
            side_effect = cur.get("side_effect", "reversible" if td_safe else None)
            if side_effect not in SIDE_EFFECTS:
                findings.append(Finding(aff, REVIEW, "action_unreviewed",
                                        f"override has no valid side_effect (one of {SIDE_EFFECTS})"))
                continue
            timing = cur.get("timing") or ({"timeout_ms": 30000} if td_safe else None)
            if not timing or "timeout_ms" not in timing:
                findings.append(Finding(aff, REVIEW, "action_unreviewed", "override has no timing.timeout_ms"))
                continue
            idem = cur.get("idempotent", act.get("idempotent", True if td_safe else None))
            if idem is None:
                findings.append(Finding(aff, REVIEW, "action_unreviewed", "override does not say idempotent"))
                continue

            schema = act.get("input") or {}
            params: dict[str, dict] = {}
            wire: dict[str, str] = {}
            ok = True
            declared = cur.get("params") or {}
            for pk, ps in (schema.get("properties") or {}).items():
                if pk in declared:
                    continue
                if pk in cur.get("omit_params", []):
                    findings.append(Finding(aff, REVIEW, "optional_param_omitted",
                                            f"'{pk}' is left to the device's default (reviewer decision)", f"{slug(aname)}"))
                    continue
                if "default" in ps:
                    findings.append(Finding(aff, REVIEW, "optional_param_omitted",
                                            f"'{pk}' has a default and is left to the device's default", f"{slug(aname)}"))
                    continue
                spec, notes = value_spec(ps)
                if spec is None:
                    findings.append(Finding(aff, UNSUPPORTED, notes[0][0],
                                            f"parameter '{pk}': {notes[0][1]}"))
                    ok = False
                    continue
                params[slug(pk)], wire[slug(pk)] = spec, pk
                for cat, why in notes:
                    findings.append(Finding(aff, REVIEW, cat, f"parameter '{pk}': {why}", slug(aname)))
            for pk, ps in declared.items():
                params[slug(pk)], wire[slug(pk)] = ps, pk
            # A numeric parameter with no limit is a limit nobody chose. Expose it only if a
            # reviewer says, by name, that it is meant to be unbounded.
            unbounded = [k for k, ps in params.items()
                         if ps.get("type") in ("number", "integer") and "range" not in ps
                         and k not in cur.get("unbounded_ok", [])]
            if unbounded:
                findings.append(Finding(
                    aff, REVIEW, "unbounded_param",
                    "not exposed: no limit is declared for " + ", ".join(f"'{k}'" for k in unbounded) +
                    "; give a range in the override, or list the parameter under unbounded_ok"))
                continue
            if not ok:
                continue

            cname = name_for(vname or aname)
            cap = {
                "name": cname,
                "kind": "action",
                "operations": ["invoke"],
                "params": params,
                "side_effect": side_effect,
                "idempotent": bool(idem),
                "timing": timing,
                "verification": cur.get("verification", {"mode": "none", "reason": "no readback is declared in the Thing Description or the review"}),
                "extensions": {"x-wot": {"affordance": aff,
                                         "reviewed": bool(ov), "td_declared_safe": td_safe}},
            }
            if cur.get("stop"):
                cap["stop"] = True
            if cur.get("preconditions"):
                cap["preconditions"] = ov["preconditions"]
            if cur.get("result"):
                cap["result"] = ov["result"]
            desc = act.get("description")
            if desc:
                cap["description"] = str(desc).strip()[:300]
            caps.append(cap)
            bindings[cname] = {"kind": "action", "href": href(form), "wire": wire,
                               "result": bool(cur.get("result")),
                               "result_key": cur.get("result_key")}
            findings.append(Finding(
                aff, MAPPED, "mapped_from_override" if ov else "mapped_td_safe",
                "exposed from a reviewed override" if ov else "exposed because the TD declares safe: true", cname))

    # ------------------------------------------------------------------ events
    for ename in (td.get("events") or {}):
        findings.append(Finding(
            f"events/{ename}", UNSUPPORTED, "event_unsupported",
            "events are not mapped; poll the related property with watch_state instead"))

    # ------------------------------------------------------------------ security
    auth = _security(td, findings)

    descriptor = {
        "openmhs": "0.1",
        "device": {
            "id": ident,
            "name": title,
            "manufacturer": overrides.get("manufacturer", "unknown"),
            "model": overrides.get("model", slug(title)),
            **({"description": td["description"].strip()[:300]} if td.get("description") else {}),
        },
        "adapter": {"id": adapter_id, "version": "0.1.0"},
        "capabilities": caps,
    }
    return Mapping(descriptor, bindings, findings, auth)


def review_template(td: dict, mapping: Mapping) -> dict:
    """A skeleton for the reviewer: every action not yet exposed, with the fields to fill."""
    todo = {f.affordance.split("/", 1)[1] for f in mapping.findings
            if f.category in ("action_unreviewed", "undeclared_params")}
    actions = {}
    for name in sorted(todo):
        act = (td.get("actions") or {}).get(name, {})
        actions[name] = {
            "expose": False,
            "side_effect": None,
            "idempotent": act.get("idempotent"),
            "timing": {"timeout_ms": None},
            "params": {},
            "verification": {"mode": "none", "reason": "no readback is declared in the Thing Description or the review"},
            "reviewed_by": None,
            "reason": None,
            "td_description": (act.get("description") or "").strip()[:200],
        }
    return {"manufacturer": None, "model": None, "actions": actions}
