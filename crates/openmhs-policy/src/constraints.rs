//! Constraint validation (spec/descriptor.md §3–5, spec/errors.md "Commands").
//!
//! Two phases, so the engine never touches a device for a request that is already
//! invalid on its face:
//!
//! 1. [`check_command`] with no state: operation, types, units, ranges, parameters.
//! 2. [`check_command`] with state: rate limits, slew limits, freshness, preconditions.
//!
//! Values are normalized to the units declared in the descriptor.

use std::collections::BTreeMap;

use openmhs_types::descriptor::{
    Capability, CompareOp, Descriptor, Operation, PreconditionValue, Tolerance, ValueSpec,
    ValueType, Verification,
};
use openmhs_types::{CommandRequest, ErrorObject, Quantity, StateValue, Timestamp, Value, units};
use serde::{Deserialize, Serialize};

/// Default readback timeout when the descriptor gives none.
pub const DEFAULT_VERIFY_TIMEOUT_MS: u64 = 2_000;

/// A previously dispatched mutation of a capability, for rate and slew limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mutation {
    pub at: Timestamp,
    /// Absolute change of a numeric property in its declared unit (0 otherwise).
    pub delta: f64,
}

/// How the engine confirms the command took effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Expectation {
    Readback {
        capability: String,
        expected: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tolerance: Option<Tolerance>,
        settle_ms: u64,
        timeout_ms: u64,
    },
    None {
        reason: String,
    },
}

/// A request that passed validation, normalized for the adapter.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub operation: Operation,
    pub value: Option<Value>,
    pub params: BTreeMap<String, Value>,
    pub expectation: Expectation,
    /// Absolute change this command makes to a numeric property (0 if unknown/not numeric).
    pub delta: f64,
}

/// Live inputs for the stateful checks.
pub struct Live<'a> {
    pub now: Timestamp,
    /// Latest reading of a capability on the same device, if any.
    pub state: &'a dyn Fn(&str) -> Option<StateValue>,
    /// Mutations of this capability dispatched earlier.
    pub history: &'a [Mutation],
}

/// Capabilities whose current state the stateful checks need.
pub fn state_needed(cap: &Capability) -> Vec<String> {
    let mut names: Vec<String> = cap
        .preconditions
        .iter()
        .map(|p| p.capability.clone())
        .collect();
    if cap.slew.is_some() {
        names.push(cap.name.clone());
    }
    names.sort();
    names.dedup();
    names
}

/// Read-only access check: the capability must support `read`.
pub fn check_read(cap: &Capability) -> Result<(), ErrorObject> {
    if cap.supports(Operation::Read)
        && cap.kind == openmhs_types::descriptor::CapabilityKind::Property
    {
        Ok(())
    } else {
        Err(ErrorObject::new(
            "command.operation.unsupported",
            format!("'{}' cannot be read", cap.name),
        ))
    }
}

