//! Converting between OpenMHS values and what protocols carry: numbers (registers),
//! JSON (HTTP, MQTT) and text (SCPI).
//!
//! Decoding always goes through the capability's declared [`ValueSpec`], so a reading
//! has the declared type and unit or is an error. Nothing is guessed: a value that does
//! not fit is reported, not coerced.

use openmhs_types::descriptor::{ValueSpec, ValueType};
use openmhs_types::value::{Quantity, Value};

/// The numeric content of a value: quantities, integers, and booleans as 1 or 0.
pub fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Quantity(q) => Some(q.value),
        #[allow(clippy::cast_precision_loss)]
        Value::Integer(i) => Some(*i as f64),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::Text(_) => None,
    }
}

/// A value of `spec`'s type from a number. Enums are indexes into the declared values.
pub fn from_number(spec: &ValueSpec, x: f64) -> Result<Value, String> {
    if !x.is_finite() {
        return Err(format!("{x} is not a finite number"));
    }
    let v =
        match spec.ty {
            ValueType::Number => Value::Quantity(Quantity {
                value: x,
                unit: spec.unit.clone().unwrap_or_default(),
            }),
            ValueType::Integer => Value::Integer(integral(x)?),
            ValueType::Boolean => match x {
                0.0 => Value::Bool(false),
                1.0 => Value::Bool(true),
                _ => return Err(format!("{x} is not a boolean (0 or 1)")),
            },
            ValueType::Enum => {
                let values = spec.values.as_deref().unwrap_or_default();
                let i = usize::try_from(integral(x)?)
                    .map_err(|_| format!("{x} is not an enum index"))?;
                Value::Text(values.get(i).cloned().ok_or_else(|| {
                    format!("enum index {i} is out of range (0..{})", values.len())
                })?)
            }
            ValueType::String => return Err("a string cannot be carried as a number".into()),
        };
    check(spec, &v)?;
    Ok(v)
}

/// The number that carries `v` for `spec`; the inverse of [`from_number`].
pub fn to_number(spec: &ValueSpec, v: &Value) -> Result<f64, String> {
    match (spec.ty, v) {
        (ValueType::Enum, Value::Text(t)) => spec
            .values
            .as_deref()
            .unwrap_or_default()
            .iter()
            .position(|x| x == t)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let f = i as f64;
                f
            })
            .ok_or_else(|| format!("'{t}' is not a declared value")),
        _ => number(v).ok_or_else(|| format!("a {} cannot be carried as a number", v.kind())),
    }
}

#[allow(clippy::cast_possible_truncation)]
fn integral(x: f64) -> Result<i64, String> {
    if x.fract() == 0.0 && x.abs() < 9.0e15 {
        Ok(x as i64)
    } else {
        Err(format!("{x} is not an integer"))
    }
}

/// `v` as plain JSON: quantities as their number, the rest as themselves.
pub fn to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Quantity(q) => serde_json::json!(q.value),
        Value::Integer(i) => serde_json::json!(i),
        Value::Bool(b) => serde_json::json!(b),
        Value::Text(t) => serde_json::json!(t),
    }
}

/// A value of `spec`'s type from JSON a device sent. Strings are parsed as text, since
/// many devices send numbers as strings.
pub fn from_json(spec: &ValueSpec, j: &serde_json::Value) -> Result<Value, String> {
    match j {
        serde_json::Value::Number(n) => {
            let x = n
                .as_f64()
                .ok_or_else(|| format!("{n} is not a usable number"))?;
            if spec.ty == ValueType::Enum {
                return Err(format!("{n} is not one of the declared values"));
            }
            from_number(spec, x)
        }
        serde_json::Value::Bool(b) if spec.ty == ValueType::Boolean => Ok(Value::Bool(*b)),
        serde_json::Value::String(s) => from_text(spec, s),
        other => Err(format!("{other} is not a {}", type_name(spec.ty))),
    }
}

/// `v` as text, e.g. for SCPI: numbers in plain decimal, booleans as 1 or 0.
pub fn to_text(v: &Value) -> String {
    match v {
        Value::Quantity(q) => q.value.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Bool(b) => if *b { "1" } else { "0" }.to_owned(),
        Value::Text(t) => t.clone(),
    }
}

