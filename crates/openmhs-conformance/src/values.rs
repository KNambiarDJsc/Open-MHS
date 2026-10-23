//! Choosing values for generated commands.
//!
//! The suite writes small, in-range changes so that rate and slew limits, verification
//! tolerances and the device itself stay comfortable: a step is 5% of the declared range,
//! at most half the slew limit, and always distinguishable by readback.

use openmhs_types::Value;
use openmhs_types::descriptor::{Capability, Slew, Tolerance, ValueSpec, ValueType, Verification};
use serde_json::{Map, json};

const TEXT_SAMPLE: &str = "openmhs-conformance";

/// The capability whose reading confirms `cap`, if readback is declared.
pub fn readback_capability(cap: &Capability) -> Option<&str> {
    match &cap.verification {
        Some(Verification::Readback { capability, .. }) => {
            Some(capability.as_deref().unwrap_or(&cap.name))
        }
        _ => None,
    }
}

fn tolerance(cap: &Capability) -> Option<Tolerance> {
    match &cap.verification {
        Some(Verification::Readback { tolerance, .. }) => *tolerance,
        _ => None,
    }
}

/// A value to write to `cap`, different from `current` where the type allows it.
pub fn write_value(cap: &Capability, current: Option<&Value>) -> Result<serde_json::Value, String> {
    let spec = cap
        .value
        .as_ref()
        .ok_or_else(|| format!("'{}' declares no value", cap.name))?;
    next_value(spec, current, cap.slew, tolerance(cap))
}

/// Parameters for invoking `cap`. A parameter that verification reads back moves a small
/// step away from `readback` (the current reading); the others take neutral values.
pub fn params(
    cap: &Capability,
    readback: Option<&Value>,
) -> Result<Map<String, serde_json::Value>, String> {
    let expect_param = match &cap.verification {
        Some(Verification::Readback { expect_param, .. }) => expect_param.as_deref(),
        _ => None,
    };
    let mut out = Map::new();
    for (name, spec) in cap.params.iter().flatten() {
        let v = if Some(name.as_str()) == expect_param {
            next_value(spec, readback, None, tolerance(cap))?
        } else {
            neutral_value(spec)?
        };
        out.insert(name.clone(), v);
    }
    Ok(out)
}

fn unit(spec: &ValueSpec) -> &str {
    spec.unit.as_deref().unwrap_or("1")
}