/// Validate a write or invoke. Without `live`, only stateless checks run.
pub fn check_command(
    descriptor: &Descriptor,
    cap: &Capability,
    req: &CommandRequest,
    live: Option<&Live<'_>>,
) -> Result<Prepared, Vec<ErrorObject>> {
    let mut errors = Vec::new();
    let operation: Operation = req.operation.into();

    if !operation.is_mutating() || !cap.supports(operation) {
        errors.push(
            ErrorObject::new(
                "command.operation.unsupported",
                format!(
                    "'{}' does not support '{}'",
                    cap.name,
                    format!("{operation:?}").to_lowercase()
                ),
            )
            .at("/operation"),
        );
        return Err(errors);
    }

    // Value (writes).
    let mut value = None;
    if operation == Operation::Write {
        match (&cap.value, &req.value) {
            (Some(spec), Some(v)) => match coerce(spec, v, "/value") {
                Ok(v) => value = Some(v),
                Err(e) => errors.push(e),
            },
            (_, None) => errors.push(
                ErrorObject::new("command.value.type_mismatch", "write requires a value")
                    .at("/value"),
            ),
            (None, Some(_)) => errors.push(
                ErrorObject::new("command.operation.unsupported", "capability has no value")
                    .at("/value"),
            ),
        }
    }

    // Parameters (invokes). Every declared parameter is required in v0.1.
    let mut params = BTreeMap::new();
    if operation == Operation::Invoke {
        let declared = cap.params.clone().unwrap_or_default();
        let given = req.params.clone().unwrap_or_default();
        for name in given.keys().filter(|k| !declared.contains_key(*k)) {
            errors.push(
                ErrorObject::new(
                    "command.param.unknown",
                    format!("'{}' has no parameter '{name}'", cap.name),
                )
                .at(format!("/params/{name}")),
            );
        }
        for (name, spec) in &declared {
            match given.get(name) {
                None => errors.push(
                    ErrorObject::new(
                        "command.param.missing",
                        format!("parameter '{name}' is required"),
                    )
                    .at(format!("/params/{name}")),
                ),
                Some(v) => match coerce(spec, v, &format!("/params/{name}")) {
                    Ok(v) => {
                        params.insert(name.clone(), v);
                    }
                    Err(e) => errors.push(e),
                },
            }
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    let expectation = match expectation(descriptor, cap, value.as_ref(), &params) {
        Ok(e) => e,
        Err(e) => return Err(vec![e]),
    };
    let mut prepared = Prepared {
        operation,
        value,
        params,
        expectation,
        delta: 0.0,
    };

    let Some(live) = live else {
        return Ok(prepared);
    };

    // Rate limit.
    if let Some(rl) = cap.rate_limit {
        let window_start = live.now.minus_ms(rl.per_ms);
        let recent = live.history.iter().filter(|m| m.at > window_start).count();
        if recent >= usize::try_from(rl.max).unwrap_or(usize::MAX) {
            errors.push(
                ErrorObject::new(
                    "command.rate_limited",
                    format!(
                        "at most {} commands per {} ms on '{}'",
                        rl.max, rl.per_ms, cap.name
                    ),
                )
                .detail("max", rl.max)
                .detail("per_ms", rl.per_ms),
            );
        }
    }

    // Slew limit: total commanded change within the window, including this command.
    if let (Some(slew), Some(spec)) = (cap.slew, &cap.value) {
        let current = (live.state)(&cap.name);
        let max_age = cap.freshness.map(|f| f.max_age_ms);
        match current.filter(|s| s.is_usable(live.now, max_age)) {
            None => errors.push(stale(&cap.name, "slew limit needs the current value")),
            Some(cur) => {
                let target = prepared.value.as_ref().and_then(|v| number_in(spec, v));
                let now_val = number_in(spec, &cur.value);
                if let (Some(t), Some(c)) = (target, now_val) {
                    let delta = (t - c).abs();
                    prepared.delta = delta;
                    let window_start = live.now.minus_ms(slew.per_ms);
                    let recent: f64 = live
                        .history
                        .iter()
                        .filter(|m| m.at > window_start)
                        // fold from +0.0: `sum()` of no f64s is -0.0, which prints as "-0".
                        .fold(0.0, |acc, m| acc + m.delta);
                    if recent + delta > slew.max_change + 1e-12 {
                        let u = spec.unit.as_deref().unwrap_or("");
                        let earlier = if recent > 0.0 {
                            format!(" (plus {recent} {u} already changed)")
                        } else {
                            String::new()
                        };
                        errors.push(
                            ErrorObject::new(
                                "command.slew_exceeded",
                                format!(
                                    "change of {delta} {u}{earlier} exceeds the limit of {} {u} per {} ms",
                                    slew.max_change, slew.per_ms,
                                ),
                            )
                            .at("/value")
                            .detail("max_change", slew.max_change)
                            .detail("per_ms", slew.per_ms),
                        );
                    }
                } else {
                    errors.push(stale(&cap.name, "current value is not numeric"));
                }
            }
        }
    }

    // Preconditions (all must hold; missing or stale state fails closed).
    for pre in &cap.preconditions {
        let target = descriptor.capability(&pre.capability);
        let spec = target.and_then(|t| t.value.as_ref());
        let max_age = pre
            .max_age_ms
            .or(target.and_then(|t| t.freshness.map(|f| f.max_age_ms)));
        let reading = (live.state)(&pre.capability).filter(|s| s.is_usable(live.now, max_age));
        let (Some(spec), Some(reading)) = (spec, reading) else {
            errors.push(
                stale(
                    &pre.capability,
                    &format!("precondition '{}' needs a fresh reading", pre.id),
                )
                .rule(pre.id.clone()),
            );
            continue;
        };
        match compare(spec, &reading.value, pre.op, &pre.value) {
            Ok(true) => {}
            Ok(false) => errors.push(
                ErrorObject::new(
                    "command.precondition.failed",
                    pre.description.clone().unwrap_or_else(|| {
                        format!(
                            "precondition '{}' on '{}' does not hold",
                            pre.id, pre.capability
                        )
                    }),
                )
                .rule(pre.id.clone())
                .detail("capability", pre.capability.clone())
                .detail(
                    "observed",
                    serde_json::to_value(&reading.value).unwrap_or_default(),
                ),
            ),
            Err(msg) => errors
                .push(ErrorObject::new("command.precondition.failed", msg).rule(pre.id.clone())),
        }
    }

    if errors.is_empty() {
        Ok(prepared)
    } else {
        Err(errors)
    }
}

fn stale(capability: &str, why: &str) -> ErrorObject {
    ErrorObject::new(
        "command.state.stale",
        format!("{why}: no fresh GOOD-quality reading of '{capability}'"),
    )
    .detail("capability", capability)
}

fn expectation(
    descriptor: &Descriptor,
    cap: &Capability,
    value: Option<&Value>,
    params: &BTreeMap<String, Value>,
) -> Result<Expectation, ErrorObject> {
    let internal = |msg: String| ErrorObject::new("command.verification.invalid", msg);
    match &cap.verification {
        None => Err(internal(format!("'{}' declares no verification", cap.name))),
        Some(Verification::None { reason }) => Ok(Expectation::None {
            reason: reason.clone(),
        }),
        Some(Verification::Readback {
            capability,
            tolerance,
            expect,
            expect_param,
            settle_ms,
            timeout_ms,
        }) => {
            let target_name = capability.clone().unwrap_or_else(|| cap.name.clone());
            let spec = descriptor
                .capability(&target_name)
                .and_then(|t| t.value.as_ref())
                .ok_or_else(|| internal(format!("readback target '{target_name}' has no value")))?;
            let raw = match (expect, expect_param) {
                (Some(v), _) => v.clone(),
                (None, Some(p)) => params
                    .get(p)
                    .cloned()
                    .ok_or_else(|| internal(format!("parameter '{p}' missing")))?,
                (None, None) => value
                    .cloned()
                    .ok_or_else(|| internal("nothing to compare the readback with".into()))?,
            };
            let expected = coerce_unchecked(spec, &raw).map_err(|e| internal(e.message))?;
            Ok(Expectation::Readback {
                capability: target_name,
                expected,
                tolerance: *tolerance,
                settle_ms: settle_ms.unwrap_or(0),
                timeout_ms: timeout_ms.unwrap_or(DEFAULT_VERIFY_TIMEOUT_MS),
            })
        }
    }
}

/// Convert a value to the spec's type and unit, and check its range.
fn coerce(spec: &ValueSpec, v: &Value, pointer: &str) -> Result<Value, ErrorObject> {
    let out = coerce_unchecked(spec, v).map_err(|e| e.at(pointer))?;
    if let (Some(range), Some(n)) = (spec.range, number_in(spec, &out)) {
        let unit = spec.unit.as_deref().unwrap_or("");
        let below = range.min.is_some_and(|min| n < min);
        let above = range.max.is_some_and(|max| n > max);
        if below || above {
            let fmt = |b: Option<f64>| b.map_or("…".to_owned(), |x| x.to_string());
            let mut e = ErrorObject::new(
                "command.value.out_of_range",
                format!(
                    "{n} {unit} is outside the allowed range {}..{} {unit}",
                    fmt(range.min),
                    fmt(range.max)
                ),
            )
            .at(pointer);
            if let Some(min) = range.min {
                e = e.detail("min", min);
            }
            if let Some(max) = range.max {
                e = e.detail("max", max);
            }
            return Err(e.detail("unit", unit));
        }
    }
    Ok(out)
}

/// Type and unit conversion without the range check.
fn coerce_unchecked(spec: &ValueSpec, v: &Value) -> Result<Value, ErrorObject> {
    let mismatch = |expected: &str| {
        ErrorObject::new(
            "command.value.type_mismatch",
            format!("expected {expected}, got {}", v.kind()),
        )
    };
    match (spec.ty, v) {
        (ValueType::Number, Value::Quantity(q)) => {
            let unit = spec.unit.as_deref().unwrap_or_default();
            Ok(Value::Quantity(Quantity {
                value: convert(q, unit)?,
                unit: unit.to_owned(),
            }))
        }
        (ValueType::Number, _) => Err(mismatch(&format!(
            "a quantity {{\"value\": …, \"unit\": \"{}\"}}",
            spec.unit.as_deref().unwrap_or("")
        ))),
        (ValueType::Integer, Value::Integer(i)) => Ok(Value::Integer(*i)),
        (ValueType::Integer, Value::Quantity(q)) if spec.unit.is_some() => {
            let n = convert(q, spec.unit.as_deref().unwrap_or_default())?;
            if n.fract() != 0.0 || !n.is_finite() {
                return Err(mismatch("a whole number"));
            }
            #[allow(clippy::cast_possible_truncation)]
            Ok(Value::Integer(n as i64))
        }
        (ValueType::Integer, _) => Err(mismatch("an integer")),
        (ValueType::Boolean, Value::Bool(b)) => Ok(Value::Bool(*b)),
        (ValueType::Boolean, _) => Err(mismatch("a boolean")),
        (ValueType::Enum, Value::Text(s)) => {
            let allowed = spec.values.as_deref().unwrap_or_default();
            if allowed.iter().any(|a| a == s) {
                Ok(Value::Text(s.clone()))
            } else {
                Err(ErrorObject::new(
                    "command.value.not_allowed",
                    format!("'{s}' is not one of {allowed:?}"),
                ))
            }
        }
        (ValueType::Enum, _) => Err(mismatch("one of the enum values")),
        (ValueType::String, Value::Text(s)) => {
            let too_long = spec
                .max_length
                .is_some_and(|max| u64::try_from(s.chars().count()).unwrap_or(u64::MAX) > max);
            if too_long {
                Err(ErrorObject::new(
                    "command.value.out_of_range",
                    "string is longer than max_length",
                ))
            } else {
                Ok(Value::Text(s.clone()))
            }
        }
        (ValueType::String, _) => Err(mismatch("a string")),
    }
}

fn convert(q: &Quantity, to: &str) -> Result<f64, ErrorObject> {
    if units::unit(&q.unit).is_none() {
        return Err(ErrorObject::new(
            "command.value.unit_unknown",
            format!("unknown unit '{}' (see spec/units.md)", q.unit),
        ));
    }
    units::convert(q.value, &q.unit, to).map_err(|e| {
        ErrorObject::new(
            "command.value.unit_mismatch",
            format!("{e}; expected a value in {to}"),
        )
        .detail("expected_unit", to)
    })
}

/// A numeric value expressed in the spec's unit.
fn number_in(spec: &ValueSpec, v: &Value) -> Option<f64> {
    match v {
        Value::Quantity(q) => match &spec.unit {
            Some(u) => units::convert(q.value, &q.unit, u).ok(),
            None => None,
        },
        #[allow(clippy::cast_precision_loss)]
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

fn compare(
    spec: &ValueSpec,
    actual: &Value,
    op: CompareOp,
    expected: &PreconditionValue,
) -> Result<bool, String> {
    let one = |e: &Value| -> Result<bool, String> {
        match spec.ty {
            ValueType::Number | ValueType::Integer => {
                let a = number_in(spec, actual).ok_or("observed value is not numeric")?;
                let b = number_in(spec, e).ok_or("expected value is not numeric")?;
                Ok(match op {
                    CompareOp::Eq | CompareOp::In => a == b,
                    CompareOp::Ne => a != b,
                    CompareOp::Lt => a < b,
                    CompareOp::Le => a <= b,
                    CompareOp::Gt => a > b,
                    CompareOp::Ge => a >= b,
                })
            }
            _ => Ok(match op {
                CompareOp::Eq | CompareOp::In => actual == e,
                CompareOp::Ne => actual != e,
                _ => return Err("ordering comparison on a non-numeric value".into()),
            }),
        }
    };
    match (op, expected) {
        (CompareOp::In, PreconditionValue::Many(vs)) => {
            for v in vs {
                if one(v)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        (_, PreconditionValue::One(v)) => one(v),
        _ => Err("malformed precondition".into()),
    }
}

/// Whether an observed readback matches the expectation within tolerance.
pub fn readback_matches(expected: &Value, observed: &Value, tolerance: Option<Tolerance>) -> bool {
    match (expected, observed) {
        (Value::Quantity(e), Value::Quantity(o)) => {
            let Ok(o_val) = units::convert(o.value, &o.unit, &e.unit) else {
                return false;
            };
            let diff = (o_val - e.value).abs();
            let allowed = match tolerance {
                Some(t) => t
                    .abs
                    .unwrap_or(0.0)
                    .max(t.rel.unwrap_or(0.0) * e.value.abs()),
                None => 1e-9 * e.value.abs().max(1.0),
            };
            diff <= allowed
        }
        (a, b) => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmhs_types::{Quality, Source};

    fn psu() -> Descriptor {
        let json: serde_json::Value = serde_json::from_str(include_str!(
            "../../../spec/examples/valid/power-supply.json"
        ))
        .unwrap();
        openmhs_types::validate_descriptor(&json).unwrap()
    }

    fn write(cap: &str, value: serde_json::Value) -> CommandRequest {
        serde_json::from_value(serde_json::json!({
            "target": {"device": "sim.psu-01", "capability": cap},
            "operation": "write",
            "value": value
        }))
        .unwrap()
    }

    fn reading(v: Value, at: i64) -> StateValue {
        StateValue {
            value: v,
            timestamp: Timestamp::from_millis(at),
            source: Source::Device,
            quality: Quality::Good,
        }
    }

    fn codes(r: Result<Prepared, Vec<ErrorObject>>) -> Vec<String> {
        r.err()
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.code)
            .collect()
    }

    const SETPOINT: &str = "power.output.voltage.setpoint";

    #[test]
    fn converts_units_and_checks_range() {
        let d = psu();
        let cap = d.capability(SETPOINT).unwrap();
        let ok = check_command(
            &d,
            cap,
            &write(SETPOINT, serde_json::json!({"value": 5000, "unit": "mV"})),
            None,
        )
        .unwrap();
        assert_eq!(
            ok.value,
            Some(Value::Quantity(Quantity {
                value: 5.0,
                unit: "V".into()
            }))
        );

        assert_eq!(
            codes(check_command(
                &d,
                cap,
                &write(SETPOINT, serde_json::json!({"value": 48, "unit": "V"})),
                None
            )),
            ["command.value.out_of_range"]
        );
        assert_eq!(
            codes(check_command(
                &d,
                cap,
                &write(SETPOINT, serde_json::json!({"value": 5, "unit": "volts"})),
                None
            )),
            ["command.value.unit_unknown"]
        );
        assert_eq!(
            codes(check_command(
                &d,
                cap,
                &write(SETPOINT, serde_json::json!({"value": 5, "unit": "A"})),
                None
            )),
            ["command.value.unit_mismatch"]
        );
        assert_eq!(
            codes(check_command(
                &d,
                cap,
                &write(SETPOINT, serde_json::json!(5)),
                None
            )),
            ["command.value.type_mismatch"]
        );
    }

    #[test]
    fn preconditions_fail_closed_without_fresh_state() {
        let d = psu();
        let cap = d.capability(SETPOINT).unwrap();
        let req = write(SETPOINT, serde_json::json!({"value": 1, "unit": "V"}));
        let now = Timestamp::from_millis(10_000);
        let current = reading(
            Value::Quantity(Quantity {
                value: 0.5,
                unit: "V".into(),
            }),
            9_990,
        );

        // Nothing known about protection.tripped.
        let state = |c: &str| (c == SETPOINT).then(|| current.clone());
        let live = Live {
            now,
            state: &state,
            history: &[],
        };
        assert_eq!(
            codes(check_command(&d, cap, &req, Some(&live))),
            ["command.state.stale"]
        );

        // Known, but 5 s old (max_age 1000 ms).
        let state = |c: &str| match c {
            SETPOINT => Some(current.clone()),
            _ => Some(reading(Value::Bool(false), 5_000)),
        };
        let live = Live {
            now,
            state: &state,
            history: &[],
        };
        assert_eq!(
            codes(check_command(&d, cap, &req, Some(&live))),
            ["command.state.stale"]
        );

        // Fresh and tripped: precondition fails.
        let state = |c: &str| match c {
            SETPOINT => Some(current.clone()),
            _ => Some(reading(Value::Bool(true), 9_999)),
        };
        let live = Live {
            now,
            state: &state,
            history: &[],
        };
        assert_eq!(
            codes(check_command(&d, cap, &req, Some(&live))),
            ["command.precondition.failed"]
        );

        // Fresh and not tripped: passes.
        let state = |c: &str| match c {
            SETPOINT => Some(current.clone()),
            _ => Some(reading(Value::Bool(false), 9_999)),
        };
        let live = Live {
            now,
            state: &state,
            history: &[],
        };
        let p = check_command(&d, cap, &req, Some(&live)).unwrap();
        assert!((p.delta - 0.5).abs() < 1e-12);
    }

    #[test]
    fn bad_quality_is_not_usable() {
        let d = psu();
        let cap = d.capability(SETPOINT).unwrap();
        let req = write(SETPOINT, serde_json::json!({"value": 1, "unit": "V"}));
        let now = Timestamp::from_millis(10_000);
        let state = |c: &str| {
            let mut r = match c {
                SETPOINT => reading(
                    Value::Quantity(Quantity {
                        value: 0.5,
                        unit: "V".into(),
                    }),
                    9_999,
                ),
                _ => reading(Value::Bool(false), 9_999),
            };
            r.quality = Quality::Bad;
            Some(r)
        };
        let live = Live {
            now,
            state: &state,
            history: &[],
        };
        assert!(
            codes(check_command(&d, cap, &req, Some(&live)))
                .iter()
                .all(|c| c == "command.state.stale")
        );
    }

    #[test]
    fn slew_and_rate_limits() {
        let d = psu();
        let cap = d.capability(SETPOINT).unwrap(); // slew 2 V / 1000 ms, rate 5 / 1000 ms
        let now = Timestamp::from_millis(10_000);
        let state = |c: &str| match c {
            SETPOINT => Some(reading(
                Value::Quantity(Quantity {
                    value: 0.0,
                    unit: "V".into(),
                }),
                9_999,
            )),
            _ => Some(reading(Value::Bool(false), 9_999)),
        };
        let big = write(SETPOINT, serde_json::json!({"value": 3, "unit": "V"}));
        let live = Live {
            now,
            state: &state,
            history: &[],
        };
        assert_eq!(
            codes(check_command(&d, cap, &big, Some(&live))),
            ["command.slew_exceeded"]
        );

        let small = write(SETPOINT, serde_json::json!({"value": 1.5, "unit": "V"}));
        let history = [Mutation {
            at: Timestamp::from_millis(9_500),
            delta: 1.0,
        }];
        let live = Live {
            now,
            state: &state,
            history: &history,
        };
        assert_eq!(
            codes(check_command(&d, cap, &small, Some(&live))),
            ["command.slew_exceeded"]
        );

        // Old history falls out of the window.
        let history = [Mutation {
            at: Timestamp::from_millis(8_000),
            delta: 2.0,
        }];
        let live = Live {
            now,
            state: &state,
            history: &history,
        };
        assert!(check_command(&d, cap, &small, Some(&live)).is_ok());

        let tiny = write(SETPOINT, serde_json::json!({"value": 0.1, "unit": "V"}));
        let history: Vec<Mutation> = (0..5)
            .map(|i| Mutation {
                at: Timestamp::from_millis(9_100 + i * 100),
                delta: 0.0,
            })
            .collect();
        let live = Live {
            now,
            state: &state,
            history: &history,
        };
        assert_eq!(
            codes(check_command(&d, cap, &tiny, Some(&live))),
            ["command.rate_limited"]
        );
    }

    #[test]
    fn readback_tolerance() {
        let v = |x: f64, u: &str| {
            Value::Quantity(Quantity {
                value: x,
                unit: u.into(),
            })
        };
        let tol = Some(Tolerance {
            abs: Some(0.05),
            rel: None,
        });
        assert!(readback_matches(&v(5.0, "V"), &v(5.04, "V"), tol));
        assert!(!readback_matches(&v(5.0, "V"), &v(5.06, "V"), tol));
        assert!(readback_matches(&v(5.0, "V"), &v(5_000.0, "mV"), None));
        assert!(!readback_matches(&v(5.0, "V"), &v(5.0, "A"), tol));
        assert!(readback_matches(
            &Value::Bool(true),
            &Value::Bool(true),
            None
        ));
    }
}
