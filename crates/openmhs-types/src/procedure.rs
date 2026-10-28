//! Procedures: stored, versioned, parameterized plans (spec/procedures.md).
//!
//! A procedure is a plan template with typed parameters. Running one fills the template
//! and submits the result as an ordinary plan, as the caller, so every step is the caller's
//! command with every check: a procedure grants nothing. Agents may propose procedures;
//! only an operator approves one into use. Expansion here is pure: the same procedure and
//! parameters always give the same plan.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::error::ErrorObject;
use crate::plan::PlanSpec;
use crate::time::Timestamp;

/// What a parameter accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamType {
    /// `{"value": <number>, "unit": "<unit>"}` in exactly the declared unit.
    Quantity,
    Integer,
    Boolean,
    String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamSpec {
    #[serde(rename = "type")]
    pub ty: ParamType,
    /// Required for a quantity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Used when the caller gives none. Without one the parameter is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Json>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// What an author writes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcedureSpec {
    /// 1-64 letters, digits, `-`, `_` or `.`.
    pub name: String,
    /// Starts at 1. A changed procedure is a new version; versions are never edited.
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, ParamSpec>,
    /// A plan (spec/plans.md) in which `{"$param": "<name>"}` stands for a parameter.
    pub plan: Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureStatus {
    /// Proposed, by an agent or a person; cannot run until approved.
    Proposed,
    Approved,
}

/// A stored procedure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Procedure {
    #[serde(flatten)]
    pub spec: ProcedureSpec,
    pub status: ProcedureStatus,
    /// SHA-256 of the spec's canonical JSON: what was approved, and what a plan ran.
    pub sha256: String,
    pub proposed_by: String,
    pub proposed_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<Timestamp>,
}

fn err(code: &str, msg: impl Into<String>) -> ErrorObject {
    ErrorObject::new(code, msg)
}