/// A value of `spec`'s type from text. Numbers may use exponents (`+5.000E+00`);
/// booleans may be `1`/`0`, `ON`/`OFF` or `true`/`false`; enums match case-insensitively.
pub fn from_text(spec: &ValueSpec, s: &str) -> Result<Value, String> {
    let t = s.trim();
    let v = match spec.ty {
        ValueType::Number | ValueType::Integer => {
            let x: f64 = t.parse().map_err(|_| format!("'{t}' is not a number"))?;
            return from_number(spec, x);
        }
        ValueType::Boolean => match t.to_ascii_lowercase().as_str() {
            "1" | "on" | "true" => Value::Bool(true),
            "0" | "off" | "false" => Value::Bool(false),
            _ => return Err(format!("'{t}' is not a boolean")),
        },
        ValueType::Enum => Value::Text(
            spec.values
                .as_deref()
                .unwrap_or_default()
                .iter()
                .find(|v| v.eq_ignore_ascii_case(t))
                .cloned()
                .ok_or_else(|| format!("'{t}' is not one of the declared values"))?,
        ),
        ValueType::String => Value::Text(s.to_owned()),
    };
    check(spec, &v)?;
    Ok(v)
}

fn type_name(t: ValueType) -> &'static str {
    match t {
        ValueType::Number => "number",
        ValueType::Integer => "integer",
        ValueType::Boolean => "boolean",
        ValueType::Enum => "enum value",
        ValueType::String => "string",
    }
}

/// Whether `v` has `spec`'s type and unit and lies within its range or values.
pub fn check(spec: &ValueSpec, v: &Value) -> Result<(), String> {
    let x = match (spec.ty, v) {
        (ValueType::Number, Value::Quantity(q)) => {
            let unit = spec.unit.as_deref().unwrap_or_default();
            if q.unit != unit {
                return Err(format!("unit '{}' is not the declared '{unit}'", q.unit));
            }
            Some(q.value)
        }
        (ValueType::Integer, Value::Integer(_)) => number(v),
        (ValueType::Boolean, Value::Bool(_)) => None,
        (ValueType::Enum, Value::Text(t)) => {
            if !spec.values.as_deref().unwrap_or_default().contains(t) {
                return Err(format!("'{t}' is not one of the declared values"));
            }
            None
        }
        (ValueType::String, Value::Text(t)) => {
            if let Some(max) = spec.max_length
                && t.chars().count() > usize::try_from(max).unwrap_or(usize::MAX)
            {
                return Err(format!("longer than {max} characters"));
            }
            None
        }
        _ => return Err(format!("a {} is not a {}", v.kind(), type_name(spec.ty))),
    };
    if let (Some(x), Some(r)) = (x, spec.range)
        && (r.min.is_some_and(|m| x < m) || r.max.is_some_and(|m| x > m))
    {
        let bound = |b: Option<f64>| b.map_or_else(String::new, |b| b.to_string());
        return Err(format!("{x} is outside {}..{}", bound(r.min), bound(r.max)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(json: serde_json::Value) -> ValueSpec {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn numbers_keep_the_declared_unit_and_range() {
        let s = spec(
            serde_json::json!({"type": "number", "quantity": "voltage", "unit": "V", "range": {"min": 0, "max": 30}}),
        );
        assert_eq!(
            from_text(&s, "+5.000E+00").unwrap(),
            Value::Quantity(Quantity {
                value: 5.0,
                unit: "V".into()
            })
        );
        assert!(from_text(&s, "31").unwrap_err().contains("outside"));
        assert!(from_json(&s, &serde_json::json!("x")).is_err());
        assert!(
            check(
                &s,
                &Value::Quantity(Quantity {
                    value: 1.0,
                    unit: "mV".into()
                })
            )
            .is_err()
        );
    }

    #[test]
    fn booleans_enums_and_integers() {
        let b = spec(serde_json::json!({"type": "boolean"}));
        assert_eq!(from_text(&b, "ON").unwrap(), Value::Bool(true));
        assert_eq!(from_number(&b, 0.0).unwrap(), Value::Bool(false));
        assert!(from_number(&b, 2.0).is_err());
        let e = spec(serde_json::json!({"type": "enum", "values": ["idle", "run"]}));
        assert_eq!(from_text(&e, "RUN").unwrap(), Value::Text("run".into()));
        assert_eq!(from_number(&e, 1.0).unwrap(), Value::Text("run".into()));
        assert_eq!(to_number(&e, &Value::Text("run".into())).unwrap(), 1.0);
        assert!(from_number(&e, 2.0).is_err());
        let i = spec(serde_json::json!({"type": "integer", "range": {"min": 0}}));
        assert_eq!(
            from_json(&i, &serde_json::json!(3)).unwrap(),
            Value::Integer(3)
        );
        assert!(from_number(&i, 1.5).is_err());
        assert!(from_number(&i, -1.0).is_err());
    }
}
