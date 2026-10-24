//! Descriptor validation: structural (JSON Schema) then semantic (spec/descriptor.md §5).

use std::collections::{BTreeMap, HashSet};

use crate::descriptor::{
    Capability, CapabilityKind, CompareOp, Descriptor, Operation, PreconditionValue, Relationship,
    RelationshipKind, SideEffect, ValueSpec, ValueType, Verification,
};
use crate::issue::Issue;
use crate::schema::{self, Schema};
use crate::units;
use crate::value::Value;

/// Specification versions this implementation accepts.
pub const SUPPORTED_SPEC_VERSIONS: &[&str] = &["0.1"];

/// Parse and fully validate a descriptor. Returns every issue found, not just the first.
pub fn validate_descriptor(json: &serde_json::Value) -> Result<Descriptor, Vec<Issue>> {
    let structural = schema::validate(Schema::Descriptor, json);
    if !structural.is_empty() {
        return Err(structural);
    }
    let descriptor: Descriptor = serde_json::from_value(json.clone()).map_err(|e| {
        // Schema and types disagree: a defect, but report it rather than panic.
        vec![Issue::new(
            "descriptor.schema",
            "",
            format!("does not match descriptor types: {e}"),
        )]
    })?;
    let issues = semantic_issues(&descriptor);
    if issues.is_empty() {
        Ok(descriptor)
    } else {
        Err(issues)
    }
}

/// Semantic checks on an already-parsed descriptor.
pub fn semantic_issues(d: &Descriptor) -> Vec<Issue> {
    let mut v = Checker {
        d,
        issues: Vec::new(),
    };
    v.check();
    v.issues
}

/// Check one relationship as if `d` declared it: the same rules, for relationships the
/// operator's topology adds to a registered device. `at` prefixes the issue pointers.
pub fn relationship_issues(d: &Descriptor, r: &Relationship, at: &str) -> Vec<Issue> {
    let mut v = Checker {
        d,
        issues: Vec::new(),
    };
    v.check_relationship(r, at);
    v.issues
}

struct Checker<'a> {
    d: &'a Descriptor,
    issues: Vec<Issue>,
}

impl<'a> Checker<'a> {
    fn push(&mut self, code: &str, pointer: String, message: impl Into<String>) {
        self.issues.push(Issue::new(code, pointer, message));
    }

    fn check(&mut self) {
        if !SUPPORTED_SPEC_VERSIONS.contains(&self.d.openmhs.as_str()) {
            self.push(
                "descriptor.version.unsupported",
                "/openmhs".into(),
                format!(
                    "spec version '{}' is not supported (supported: {SUPPORTED_SPEC_VERSIONS:?})",
                    self.d.openmhs
                ),
            );
        }

        let mut components = HashSet::new();
        for (i, c) in self.d.components.iter().enumerate() {
            if !components.insert(c.id.as_str()) {
                self.push(
                    "descriptor.component.duplicate",
                    format!("/components/{i}/id"),
                    format!("duplicate component '{}'", c.id),
                );
            }
        }

        let mut names = HashSet::new();
        for (i, cap) in self.d.capabilities.iter().enumerate() {
            let at = format!("/capabilities/{i}");
            if !names.insert(cap.name.as_str()) {
                self.push(
                    "descriptor.capability.duplicate",
                    format!("{at}/name"),
                    format!("duplicate capability '{}'", cap.name),
                );
            }
            if let Some(comp) = &cap.component
                && !components.contains(comp.as_str())
            {
                self.push(
                    "descriptor.component.unknown",
                    format!("{at}/component"),
                    format!("component '{comp}' is not declared"),
                );
            }
            self.check_capability(cap, &at);
        }

        let mut seen = HashSet::new();
        for (i, r) in self.d.relationships.iter().enumerate() {
            let at = format!("/relationships/{i}");
            if !seen.insert(relationship_key(r)) {
                self.push(
                    "descriptor.relationship.duplicate",
                    at.clone(),
                    format!("{} relationship declared twice", r.kind.name()),
                );
            }
            self.check_relationship(r, &at);
        }
    }