fn valid_name(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The JSON a spec is hashed as: keys sorted (serde_json's map is ordered), no whitespace.
pub fn canonical(spec: &ProcedureSpec) -> String {
    let v = serde_json::to_value(spec).unwrap_or(Json::Null);
    serde_json::to_string(&v).unwrap_or_default()
}

/// Check a spec before it is stored: a valid name and version, well-formed parameters, every
/// placeholder naming a parameter, every parameter used, and a template that expands (with
/// each parameter's default or a sample value) into a plan.
pub fn validate(spec: &ProcedureSpec) -> Result<(), Vec<ErrorObject>> {
    let mut errors = Vec::new();
    if !valid_name(&spec.name) {
        errors.push(err(
            "procedure.name",
            "a procedure name is 1-64 letters, digits, '-', '_' or '.'",
        ));
    }
    if spec.version == 0 {
        errors.push(err("procedure.version", "versions start at 1"));
    }
    for (name, p) in &spec.params {
        if !valid_name(name) {
            errors.push(err(
                "procedure.param",
                format!("parameter name '{name}' is not valid"),
            ));
        }
        if p.ty == ParamType::Quantity && p.unit.as_deref().is_none_or(str::is_empty) {
            errors.push(err(
                "procedure.param",
                format!("quantity parameter '{name}' needs a unit"),
            ));
        }
        if let (Some(lo), Some(hi)) = (p.min, p.max)
            && lo > hi
        {
            errors.push(err(
                "procedure.param",
                format!("parameter '{name}' has min above max"),
            ));
        }
        if let Some(d) = &p.default
            && let Err(e) = check_value(name, p, d)
        {
            errors.push(e);
        }
    }
    let mut used = Vec::new();
    placeholders(&spec.plan, &mut used);
    for u in &used {
        if !spec.params.contains_key(u) {
            errors.push(err(
                "procedure.param",
                format!("the plan uses '{u}', which is not a declared parameter"),
            ));
        }
    }
    for name in spec.params.keys() {
        if !used.contains(name) {
            errors.push(err(
                "procedure.param",
                format!("parameter '{name}' is declared but never used"),
            ));
        }
    }
    if errors.is_empty() {
        let sample: BTreeMap<String, Json> = spec
            .params
            .iter()
            .map(|(n, p)| (n.clone(), p.default.clone().unwrap_or_else(|| sample(p))))
            .collect();
        if let Err(mut e) = expand(spec, "", &sample) {
            errors.append(&mut e);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn sample(p: &ParamSpec) -> Json {
    let n = p.min.or(p.max).unwrap_or(0.0);
    match p.ty {
        ParamType::Quantity => serde_json::json!({ "value": n, "unit": p.unit }),
        ParamType::Integer => serde_json::json!(n as i64),
        ParamType::Boolean => Json::Bool(false),
        ParamType::String => Json::String(String::new()),
    }
}

fn placeholders(v: &Json, out: &mut Vec<String>) {
    match v {
        Json::Object(m) => {
            if let (1, Some(Json::String(name))) = (m.len(), m.get("$param")) {
                if !out.contains(name) {
                    out.push(name.clone());
                }
            } else {
                m.values().for_each(|x| placeholders(x, out));
            }
        }
        Json::Array(items) => items.iter().for_each(|x| placeholders(x, out)),
        _ => {}
    }
}

fn check_value(name: &str, p: &ParamSpec, v: &Json) -> Result<(), ErrorObject> {
    let bad = |why: String| {
        Err(err(
            "procedure.param.invalid",
            format!("parameter '{name}': {why}"),
        )
        .detail("param", name))
    };
    let number = match (p.ty, v) {
        (ParamType::Quantity, Json::Object(m)) => {
            let unit = m.get("unit").and_then(Json::as_str);
            if unit != p.unit.as_deref() {
                return bad(format!(
                    "expects {} (got {})",
                    p.unit.as_deref().unwrap_or("?"),
                    unit.unwrap_or("no unit")
                ));
            }
            match m.get("value").and_then(Json::as_f64) {
                Some(n) if m.len() == 2 => Some(n),
                _ => return bad("a quantity is {\"value\": <number>, \"unit\": \"...\"}".into()),
            }
        }
        (ParamType::Quantity, _) => {
            return bad(format!(
                "expects a quantity in {}",
                p.unit.as_deref().unwrap_or("?")
            ));
        }
        (ParamType::Integer, Json::Number(n)) if n.is_i64() => n.as_f64(),
        (ParamType::Integer, _) => return bad("expects an integer".into()),
        (ParamType::Boolean, Json::Bool(_)) | (ParamType::String, Json::String(_)) => None,
        (ParamType::Boolean, _) => return bad("expects true or false".into()),
        (ParamType::String, _) => return bad("expects a string".into()),
    };
    if let Some(n) = number
        && (p.min.is_some_and(|lo| n < lo) || p.max.is_some_and(|hi| n > hi))
    {
        return bad(format!(
            "{n} is outside {}..{}",
            p.min.map_or("-inf".into(), |x| x.to_string()),
            p.max.map_or("inf".into(), |x| x.to_string())
        ));
    }
    Ok(())
}

fn substitute(v: &Json, values: &BTreeMap<String, Json>) -> Json {
    match v {
        Json::Object(m) => {
            if let (1, Some(Json::String(name))) = (m.len(), m.get("$param")) {
                return values.get(name).cloned().unwrap_or(Json::Null);
            }
            Json::Object(
                m.iter()
                    .map(|(k, x)| (k.clone(), substitute(x, values)))
                    .collect(),
            )
        }
        Json::Array(items) => Json::Array(items.iter().map(|x| substitute(x, values)).collect()),
        other => other.clone(),
    }
}

/// Fill `spec`'s template with `given` (defaults for the rest) and return the plan to
/// submit. Unknown or missing parameters, a wrong type or unit, or a value outside the
/// declared range is refused before anything is created. The plan's description names the
/// procedure, its version and `sha256`, so every step's commands trace back to it.
pub fn expand(
    spec: &ProcedureSpec,
    sha256: &str,
    given: &BTreeMap<String, Json>,
) -> Result<PlanSpec, Vec<ErrorObject>> {
    let mut errors = Vec::new();
    for name in given.keys() {
        if !spec.params.contains_key(name) {
            errors.push(
                err(
                    "procedure.param.unknown",
                    format!(
                        "'{}' v{} has no parameter '{name}'",
                        spec.name, spec.version
                    ),
                )
                .detail("param", name.as_str()),
            );
        }
    }
    let mut values = BTreeMap::new();
    for (name, p) in &spec.params {
        match given.get(name).or(p.default.as_ref()) {
            Some(v) => match check_value(name, p, v) {
                Ok(()) => {
                    values.insert(name.clone(), v.clone());
                }
                Err(e) => errors.push(e),
            },
            None => errors.push(
                err(
                    "procedure.param.missing",
                    format!("parameter '{name}' is required"),
                )
                .detail("param", name.as_str()),
            ),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut plan: PlanSpec =
        serde_json::from_value(substitute(&spec.plan, &values)).map_err(|e| {
            vec![err(
                "procedure.plan",
                format!("the template is not a plan: {e}"),
            )]
        })?;
    let origin = format!(
        "procedure {} v{}{}",
        spec.name,
        spec.version,
        if sha256.is_empty() {
            String::new()
        } else {
            format!(" (sha256 {})", &sha256[..sha256.len().min(16)])
        }
    );
    plan.description = Some(match (&plan.description, &spec.description) {
        (Some(d), _) | (None, Some(d)) => format!("{origin}: {d}"),
        (None, None) => origin,
    });
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ramp() -> ProcedureSpec {
        serde_json::from_value(json!({
            "name": "psu.set-output",
            "version": 1,
            "description": "set the supply and switch it on",
            "params": {
                "voltage": { "type": "quantity", "unit": "V", "min": 0, "max": 12 },
                "enable": { "type": "boolean", "default": true }
            },
            "plan": { "steps": [
                { "id": "set", "request": {
                    "target": { "device": "sim.psu-01", "capability": "power.output.voltage.setpoint" },
                    "operation": "write", "value": { "$param": "voltage" } } },
                { "id": "on", "after": ["set"], "request": {
                    "target": { "device": "sim.psu-01", "capability": "power.output.enabled" },
                    "operation": "write", "value": { "$param": "enable" } } }
            ] }
        }))
        .unwrap()
    }

    fn params(v: Json) -> BTreeMap<String, Json> {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_valid_procedure_expands_deterministically_and_names_its_origin() {
        let spec = ramp();
        validate(&spec).unwrap();
        let given = params(json!({ "voltage": { "value": 2.0, "unit": "V" } }));
        let a = expand(&spec, "ab".repeat(32).as_str(), &given).unwrap();
        let b = expand(&spec, "ab".repeat(32).as_str(), &given).unwrap();
        assert_eq!(a, b);
        let text = serde_json::to_string(&a).unwrap();
        assert!(
            text.contains("\"value\":2.0") && text.contains("\"value\":true"),
            "{text}"
        );
        assert!(
            a.description
                .unwrap()
                .starts_with("procedure psu.set-output v1 (sha256 abababababababab)")
        );
        assert_eq!(canonical(&spec), canonical(&ramp()));
    }

    #[test]
    fn parameters_are_checked_before_anything_is_built() {
        let spec = ramp();
        let codes = |v: Json| -> Vec<String> {
            expand(&spec, "", &params(v))
                .unwrap_err()
                .into_iter()
                .map(|e| e.code)
                .collect()
        };
        assert_eq!(codes(json!({})), ["procedure.param.missing"]);
        assert_eq!(
            codes(json!({ "voltage": { "value": 2, "unit": "mV" } })),
            ["procedure.param.invalid"]
        );
        assert_eq!(
            codes(json!({ "voltage": { "value": 48, "unit": "V" } })),
            ["procedure.param.invalid"]
        );
        assert_eq!(
            codes(json!({ "voltage": { "value": 1, "unit": "V" }, "extra": 1 })),
            ["procedure.param.unknown"]
        );
        assert_eq!(
            codes(json!({ "voltage": { "value": 1, "unit": "V" }, "enable": "yes" })),
            ["procedure.param.invalid"]
        );
    }

    #[test]
    fn malformed_procedures_are_refused_with_every_problem_named() {
        let mut spec = ramp();
        spec.name = "bad name".into();
        spec.version = 0;
        spec.params.insert(
            "unused".into(),
            serde_json::from_value(json!({ "type": "integer" })).unwrap(),
        );
        spec.params.insert(
            "level".into(),
            serde_json::from_value(json!({ "type": "quantity" })).unwrap(),
        );
        let errors = validate(&spec).unwrap_err();
        let text = format!("{errors:?}");
        for needle in [
            "procedure.name",
            "procedure.version",
            "never used",
            "needs a unit",
        ] {
            assert!(text.contains(needle), "{needle}: {text}");
        }

        let mut undeclared = ramp();
        undeclared.params.remove("enable");
        let text = format!("{:?}", validate(&undeclared).unwrap_err());
        assert!(text.contains("not a declared parameter"), "{text}");

        let mut not_a_plan = ramp();
        not_a_plan.plan = json!({ "steps": [{ "id": "x" }] });
        not_a_plan.params.clear();
        let text = format!("{:?}", validate(&not_a_plan).unwrap_err());
        assert!(text.contains("procedure.plan"), "{text}");
    }
}
