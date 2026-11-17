//! The measured values behind a verdict: `outcome.verified.conditions` (RFC 0005, ADR-0035).
//!
//! A producer that ran the RFC 0004 verifier copies each condition of the contract, as judged, onto the
//! `outcome.verified` event: the bounds, the result and the values the verifier used. With them a store can say by how
//! much a goal missed, not only that it did. Nothing here knows a domain: a distance is computed for numeric
//! comparisons only, and only between values in the same unit.

use serde_json::Value;

use crate::error::RecordError;

/// RFC 0004 condition operators, in the spelling of the contract.
pub const OPS: &[&str] = &["eq", "ne", "lt", "le", "gt", "ge", "between", "in"];
/// A condition's result, in lower case as every outcome value in an event.
pub const RESULTS: &[&str] = &["true", "false", "undetermined"];

/// One condition of a contract, as the verifier judged it.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifiedCondition {
    pub name: Option<String>,
    pub evidence: String,
    pub op: String,
    pub result: String,
    pub min: Option<Value>,
    pub max: Option<Value>,
    pub value: Option<Value>,
    pub values: Option<Vec<Value>>,
    /// The values of the records the verifier used, in its order.
    pub observed: Vec<Value>,
}

/// Which side of its bound a value lies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    Above,
    Below,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Above => "above",
            Self::Below => "below",
        }
    }
}

/// How far a failed condition's values lie outside its bound.
#[derive(Debug, Clone, PartialEq)]
pub struct Miss {
    /// The smallest distance outside the bound over the observed values: the goal missed by at least this much.
    pub at_least: f64,
    /// `None` when the observed values lie on both sides (possible for `between`).
    pub side: Option<Side>,
    pub unit: Option<String>,
}

/// A number, or `{"value": <number>, "unit": "<unit>"}`: its numeric part and unit. Anything else is not a quantity.
pub fn quantity(v: &Value) -> Option<(f64, Option<String>)> {
    match v {
        Value::Number(n) => n.as_f64().map(|x| (x, None)),
        Value::Object(o) => {
            let x = o.get("value")?.as_f64()?;
            let unit = match o.get("unit") {
                None | Some(Value::Null) => None,
                Some(Value::String(u)) => Some(u.clone()),
                Some(_) => return None,
            };
            Some((x, unit))
        }
        _ => None,
    }
}

impl VerifiedCondition {
    /// Distance outside the bound of one observed value, and its side; `None` when the value is inside, is not a
    /// quantity, is in another unit than the bound, or the operator has no distance (`eq`, `ne`, `in`).
    pub fn outside(&self, observed: &Value) -> Option<(f64, Side, Option<String>)> {
        let (x, unit) = quantity(observed)?;
        let bound = |b: &Option<Value>| -> Option<f64> {
            let (v, u) = quantity(b.as_ref()?)?;
            (u == unit).then_some(v)
        };
        let (distance, side) = match self.op.as_str() {
            "between" => {
                let (lo, hi) = (bound(&self.min)?, bound(&self.max)?);
                if x < lo {
                    (lo - x, Side::Below)
                } else if x > hi {
                    (x - hi, Side::Above)
                } else {
                    return None;
                }
            }
            "lt" | "le" => {
                let b = bound(&self.value)?;
                let outside = if self.op == "lt" { x >= b } else { x > b };
                if !outside {
                    return None;
                }
                (x - b, Side::Above)
            }
            "gt" | "ge" => {
                let b = bound(&self.value)?;
                let outside = if self.op == "gt" { x <= b } else { x < b };
                if !outside {
                    return None;
                }
                (b - x, Side::Below)
            }
            _ => return None,
        };
        Some((distance, side, unit))
    }

    /// For a failed numeric condition: how far its values lie outside the bound. `None` unless every observed value
    /// is outside in one unit, which is what a `false` result means (RFC 0004: every record fails).
    pub fn miss(&self) -> Option<Miss> {
        if self.result != "false" || self.observed.is_empty() {
            return None;
        }
        let mut found: Option<Miss> = None;
        for o in &self.observed {
            let (d, side, unit) = self.outside(o)?;
            match &mut found {
                None => {
                    found = Some(Miss {
                        at_least: d,
                        side: Some(side),
                        unit,
                    })
                }
                Some(m) => {
                    if m.unit != unit {
                        return None;
                    }
                    if m.side != Some(side) {
                        m.side = None;
                    }
                    m.at_least = m.at_least.min(d);
                }
            }
        }
        found
    }

    /// The name to show: the contract's, else `<evidence> <op>`.
    pub fn label(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("{} {}", self.evidence, self.op))
    }
}

