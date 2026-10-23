"""Typed models of the gateway's wire format.

Each model mirrors one object in ``spec/schemas`` and is checked against it by
``tests/test_models.py``: the field names must equal the schema's ``properties`` and the
spec's example documents must round-trip unchanged. Fields the schema does not know
(from a newer gateway) are kept in ``extra`` rather than dropped or rejected.
"""

from __future__ import annotations

import dataclasses
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, ClassVar, TypeVar, Union

# ------------------------------------------------------------------ enums


class CommandState(str, Enum):
    """Lifecycle states (spec/lifecycle.md)."""

    CREATED = "CREATED"
    VALIDATING = "VALIDATING"
    DENIED = "DENIED"
    AWAITING_APPROVAL = "AWAITING_APPROVAL"
    APPROVED = "APPROVED"
    EXPIRED = "EXPIRED"
    CANCELLED = "CANCELLED"
    DISPATCHED = "DISPATCHED"
    ACCEPTED = "ACCEPTED"
    REJECTED = "REJECTED"
    FAILED = "FAILED"
    TIMED_OUT = "TIMED_OUT"
    APPLIED = "APPLIED"
    VERIFIED = "VERIFIED"
    VERIFICATION_FAILED = "VERIFICATION_FAILED"
    UNVERIFIABLE = "UNVERIFIABLE"
    UNKNOWN_OUTCOME = "UNKNOWN_OUTCOME"
    RECONCILED_APPLIED = "RECONCILED_APPLIED"
    RECONCILED_NOT_APPLIED = "RECONCILED_NOT_APPLIED"


class Outcome(str, Enum):
    """What a receipt means for the caller, in one word."""

    PENDING = "pending"
    SUCCEEDED = "succeeded"
    UNVERIFIED = "unverified"
    FAILED = "failed"
    DENIED = "denied"
    NOT_EXECUTED = "not_executed"
    UNCERTAIN = "uncertain"


class Decision(str, Enum):
    ALLOW = "allow"
    DENY = "deny"
    REQUIRE_APPROVAL = "require_approval"


class Role(str, Enum):
    OPERATOR = "operator"
    AGENT = "agent"


# ------------------------------------------------------------------ values


@dataclass(frozen=True)
class Quantity:
    """A number with its unit, e.g. ``Quantity(5, "V")``. Units are never guessed."""

    value: float
    unit: str

    def to_dict(self) -> dict[str, Any]:
        return {"value": self.value, "unit": self.unit}


#: A typed value: a quantity, integer, boolean or string (common.schema.json#/$defs/value).
Value = Union[Quantity, int, bool, str, float]


def parse_value(v: Any) -> Any:
    if isinstance(v, dict) and set(v) == {"value", "unit"}:
        return Quantity(v["value"], v["unit"])
    return v


def dump_value(v: Any) -> Any:
    return v.to_dict() if isinstance(v, Quantity) else v


# ------------------------------------------------------------------ machinery

M = TypeVar("M", bound="Model")


@dataclass
class Model:
    """Shared ``from_dict`` / ``to_dict``. Subclasses declare how fields convert."""

    #: field -> Model subclass, for nested objects (lists of them too).
    _nested: ClassVar[dict[str, type]] = {}
    #: fields holding a typed value (Quantity or scalar).
    _values: ClassVar[frozenset[str]] = frozenset()
    #: fields holding a map of typed values.
    _value_maps: ClassVar[frozenset[str]] = frozenset()
    #: fields that are str enums.
    _enums: ClassVar[dict[str, type[Enum]]] = {}
    #: list fields the gateway omits when empty.
    _omit_empty: ClassVar[frozenset[str]] = frozenset()

    @classmethod
    def from_dict(cls: type[M], data: dict[str, Any]) -> M:
        names = {f.name for f in dataclasses.fields(cls) if f.name != "extra"}
        kwargs: dict[str, Any] = {}
        for key, raw in data.items():
            if key not in names:
                continue
            kwargs[key] = cls._decode(key, raw)
        obj = cls(**kwargs)
        obj.extra = {k: v for k, v in data.items() if k not in names}
        return obj

    @classmethod
    def _decode(cls, key: str, raw: Any) -> Any:
        if raw is None:
            return None
        if key in cls._nested:
            sub = cls._nested[key]
            if isinstance(raw, list):
                return [sub.from_dict(x) for x in raw]
            return sub.from_dict(raw)
        if key in cls._values:
            return parse_value(raw)
        if key in cls._value_maps:
            return {k: parse_value(v) for k, v in raw.items()}
        if key in cls._enums:
            try:
                return cls._enums[key](raw)
            except ValueError:
                return raw  # a newer gateway's value: keep it as a string
        return raw

    def to_dict(self) -> dict[str, Any]:
        out: dict[str, Any] = {}
        for f in dataclasses.fields(self):
            if f.name == "extra":
                continue
            v = getattr(self, f.name)
            if v is None or (f.name in self._omit_empty and not v):
                continue
            out[f.name] = _encode(v)
        out.update(self.extra)
        return out