/// `value` as a number in `spec`'s unit, if it is one.
fn number_in(spec: &ValueSpec, value: &Value) -> Option<f64> {
    match value {
        Value::Quantity(q) => openmhs_types::units::convert(q.value, &q.unit, unit(spec)).ok(),
        #[allow(clippy::cast_precision_loss)]
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

fn round(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

fn bounds(spec: &ValueSpec) -> (Option<f64>, Option<f64>) {
    spec.range.as_ref().map_or((None, None), |r| (r.min, r.max))
}

/// A small step away from `current`, inside the range.
fn step_from(spec: &ValueSpec, current: f64, slew: Option<Slew>, tol: Option<Tolerance>) -> f64 {
    let (min, max) = bounds(spec);
    let span = match (min, max) {
        (Some(a), Some(b)) => b - a,
        _ => current.abs().max(1.0) * 2.0,
    };
    if span <= 0.0 {
        return min.unwrap_or(current);
    }
    let mut step = span * 0.05;
    // Readback must be able to tell the new value from the old one.
    if let Some(t) = tol {
        let needed = t
            .abs
            .unwrap_or(0.0)
            .max(t.rel.unwrap_or(0.0) * current.abs())
            * 4.0;
        step = step.max(needed);
    }
    // The slew limit is in the canonical unit; convert the allowance to the declared one.
    if let Some(s) = slew {
        let scale = openmhs_types::units::unit(unit(spec)).map_or(1.0, |u| u.scale);
        step = step.min(s.max_change / scale * 0.5);
    }
    let up = current + step;
    let down = current - step;
    let target = if max.is_none_or(|m| up <= m) {
        up
    } else if min.is_none_or(|m| down >= m) {
        down
    } else {
        min.unwrap_or(current)
    };
    round(target)
}

fn midpoint(spec: &ValueSpec) -> f64 {
    match bounds(spec) {
        (Some(a), Some(b)) => round(a + (b - a) / 2.0),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => 1.0,
    }
}

fn number(spec: &ValueSpec, x: f64) -> serde_json::Value {
    if spec.ty == ValueType::Integer {
        #[allow(clippy::cast_possible_truncation)]
        let i = x.round() as i64;
        json!(i)
    } else {
        json!({ "value": x, "unit": unit(spec) })
    }
}

fn next_value(
    spec: &ValueSpec,
    current: Option<&Value>,
    slew: Option<Slew>,
    tol: Option<Tolerance>,
) -> Result<serde_json::Value, String> {
    Ok(match spec.ty {
        ValueType::Number | ValueType::Integer => {
            let x = match current.and_then(|v| number_in(spec, v)) {
                Some(c) => {
                    let mut t = step_from(spec, c, slew, tol);
                    if spec.ty == ValueType::Integer && t.round() == c.round() {
                        t = if bounds(spec).1.is_none_or(|m| c + 1.0 <= m) {
                            c + 1.0
                        } else {
                            c - 1.0
                        };
                    }
                    t
                }
                None => midpoint(spec),
            };
            number(spec, x)
        }
        ValueType::Boolean => json!(!matches!(current, Some(Value::Bool(true)))),
        ValueType::Enum => {
            let values = spec.values.as_deref().unwrap_or_default();
            let now = match current {
                Some(Value::Text(t)) => Some(t.as_str()),
                _ => None,
            };
            let pick = values
                .iter()
                .find(|v| Some(v.as_str()) != now)
                .or(values.first())
                .ok_or("enum declares no values")?;
            json!(pick)
        }
        ValueType::String => json!(sample_text(spec)),
    })
}

fn neutral_value(spec: &ValueSpec) -> Result<serde_json::Value, String> {
    Ok(match spec.ty {
        ValueType::Number | ValueType::Integer => number(spec, midpoint(spec)),
        ValueType::Boolean => json!(true),
        ValueType::Enum => json!(
            spec.values
                .as_deref()
                .and_then(<[String]>::first)
                .ok_or("enum declares no values")?
        ),
        ValueType::String => json!(sample_text(spec)),
    })
}

fn sample_text(spec: &ValueSpec) -> String {
    let max = spec
        .max_length
        .and_then(|m| usize::try_from(m).ok())
        .unwrap_or(TEXT_SAMPLE.len());
    TEXT_SAMPLE.chars().take(max.max(1)).collect()
}

/// Whether a reading has the type, unit and range `spec` declares. `Err` explains why not.
pub fn check_reading(spec: &ValueSpec, value: &Value) -> Result<(), String> {
    let range = |x: f64| -> Result<(), String> {
        let (min, max) = bounds(spec);
        if min.is_some_and(|m| x < m) || max.is_some_and(|m| x > m) {
            let show = |b: Option<f64>| b.map_or(String::new(), |b| b.to_string());
            return Err(format!(
                "{x} is outside the declared range {}..{} {}",
                show(min),
                show(max),
                unit(spec)
            ));
        }
        Ok(())
    };
    match (spec.ty, value) {
        (ValueType::Number, Value::Quantity(q)) => {
            if q.unit != unit(spec) {
                return Err(format!(
                    "reported in '{}'; readings must use the declared unit '{}'",
                    q.unit,
                    unit(spec)
                ));
            }
            if !q.value.is_finite() {
                return Err(format!("{} is not a finite number", q.value));
            }
            range(q.value)
        }
        #[allow(clippy::cast_precision_loss)]
        (ValueType::Integer, Value::Integer(i)) => range(*i as f64),
        (ValueType::Boolean, Value::Bool(_)) => Ok(()),
        (ValueType::Enum, Value::Text(t)) => {
            let values = spec.values.as_deref().unwrap_or_default();
            if values.iter().any(|v| v == t) {
                Ok(())
            } else {
                Err(format!(
                    "'{t}' is not one of the declared values {}",
                    values.join(", ")
                ))
            }
        }
        (ValueType::String, Value::Text(t)) => match spec.max_length {
            Some(m) if t.chars().count() as u64 > m => {
                Err(format!("text is longer than the declared {m} characters"))
            }
            _ => Ok(()),
        },
        (ty, v) => Err(format!(
            "a {} where the descriptor declares {}",
            v.kind(),
            match ty {
                ValueType::Number => "a number with a unit",
                ValueType::Integer => "an integer",
                ValueType::Boolean => "a boolean",
                ValueType::Enum => "an enum",
                ValueType::String => "a string",
            }
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmhs_types::Quantity;

    fn spec(json: serde_json::Value) -> ValueSpec {
        serde_json::from_value(json).unwrap()
    }

    fn volts(v: f64) -> Value {
        Value::Quantity(Quantity {
            value: v,
            unit: "V".into(),
        })
    }

    #[test]
    fn steps_stay_in_range_and_under_the_slew_limit() {
        let s = spec(
            json!({"type": "number", "quantity": "electric_potential", "unit": "V", "range": {"min": 0, "max": 12}}),
        );
        let slew = Some(Slew {
            max_change: 2.0,
            per_ms: 1000,
        });
        assert_eq!(step_from(&s, 0.0, slew, None), 0.6);
        assert_eq!(step_from(&s, 11.8, slew, None), 11.2);
        // Slew is canonical (V); a mV-declared value gets the converted allowance.
        let mv = spec(
            json!({"type": "number", "quantity": "electric_potential", "unit": "mV", "range": {"min": 0, "max": 12000}}),
        );
        assert_eq!(step_from(&mv, 0.0, slew, None), 600.0);
        let tight = Some(Slew {
            max_change: 0.2,
            per_ms: 1000,
        });
        assert_eq!(step_from(&s, 0.0, tight, None), 0.1);
    }

    #[test]
    fn steps_exceed_the_verification_tolerance() {
        let s = spec(
            json!({"type": "number", "quantity": "electric_potential", "unit": "V", "range": {"min": 0, "max": 1}}),
        );
        let tol = Some(Tolerance {
            abs: Some(0.1),
            rel: None,
        });
        assert_eq!(step_from(&s, 0.0, None, tol), 0.4);
    }

    #[test]
    fn generated_values_differ_from_the_current_one() {
        let b = spec(json!({"type": "boolean"}));
        assert_eq!(
            next_value(&b, Some(&Value::Bool(true)), None, None).unwrap(),
            json!(false)
        );
        let e = spec(json!({"type": "enum", "values": ["a", "b"]}));
        assert_eq!(
            next_value(&e, Some(&Value::Text("a".into())), None, None).unwrap(),
            json!("b")
        );
        let n = spec(
            json!({"type": "number", "quantity": "electric_potential", "unit": "V", "range": {"min": 0, "max": 12}}),
        );
        assert_eq!(
            next_value(&n, Some(&volts(5.0)), None, None).unwrap(),
            json!({"value": 5.6, "unit": "V"})
        );
        assert_eq!(
            next_value(&n, None, None, None).unwrap(),
            json!({"value": 6.0, "unit": "V"})
        );
    }

    #[test]
    fn readings_are_checked_for_type_unit_and_range() {
        let n = spec(
            json!({"type": "number", "quantity": "electric_potential", "unit": "V", "range": {"min": 0, "max": 12}}),
        );
        assert!(check_reading(&n, &volts(3.0)).is_ok());
        let e = check_reading(
            &n,
            &Value::Quantity(Quantity {
                value: 3000.0,
                unit: "mV".into(),
            }),
        )
        .unwrap_err();
        assert!(e.contains("declared unit 'V'"), "{e}");
        assert!(
            check_reading(&n, &volts(13.0))
                .unwrap_err()
                .contains("0..12 V")
        );
        assert!(
            check_reading(&n, &Value::Integer(3))
                .unwrap_err()
                .contains("a number with a unit")
        );
    }
}