fn operand_form(v: &Value) -> bool {
    match v {
        Value::Number(_) | Value::String(_) | Value::Bool(_) => true,
        Value::Object(o) => {
            o.get("value")
                .is_some_and(|x| matches!(x, Value::Number(_) | Value::String(_) | Value::Bool(_)))
                && o.get("unit")
                    .is_none_or(|u| matches!(u, Value::String(_) | Value::Null))
        }
        _ => false,
    }
}

fn bad(message: impl Into<String>) -> RecordError {
    RecordError::new("event.field", message)
}

/// Read `outcome.verified.conditions` from an event. Absent is an empty list; a malformed list is refused, and so is a
/// list that contradicts the verdict (RFC 0004: any `false` is `violated`; otherwise any `undetermined` is `unknown`;
/// all `true` is `satisfied`).
pub fn parse_conditions(event: &Value) -> Result<Vec<VerifiedCondition>, RecordError> {
    let Some(list) = event
        .get("outcome")
        .and_then(|o| o.get("verified"))
        .and_then(|v| v.get("conditions"))
        .filter(|c| !c.is_null())
    else {
        return Ok(Vec::new());
    };
    let items = list
        .as_array()
        .ok_or_else(|| bad("`outcome.verified.conditions` must be an array"))?;
    if items.is_empty() {
        return Err(bad(
            "`outcome.verified.conditions` is empty; leave it out instead",
        ));
    }
    let mut out = Vec::with_capacity(items.len());
    for (i, c) in items.iter().enumerate() {
        let at = |k: &str| format!("`outcome.verified.conditions[{i}].{k}`");
        if !c.is_object() {
            return Err(bad(format!(
                "`outcome.verified.conditions[{i}]` must be an object"
            )));
        }
        let string = |k: &str| -> Result<Option<String>, RecordError> {
            match c.get(k) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(s)) if !s.is_empty() => Ok(Some(s.clone())),
                Some(_) => Err(bad(format!("{} must be a non-empty string", at(k)))),
            }
        };
        let evidence =
            string("evidence")?.ok_or_else(|| bad(format!("{} is required", at("evidence"))))?;
        let op = string("op")?.ok_or_else(|| bad(format!("{} is required", at("op"))))?;
        if !OPS.contains(&op.as_str()) {
            return Err(RecordError::new(
                "event.value",
                format!("{} is {op:?}; expected one of {}", at("op"), OPS.join(", ")),
            ));
        }
        let result =
            string("result")?.ok_or_else(|| bad(format!("{} is required", at("result"))))?;
        if !RESULTS.contains(&result.as_str()) {
            return Err(RecordError::new(
                "event.value",
                format!(
                    "{} is {result:?}; expected one of {}",
                    at("result"),
                    RESULTS.join(", ")
                ),
            ));
        }
        let operand = |k: &str| -> Result<Option<Value>, RecordError> {
            match c.get(k) {
                None | Some(Value::Null) => Ok(None),
                Some(v) if operand_form(v) => Ok(Some(v.clone())),
                Some(_) => Err(bad(format!(
                    "{} must be a number, string, boolean or {{\"value\", \"unit\"}}",
                    at(k)
                ))),
            }
        };
        let (min, max, value) = (operand("min")?, operand("max")?, operand("value")?);
        let values = match c.get("values") {
            None | Some(Value::Null) => None,
            Some(Value::Array(a)) if !a.is_empty() && a.iter().all(operand_form) => Some(a.clone()),
            Some(_) => {
                return Err(bad(format!(
                    "{} must be a non-empty array of operands",
                    at("values")
                )));
            }
        };
        let complete = match op.as_str() {
            "between" => min.is_some() && max.is_some(),
            "in" => values.is_some(),
            _ => value.is_some(),
        };
        if !complete {
            return Err(bad(format!(
                "`outcome.verified.conditions[{i}]`: `{op}` is missing its bound"
            )));
        }
        let observed = match c.get("observed") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(a)) if a.iter().all(operand_form) => a.clone(),
            Some(_) => {
                return Err(bad(format!(
                    "{} must be an array of values",
                    at("observed")
                )));
            }
        };
        out.push(VerifiedCondition {
            name: string("name")?,
            evidence,
            op,
            result,
            min,
            max,
            value,
            values,
            observed,
        });
    }
    let verdict = event
        .get("outcome")
        .and_then(|o| o.get("verified"))
        .and_then(|v| v.get("value"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let implied = if out.iter().any(|c| c.result == "false") {
        "violated"
    } else if out.iter().any(|c| c.result == "undetermined") {
        "unknown"
    } else {
        "satisfied"
    };
    if verdict != implied {
        return Err(RecordError::new(
            "event.value",
            format!(
                "`outcome.verified.value` is {verdict:?} but its conditions make it {implied:?} (RFC 0004)"
            ),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    fn event(verdict: &str, conditions: Value) -> Value {
        json!({"outcome": {"verified": {"value": verdict, "contract": "c", "conditions": conditions}}})
    }

    fn m(x: f64) -> Value {
        json!({"value": x, "unit": "m"})
    }

    #[test]
    fn a_between_miss_is_the_closest_record_and_its_side() {
        let e = event(
            "violated",
            json!([{"name": "x within 0.20 m", "evidence": "x", "op": "between", "min": m(4.8), "max": m(5.2),
                    "result": "false", "observed": [m(5.55), m(5.5), m(5.6)]}]),
        );
        let c = parse_conditions(&e).unwrap();
        let miss = c[0].miss().unwrap();
        assert!((miss.at_least - 0.3).abs() < 1e-9, "{miss:?}");
        assert_eq!(miss.side, Some(Side::Above));
        assert_eq!(miss.unit.as_deref(), Some("m"));
        assert_eq!(c[0].label(), "x within 0.20 m");
    }

    #[test]
    fn records_on_both_sides_have_no_side() {
        let e = event(
            "violated",
            json!([{"evidence": "x", "op": "between", "min": m(1.0), "max": m(2.0), "result": "false",
                    "observed": [m(0.5), m(2.25)]}]),
        );
        let miss = parse_conditions(&e).unwrap()[0].miss().unwrap();
        assert_eq!(miss.side, None);
        assert!((miss.at_least - 0.25).abs() < 1e-9);
    }

    #[test]
    fn an_upper_bound_misses_above() {
        let e = event(
            "violated",
            json!([{"evidence": "z", "op": "le", "value": m(0.15), "result": "false", "observed": [m(0.4)]}]),
        );
        let miss = parse_conditions(&e).unwrap()[0].miss().unwrap();
        assert_eq!(miss.side, Some(Side::Above));
        assert!((miss.at_least - 0.25).abs() < 1e-9);
    }

    #[test]
    fn units_are_never_mixed_and_categories_have_no_distance() {
        let e = event(
            "violated",
            json!([{"evidence": "z", "op": "le", "value": m(0.15), "result": "false",
                    "observed": [{"value": 40, "unit": "cm"}]},
                   {"evidence": "state", "op": "eq", "value": "closed", "result": "false", "observed": ["open"]}]),
        );
        let c = parse_conditions(&e).unwrap();
        assert_eq!(c[0].miss(), None, "metres against centimetres");
        assert_eq!(c[1].miss(), None, "eq has no distance");
    }

    #[test]
    fn a_condition_that_held_has_no_miss() {
        let e = event(
            "satisfied",
            json!([{"evidence": "z", "op": "le", "value": m(0.15), "result": "true", "observed": [m(0.1)]}]),
        );
        assert_eq!(parse_conditions(&e).unwrap()[0].miss(), None);
    }

    #[test]
    fn absent_conditions_are_an_empty_list() {
        assert!(
            parse_conditions(&json!({"outcome": {"verified": {"value": "violated"}}}))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn malformed_or_contradicting_conditions_are_refused() {
        let ok = json!({"evidence": "z", "op": "le", "value": m(0.15), "result": "false", "observed": [m(0.4)]});
        let with = |k: &str, v: Value| {
            let mut c = ok.clone();
            c[k] = v;
            event("violated", json!([c]))
        };
        let cases = vec![
            ("event.field", event("violated", json!({}))),
            ("event.field", event("violated", json!([]))),
            ("event.field", event("violated", json!(["z"]))),
            ("event.value", with("op", json!("approximately"))),
            ("event.value", with("result", json!("FALSE"))),
            ("event.field", with("value", Value::Null)),
            ("event.field", with("evidence", json!(""))),
            ("event.field", with("observed", json!("0.4"))),
            ("event.field", with("observed", json!([[0.4]]))),
            ("event.field", with("value", json!({"unit": "m"}))),
            // the verdict and its conditions disagree
            ("event.value", event("satisfied", json!([ok.clone()]))),
            (
                "event.value",
                event(
                    "violated",
                    json!([{"evidence": "z", "op": "le", "value": m(0.15), "result": "true"}]),
                ),
            ),
            (
                "event.value",
                event(
                    "violated",
                    json!([{"evidence": "z", "op": "le", "value": m(0.15), "result": "undetermined"}]),
                ),
            ),
        ];
        for (code, e) in cases {
            match parse_conditions(&e) {
                Err(err) => assert_eq!(err.code, code, "{err} for {e}"),
                Ok(c) => panic!("accepted {e}: {c:?}"),
            }
        }
    }
}