def _encode(v: Any) -> Any:
    if isinstance(v, Model):
        return v.to_dict()
    if isinstance(v, Quantity):
        return v.to_dict()
    if isinstance(v, Enum):
        return v.value
    if isinstance(v, list):
        return [_encode(x) for x in v]
    if isinstance(v, dict):
        return {k: _encode(x) for k, x in v.items()}
    return v


def _extra() -> Any:
    return field(default_factory=dict, repr=False, compare=False)


# ------------------------------------------------------------------ common


@dataclass
class ErrorObject(Model):
    """One reason something was refused or failed (common.schema.json#/$defs/error)."""

    code: str
    message: str
    pointer: str | None = None
    rule_id: str | None = None
    #: Numbers from the violated constraint, e.g. ``{"min": 0, "max": 12, "unit": "V"}``.
    details: dict[str, Any] | None = None
    extra: dict[str, Any] = _extra()


@dataclass
class StateValue(Model):
    """A reading (common.schema.json#/$defs/state_value)."""

    value: Any
    timestamp: str
    source: str
    quality: str
    extra: dict[str, Any] = _extra()
    _values = frozenset({"value"})

    @property
    def good(self) -> bool:
        return self.quality == "GOOD"


# ------------------------------------------------------------------ receipt


@dataclass
class Transition(Model):
    state: CommandState | str
    at: str
    detail: str | None = None
    extra: dict[str, Any] = _extra()
    _enums = {"state": CommandState}


@dataclass
class RuleMatch(Model):
    rule_id: str
    effect: Decision | str
    reason: str | None = None
    extra: dict[str, Any] = _extra()
    _enums = {"effect": Decision}


@dataclass
class PolicyRecord(Model):
    decision: Decision | str
    rules: list[RuleMatch]
    errors: list[ErrorObject] = field(default_factory=list)
    approved_by: str | None = None
    extra: dict[str, Any] = _extra()
    _nested = {"rules": RuleMatch, "errors": ErrorObject}
    _enums = {"decision": Decision}
    _omit_empty = frozenset({"errors"})


@dataclass
class AdapterResult(Model):
    acknowledged: bool | None = None
    reported_applied: bool | None = None
    value: Any = None
    result: dict[str, Any] | None = None
    error: ErrorObject | None = None
    extra: dict[str, Any] = _extra()
    _nested = {"error": ErrorObject}
    _values = frozenset({"value"})
    _value_maps = frozenset({"result"})


@dataclass
class Verification(Model):
    mode: str
    capability: str | None = None
    expected: Any = None
    observed: StateValue | None = None
    matched: bool | None = None
    reason: str | None = None
    extra: dict[str, Any] = _extra()
    _nested = {"observed": StateValue}
    _values = frozenset({"expected"})