    /// Each kind uses its own fields (spec/descriptor.md §7) and no others.
    fn check_relationship(&mut self, r: &Relationship, at: &str) {
        if let Some(comp) = &r.component
            && !self.d.components.iter().any(|c| &c.id == comp)
        {
            self.push(
                "descriptor.component.unknown",
                format!("{at}/component"),
                format!("component '{comp}' is not declared"),
            );
        }
        let kind = r.kind.name();
        let (needs, fields): (&str, [(&str, bool); 4]) = match r.kind {
            RelationshipKind::Contains | RelationshipKind::DependsOn => (
                "target",
                [
                    ("target", true),
                    ("resource", false),
                    ("frame", false),
                    ("capabilities", false),
                ],
            ),
            RelationshipKind::SharesResource => (
                "resource",
                [
                    ("target", false),
                    ("resource", true),
                    ("frame", false),
                    ("capabilities", false),
                ],
            ),
            RelationshipKind::Frame => (
                "frame",
                [
                    ("target", false),
                    ("resource", false),
                    ("frame", true),
                    ("capabilities", true),
                ],
            ),
        };
        let present = |field: &str| match field {
            "target" => r.target.is_some(),
            "resource" => r.resource.is_some(),
            "frame" => r.frame.is_some(),
            _ => !r.capabilities.is_empty(),
        };
        if !present(needs) {
            self.push(
                "descriptor.relationship.field_missing",
                format!("{at}/{needs}"),
                format!("a {kind} relationship needs '{needs}'"),
            );
        }
        for (field, allowed) in fields {
            if !allowed && present(field) {
                self.push(
                    "descriptor.relationship.field_not_applicable",
                    format!("{at}/{field}"),
                    format!("'{field}' does not apply to a {kind} relationship"),
                );
            }
        }
        if r.target.as_deref() == Some(self.d.device.id.as_str()) {
            self.push(
                "descriptor.relationship.self",
                format!("{at}/target"),
                format!("a device cannot have a {kind} relationship with itself"),
            );
        }
        if r.kind == RelationshipKind::Frame {
            for (j, name) in r.capabilities.iter().enumerate() {
                if self.d.capability(name).is_none() {
                    self.push(
                        "descriptor.relationship.unknown_capability",
                        format!("{at}/capabilities/{j}"),
                        format!("capability '{name}' does not exist"),
                    );
                }
            }
        }
    }