@dataclass
class Receipt(Model):
    """The immutable record of a command, including denials (receipt.schema.json)."""

    command_id: str
    principal: str
    #: The command request as submitted (command.schema.json).
    request: dict[str, Any]
    state: CommandState | str
    outcome: Outcome | str
    created_at: str
    transitions: list[Transition]
    policy: PolicyRecord
    correlation_id: str | None = None
    idempotency_key: str | None = None
    deadline_at: str | None = None
    adapter_result: AdapterResult | None = None
    verification: Verification | None = None
    audit_hash: str | None = None
    extra: dict[str, Any] = _extra()
    _nested = {
        "transitions": Transition,
        "policy": PolicyRecord,
        "adapter_result": AdapterResult,
        "verification": Verification,
    }
    _enums = {"state": CommandState, "outcome": Outcome}

    @property
    def confirmed(self) -> bool:
        """True only if the effect was verified by reading the device back."""
        return self.outcome == Outcome.SUCCEEDED

    @property
    def denied(self) -> bool:
        return self.outcome == Outcome.DENIED

    @property
    def pending(self) -> bool:
        return self.outcome == Outcome.PENDING

    @property
    def uncertain(self) -> bool:
        """It may or may not have happened. Never resubmit; reconcile instead."""
        return self.outcome == Outcome.UNCERTAIN

    @property
    def reasons(self) -> list[ErrorObject]:
        """Why it was refused or failed: policy errors, then the adapter's error."""
        out = list(self.policy.errors)
        if self.adapter_result is not None and self.adapter_result.error is not None:
            out.append(self.adapter_result.error)
        return out


@dataclass
class PreflightResult(Model):
    """What ``commands.preflight`` says a submission would get. Advisory."""

    decision: Decision | str
    rules: list[RuleMatch]
    checked_at: str
    errors: list[ErrorObject] = field(default_factory=list)
    extra: dict[str, Any] = _extra()
    _nested = {"rules": RuleMatch, "errors": ErrorObject}
    _enums = {"decision": Decision}
    _omit_empty = frozenset({"errors"})

    @property
    def allowed(self) -> bool:
        return self.decision == Decision.ALLOW


# ------------------------------------------------------------------ gateway results


@dataclass
class GatewayInfo(Model):
    version: str
    spec_version: str
    principal: str
    role: Role | str
    simulation: bool
    devices: int
    extra: dict[str, Any] = _extra()
    _enums = {"role": Role}


@dataclass
class ReadResult(Model):
    """A reading plus how fresh it is. Do not base decisions on it unless ``usable``."""

    device: str
    capability: str
    state: StateValue
    age_ms: int
    usable: bool
    max_age_ms: int | None = None
    extra: dict[str, Any] = _extra()
    _nested = {"state": StateValue}

    @property
    def value(self) -> Any:
        return self.state.value


@dataclass
class Descriptor(Model):
    """A device descriptor (descriptor.schema.json). Nested parts stay plain dicts;
    use :meth:`capability` to look one up."""

    #: The spec version the descriptor is written against, e.g. ``"0.1"``.
    openmhs: str
    device: dict[str, Any]
    adapter: dict[str, Any]
    capabilities: list[dict[str, Any]]
    components: list[dict[str, Any]] | None = None
    extensions: dict[str, Any] | None = None
    extra: dict[str, Any] = _extra()

    @property
    def id(self) -> str:
        return str(self.device["id"])

    def capability(self, name: str) -> dict[str, Any] | None:
        return next((c for c in self.capabilities if c.get("name") == name), None)


@dataclass
class TokenInfo(Model):
    id: str
    principal: str
    role: Role | str
    created_at: str
    extra: dict[str, Any] = _extra()
    _enums = {"role": Role}


@dataclass
class CreatedToken(Model):
    """Returned once by ``tokens.create``. The secret cannot be shown again."""

    id: str
    principal: str
    role: Role | str
    created_at: str
    token: str = field(repr=False)
    extra: dict[str, Any] = _extra()
    _enums = {"role": Role}


@dataclass
class AuditEntry(Model):
    seq: int
    at: str
    kind: str
    subject: str
    data: Any
    hash: str
    extra: dict[str, Any] = _extra()


@dataclass
class AuditVerifyResult(Model):
    records: int
    valid: bool
    first_invalid: int | None = None
    extra: dict[str, Any] = _extra()