    fn check_capability(&mut self, cap: &'a Capability, at: &str) {
        // Kind ↔ operations.
        let allowed: &[Operation] = match cap.kind {
            CapabilityKind::Property => &[Operation::Read, Operation::Write, Operation::Subscribe],
            CapabilityKind::Action => &[Operation::Invoke],
            CapabilityKind::Event => &[Operation::Subscribe],
        };
        for (j, op) in cap.operations.iter().enumerate() {
            if !allowed.contains(op) {
                self.push(
                    "descriptor.capability.invalid_operation",
                    format!("{at}/operations/{j}"),
                    format!(
                        "{:?} capability '{}' cannot support '{op:?}'",
                        cap.kind, cap.name
                    )
                    .to_lowercase(),
                );
            }
        }

        // Kind ↔ value shape.
        match cap.kind {
            CapabilityKind::Property => {
                if cap.value.is_none() {
                    self.push(
                        "descriptor.capability.value_missing",
                        at.to_owned(),
                        format!("property '{}' must declare 'value'", cap.name),
                    );
                }
                for (field, present) in [
                    ("params", cap.params.is_some()),
                    ("result", cap.result.is_some()),
                    ("payload", cap.payload.is_some()),
                ] {
                    if present {
                        self.push(
                            "descriptor.capability.field_not_applicable",
                            format!("{at}/{field}"),
                            format!("properties do not take '{field}'"),
                        );
                    }
                }
            }
            CapabilityKind::Action => {
                for (field, present) in [
                    ("value", cap.value.is_some()),
                    ("payload", cap.payload.is_some()),
                ] {
                    if present {
                        self.push(
                            "descriptor.capability.field_not_applicable",
                            format!("{at}/{field}"),
                            format!("actions do not take '{field}'"),
                        );
                    }
                }
            }
            CapabilityKind::Event => {
                for (field, present) in [
                    ("value", cap.value.is_some()),
                    ("params", cap.params.is_some()),
                    ("result", cap.result.is_some()),
                ] {
                    if present {
                        self.push(
                            "descriptor.capability.field_not_applicable",
                            format!("{at}/{field}"),
                            format!("events do not take '{field}'"),
                        );
                    }
                }
            }
        }

        // Value specs.
        if let Some(spec) = &cap.value {
            self.check_value_spec(spec, &format!("{at}/value"));
        }
        for (field, map) in [
            ("params", &cap.params),
            ("result", &cap.result),
            ("payload", &cap.payload),
        ] {
            if let Some(map) = map {
                for (name, spec) in map {
                    self.check_value_spec(spec, &format!("{at}/{field}/{name}"));
                }
            }
        }

        // Side effects and verification.
        let mutating = cap.is_mutating();
        if mutating && cap.side_effect == SideEffect::None {
            self.push(
                "descriptor.capability.side_effect_mismatch",
                format!("{at}/side_effect"),
                format!(
                    "'{}' can change device state, so side_effect cannot be 'none'",
                    cap.name
                ),
            );
        }
        if !mutating && cap.side_effect != SideEffect::None {
            self.push(
                "descriptor.capability.side_effect_mismatch",
                format!("{at}/side_effect"),
                format!(
                    "'{}' cannot change device state, so side_effect must be 'none'",
                    cap.name
                ),
            );
        }
        match (&cap.verification, mutating) {
            (None, true) => self.push(
                "descriptor.capability.verification_missing",
                at.to_owned(),
                format!("'{}' changes device state and must declare 'verification' (readback, or none with a reason)", cap.name),
            ),
            (Some(_), false) => self.push(
                "descriptor.verification.not_applicable",
                format!("{at}/verification"),
                "verification only applies to capabilities that write or invoke",
            ),
            (Some(ver), true) => self.check_verification(cap, ver, &format!("{at}/verification")),
            (None, false) => {}
        }

        // Constraints that only make sense on numeric properties.
        let numeric_value = cap
            .value
            .as_ref()
            .is_some_and(|s| matches!(s.ty, ValueType::Number | ValueType::Integer));
        if cap.slew.is_some() && !(cap.kind == CapabilityKind::Property && numeric_value) {
            self.push(
                "descriptor.constraint.not_applicable",
                format!("{at}/slew"),
                "slew limits only apply to numeric properties",
            );
        }

        // Preconditions.
        let mut ids = HashSet::new();
        for (k, pre) in cap.preconditions.iter().enumerate() {
            let pat = format!("{at}/preconditions/{k}");
            if !ids.insert(pre.id.as_str()) {
                self.push(
                    "descriptor.precondition.duplicate_id",
                    format!("{pat}/id"),
                    format!("duplicate precondition id '{}'", pre.id),
                );
            }
            let Some(target) = self.d.capability(&pre.capability) else {
                self.push(
                    "descriptor.precondition.unknown_capability",
                    format!("{pat}/capability"),
                    format!("capability '{}' does not exist", pre.capability),
                );
                continue;
            };
            if !target.is_readable_property() {
                self.push(
                    "descriptor.precondition.not_readable",
                    format!("{pat}/capability"),
                    format!("'{}' is not a readable property", pre.capability),
                );
                continue;
            }
            let Some(spec) = &target.value else { continue };
            if let Err(msg) = check_comparison(spec, pre.op, &pre.value) {
                self.push(
                    "descriptor.precondition.type_mismatch",
                    format!("{pat}/value"),
                    msg,
                );
            }
        }
    }

    fn check_verification(&mut self, cap: &Capability, ver: &Verification, at: &str) {
        let Verification::Readback {
            capability,
            expect,
            expect_param,
            ..
        } = ver
        else {
            return;
        };
        let target = match (capability, cap.kind) {
            (Some(name), _) => match self.d.capability(name) {
                Some(t) => t,
                None => {
                    self.push(
                        "descriptor.verification.unknown_capability",
                        format!("{at}/capability"),
                        format!("capability '{name}' does not exist"),
                    );
                    return;
                }
            },
            (None, CapabilityKind::Property) => cap,
            (None, _) => {
                self.push(
                    "descriptor.verification.capability_missing",
                    at.to_owned(),
                    "actions must name the property to read back",
                );
                return;
            }
        };
        if !target.is_readable_property() {
            self.push(
                "descriptor.verification.not_readable",
                format!("{at}/capability"),
                format!("'{}' is not a readable property", target.name),
            );
            return;
        }
        let Some(target_spec) = &target.value else {
            return;
        };

        if cap.kind == CapabilityKind::Action && expect.is_none() && expect_param.is_none() {
            self.push(
                "descriptor.verification.expectation_missing",
                at.to_owned(),
                "action readback needs 'expect' or 'expect_param'",
            );
        }
        if expect.is_some() && expect_param.is_some() {
            self.push(
                "descriptor.verification.ambiguous_expectation",
                at.to_owned(),
                "use either 'expect' or 'expect_param', not both",
            );
        }
        if let Some(value) = expect
            && let Err(msg) = check_comparison(
                target_spec,
                CompareOp::Eq,
                &PreconditionValue::One(value.clone()),
            )
        {
            self.push(
                "descriptor.verification.type_mismatch",
                format!("{at}/expect"),
                msg,
            );
        }
        if let Some(param) = expect_param {
            match cap.params.as_ref().and_then(|p| p.get(param)) {
                None => self.push(
                    "descriptor.verification.unknown_param",
                    format!("{at}/expect_param"),
                    format!("'{}' has no parameter '{param}'", cap.name),
                ),
                Some(pspec) => {
                    if !specs_comparable(pspec, target_spec) {
                        self.push(
                            "descriptor.verification.type_mismatch",
                            format!("{at}/expect_param"),
                            format!(
                                "parameter '{param}' cannot be compared with '{}'",
                                target.name
                            ),
                        );
                    }
                }
            }
        }
    }

    fn check_value_spec(&mut self, spec: &ValueSpec, at: &str) {
        let numeric = matches!(spec.ty, ValueType::Number | ValueType::Integer);

        if let Some(q) = &spec.quantity
            && !units::is_quantity(q)
        {
            self.push(
                "descriptor.quantity.unknown",
                format!("{at}/quantity"),
                format!("unknown quantity kind '{q}'"),
            );
        }
        if let Some(code) = &spec.unit {
            match units::unit(code) {
                None => self.push(
                    "descriptor.unit.unknown",
                    format!("{at}/unit"),
                    format!("unknown unit '{code}' (see spec/units.md)"),
                ),
                Some(def) => {
                    if let Some(q) = &spec.quantity
                        && units::is_quantity(q)
                        && def.quantity != q
                    {
                        self.push(
                            "descriptor.unit.quantity_mismatch",
                            format!("{at}/unit"),
                            format!("unit '{code}' measures {}, not {q}", def.quantity),
                        );
                    }
                }
            }
        }
        if !numeric && (spec.quantity.is_some() || spec.unit.is_some()) {
            self.push(
                "descriptor.constraint.not_applicable",
                at.to_owned(),
                "quantity and unit only apply to number and integer values",
            );
        }
        if let Some(r) = spec.range {
            if !numeric {
                self.push(
                    "descriptor.constraint.not_applicable",
                    format!("{at}/range"),
                    "range only applies to number and integer values",
                );
            } else if let (Some(min), Some(max)) = (r.min, r.max)
                && min > max
            {
                self.push(
                    "descriptor.constraint.range_inverted",
                    format!("{at}/range"),
                    format!("min {min} is greater than max {max}"),
                );
            }
        }
        if spec.values.is_some() && spec.ty != ValueType::Enum {
            self.push(
                "descriptor.constraint.not_applicable",
                format!("{at}/values"),
                "values only apply to enum values",
            );
        }
        if spec.max_length.is_some() && spec.ty != ValueType::String {
            self.push(
                "descriptor.constraint.not_applicable",
                format!("{at}/max_length"),
                "max_length only applies to string values",
            );
        }
    }
}

/// Whether two value specs hold comparable values (same type, same quantity kind).
fn specs_comparable(a: &ValueSpec, b: &ValueSpec) -> bool {
    let quantity = |s: &ValueSpec| s.unit.as_deref().and_then(units::unit).map(|u| u.quantity);
    match (a.ty, b.ty) {
        (ValueType::Number, ValueType::Number) => {
            quantity(a).is_some() && quantity(a) == quantity(b)
        }
        (ValueType::Integer, ValueType::Integer) => quantity(a) == quantity(b),
        (ValueType::Enum, ValueType::Enum) => a.values == b.values,
        (x, y) => x == y,
    }
}

/// Check that `value` can be compared to a value of `spec` with `op`.
pub fn check_comparison(
    spec: &ValueSpec,
    op: CompareOp,
    value: &PreconditionValue,
) -> Result<(), String> {
    let values: Vec<&Value> = match (op, value) {
        (CompareOp::In, PreconditionValue::Many(vs)) => vs.iter().collect(),
        (CompareOp::In, PreconditionValue::One(_)) => {
            return Err("'in' requires an array of values".into());
        }
        (_, PreconditionValue::Many(_)) => {
            return Err(format!("'{op:?}' requires a single value").to_lowercase());
        }
        (_, PreconditionValue::One(v)) => vec![v],
    };
    if op.is_ordering() && !matches!(spec.ty, ValueType::Number | ValueType::Integer) {
        return Err(format!(
            "ordering comparison '{op:?}' is not defined for {:?} values",
            spec.ty
        )
        .to_lowercase());
    }
    for v in values {
        check_value_against(spec, v)?;
    }
    Ok(())
}

fn check_value_against(spec: &ValueSpec, v: &Value) -> Result<(), String> {
    match (spec.ty, v) {
        (ValueType::Number, Value::Quantity(q)) => {
            let target_unit = spec.unit.as_deref().unwrap_or_default();
            units::convert(q.value, &q.unit, target_unit)
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        (ValueType::Number, other) => Err(format!(
            "expected a quantity {{value, unit}}, got {}",
            other.kind()
        )),
        (ValueType::Integer, Value::Integer(_)) => Ok(()),
        (ValueType::Integer, Value::Quantity(q)) if spec.unit.is_some() => {
            let target_unit = spec.unit.as_deref().unwrap_or_default();
            units::convert(q.value, &q.unit, target_unit)
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        (ValueType::Boolean, Value::Bool(_)) => Ok(()),
        (ValueType::Enum, Value::Text(s)) => {
            let allowed = spec.values.as_deref().unwrap_or_default();
            if allowed.iter().any(|a| a == s) {
                Ok(())
            } else {
                Err(format!("'{s}' is not one of {allowed:?}"))
            }
        }
        (ValueType::String, Value::Text(_)) => Ok(()),
        (ty, other) => Err(format!("expected {ty:?}, got {}", other.kind()).to_lowercase()),
    }
}

/// Group issues by code; handy for reports.
pub fn issues_by_code(issues: &[Issue]) -> BTreeMap<&str, usize> {
    let mut map = BTreeMap::new();
    for i in issues {
        *map.entry(i.code.as_str()).or_default() += 1;
    }
    map
}

/// Two relationships are the same if they say the same thing about the same part.
fn relationship_key(r: &Relationship) -> (RelationshipKind, Option<&str>, Option<&str>) {
    let object = r
        .target
        .as_deref()
        .or(r.resource.as_deref())
        .or(r.frame.as_deref());
    (r.kind, r.component.as_deref(), object)
}
