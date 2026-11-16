//! Outcome contracts, evidence records and verdicts ([RFC 0004](../../../docs/rfcs/0004-outcome-contracts-and-evidence.md)).
//!
//! An outcome contract names the evidence a declared physical outcome needs and the conditions it must
//! meet. [`verify`] checks a set of evidence records against it and answers `SATISFIED`, `VIOLATED` or
//! `UNKNOWN`, with the reasons. It is a pure function: no I/O, no clock, no randomness, so the same inputs
//! always give the same report. It never sends anything to a device.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub mod audit;

pub const VERIFIER: &str = concat!("openmhs-outcome ", env!("CARGO_PKG_VERSION"));

// ---------------------------------------------------------------- evidence records

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// A report of what a system did, such as an action result or a command's state.
    Controller,
    /// The device's own reading of its state.
    DeviceState,
    /// A measurement by something other than the device acted on. The producer's word.
    IndependentSensor,
    Operator,
    Calibration,
    Configuration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    Device,
    Simulator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Quality {
    Good,
    Uncertain,
    Bad,
}

fn good() -> Quality {
    Quality::Good
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    pub kind: SourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimeInfo {
    /// When it was measured, on `clock_domain`'s clock (RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// When it was received, kept for the record; the rules use `source`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_domain: Option<String>,
    /// How far that clock may be from true. Missing means unknown, not zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncertainty_ms: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelationKind {
    #[serde(rename = "used")]
    Used,
    #[serde(rename = "wasGeneratedBy")]
    WasGeneratedBy,
    #[serde(rename = "wasDerivedFrom")]
    WasDerivedFrom,
    #[serde(rename = "wasInformedBy")]
    WasInformedBy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relation {
    #[serde(rename = "type")]
    pub kind: RelationKind,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalibrationInfo {
    pub name: String,
    pub applies_to: String,
    pub valid_from: String,
    pub valid_until: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub id: String,
    pub signal: String,
    pub value: Value,
    pub source: SourceInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<TimeInfo>,
    #[serde(default = "good")]
    pub quality: Quality,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub refs: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<Relation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration: Option<CalibrationInfo>,
    /// The coordinate frame the value is expressed in (a ROS `header.frame_id`), when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
}

// ---------------------------------------------------------------- contracts

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Between,
    In,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timing {
    /// The evidence item whose time defines every window.
    pub anchor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_window_ms: Option<[i64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_clock_uncertainty_ms: Option<f64>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub signal: String,
    pub kind: SourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Milliseconds relative to the anchor's time, inclusive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_ms: Option<[i64; 2]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calibration: Vec<String>,
    /// Whether records must carry the contract's `binding`. A device's own readings usually do not
    /// know which attempt they belong to; their window and device identify them instead.
    #[serde(default = "yes")]
    pub bound: bool,
    /// The coordinate frame records must be expressed in. A record in another frame, or in none, is not
    /// evidence for this item: nothing is transformed on the verifier's behalf.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
    /// For a sensor that must show some variation (a motion-capture pose, a noisy measurement): the longest, in
    /// milliseconds, that the same value may be repeated. A record whose value has been identical for longer is
    /// stale, however fresh its timestamp (`evidence.stale`). Opt-in: a setpoint or a settled simulator
    /// legitimately repeats itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_unchanged_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub evidence: String,
    pub op: Op,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    pub contract: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub binding: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<Timing>,
    pub evidence: BTreeMap<String, Item>,
    pub conditions: Vec<Condition>,
}

/// A contract or evidence set that cannot be checked at all. Not a verdict.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum InputError {
    #[error("contract: {0}")]
    Contract(String),
    #[error("evidence: {0}")]
    Evidence(String),
}

impl Contract {
    pub fn check(&self) -> Result<(), InputError> {
        let bad = |m: String| Err(InputError::Contract(m));
        if self.evidence.is_empty() {
            return bad("names no evidence".into());
        }
        if self.conditions.is_empty() {
            return bad("has no conditions, so nothing could be established".into());
        }
        if let Some(t) = &self.timing {
            if !self.evidence.contains_key(&t.anchor) {
                return bad(format!(
                    "timing.anchor '{}' is not an evidence item",
                    t.anchor
                ));
            }
            if t.max_clock_uncertainty_ms
                .is_some_and(|u| !(u.is_finite() && u >= 0.0))
            {
                return bad("timing.max_clock_uncertainty_ms must be a non-negative number".into());
            }
        }
        let windows = self
            .timing
            .iter()
            .filter_map(|t| t.default_window_ms)
            .chain(self.evidence.values().filter_map(|i| i.window_ms));
        for [lo, hi] in windows {
            if lo > hi {
                return bad(format!("window [{lo}, {hi}] is empty"));
            }
        }
        for (name, item) in &self.evidence {
            if item.max_unchanged_ms == Some(0) {
                return bad(format!(
                    "evidence '{name}' has max_unchanged_ms 0, which would reject every record"
                ));
            }
            if item.window_ms.is_some() && self.timing.is_none() {
                return bad(format!(
                    "evidence '{name}' has a window but the contract has no timing.anchor"
                ));
            }
            if item.kind == SourceKind::Calibration {
                return bad(format!(
                    "evidence '{name}': calibration records are referenced by items, not items themselves"
                ));
            }
        }
        for (i, c) in self.conditions.iter().enumerate() {
            if !self.evidence.contains_key(&c.evidence) {
                return bad(format!(
                    "condition {i} names unknown evidence '{}'",
                    c.evidence
                ));
            }
            let ok = match c.op {
                Op::Between => c.min.is_some() && c.max.is_some(),
                Op::In => c.values.as_ref().is_some_and(|v| !v.is_empty()),
                _ => c.value.is_some(),
            };
            if !ok {
                return bad(format!(
                    "condition {i}: '{:?}' is missing its operand",
                    c.op
                ));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- the report

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Satisfied,
    Violated,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConditionResult {
    True,
    False,
    Undetermined,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Reason {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    pub detail: String,
}

fn reason(code: &str, item: &str, record: Option<&str>, detail: impl Into<String>) -> Reason {
    Reason {
        code: code.to_owned(),
        item: Some(item.to_owned()),
        record: record.map(str::to_owned),
        detail: detail.into(),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemReport {
    pub anchor: bool,
    pub accepted: Vec<String>,
    pub rejected: Vec<Reason>,
    /// Where accepted records say they came from, so a simulator is never mistaken for a device.
    pub provenance: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionReport {
    pub index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub evidence: String,
    pub op: Op,
    pub result: ConditionResult,
    pub records: Vec<String>,
    pub observed: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub verifier: String,
    pub contract: Value,
    pub evidence: Value,
    pub verdict: Verdict,
    pub reasons: Vec<Reason>,
    pub conditions: Vec<ConditionReport>,
    pub items: BTreeMap<String, ItemReport>,
}

impl Report {
    /// Canonical bytes: the same inputs always give these same bytes.
    pub fn to_canonical_json(&self) -> Vec<u8> {
        // serde_json's map is ordered, so a round trip through Value sorts every object's keys.
        serde_json::to_value(self)
            .and_then(|v| serde_json::to_vec_pretty(&v))
            .unwrap_or_default()
    }
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn canonical<T: Serialize>(v: &T) -> Vec<u8> {
    serde_json::to_value(v)
        .and_then(|v| serde_json::to_vec(&v))
        .unwrap_or_default()
}

// ---------------------------------------------------------------- verification

/// Parse a JSONL evidence file. Blank lines are allowed; anything else that is not a record is an error.
pub fn parse_evidence(text: &str) -> Result<Vec<Record>, InputError> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let r: Record = serde_json::from_str(line)
            .map_err(|e| InputError::Evidence(format!("line {}: {e}", n + 1)))?;
        out.push(r);
    }
    Ok(out)
}

struct Timed {
    at: i64,
    domain: Option<String>,
    uncertainty: Option<f64>,
}

fn timed(r: &Record) -> Result<Option<Timed>, String> {
    let Some(t) = &r.time else { return Ok(None) };
    let Some(src) = &t.source else {
        return Ok(None);
    };
    let at = openmhs_types::Timestamp::parse_rfc3339(src)
        .map_err(|e| format!("time.source '{src}' is not RFC 3339: {e}"))?
        .millis();
    if t.uncertainty_ms
        .is_some_and(|u| !(u.is_finite() && u >= 0.0))
    {
        return Err("time.uncertainty_ms must be a non-negative number".into());
    }
    Ok(Some(Timed {
        at,
        domain: t.clock_domain.clone(),
        uncertainty: t.uncertainty_ms,
    }))
}

fn parse_time(s: &str, what: &str) -> Result<i64, String> {
    openmhs_types::Timestamp::parse_rfc3339(s)
        .map(|t| t.millis())
        .map_err(|e| format!("{what} '{s}' is not RFC 3339: {e}"))
}

/// What the rules said about one record for one item.
enum Fate {
    Accepted,
    Rejected(Reason),
}

struct Ctx<'a> {
    contract: &'a Contract,
    calibrations: Vec<&'a Record>,
}

impl Ctx<'_> {
    fn candidates<'r>(&self, item: &Item, records: &'r [Record]) -> Vec<&'r Record> {
        records
            .iter()
            .filter(|r| r.signal == item.signal && r.source.kind == item.kind)
            .filter(|r| item.device.is_none() || r.source.device == item.device)
            .collect()
    }

    /// Rules 1, 2, 4 (own uncertainty) and 6. Time windows are applied by the caller.
    fn check(&self, name: &str, item: &Item, r: &Record, t: Option<&Timed>) -> Fate {
        let c = self.contract;
        if item.bound {
            for (key, want) in &c.binding {
                match r.refs.get(key) {
                    None => {
                        return Fate::Rejected(reason(
                            "evidence.identity_unbound",
                            name,
                            Some(&r.id),
                            format!(
                                "carries no '{key}', and the contract is bound to {key}={want}"
                            ),
                        ));
                    }
                    Some(got) if got != want => {
                        return Fate::Rejected(reason(
                            "evidence.identity_mismatch",
                            name,
                            Some(&r.id),
                            format!("{key}={got}, the contract is bound to {key}={want}"),
                        ));
                    }
                    Some(_) => {}
                }
            }
        }
        if let Some(want) = &item.frame
            && r.frame.as_deref() != Some(want.as_str())
        {
            return Fate::Rejected(reason(
                "evidence.frame",
                name,
                Some(&r.id),
                format!(
                    "expressed in frame '{}', the contract needs '{want}'",
                    r.frame.as_deref().unwrap_or("none")
                ),
            ));
        }
        if r.quality != Quality::Good {
            return Fate::Rejected(reason(
                "evidence.quality",
                name,
                Some(&r.id),
                format!("quality is {:?}, only GOOD counts", r.quality),
            ));
        }
        if let (Some(t), Some(max)) = (
            t,
            c.timing.as_ref().and_then(|t| t.max_clock_uncertainty_ms),
        ) && let Some(u) = t.uncertainty
            && u > max
        {
            return Fate::Rejected(reason(
                "time.uncertainty",
                name,
                Some(&r.id),
                format!("clock uncertainty {u} ms is more than the {max} ms allowed"),
            ));
        }
        for cal in &item.calibration {
            if let Some(rejection) = self.calibration(name, r, t, cal) {
                return Fate::Rejected(rejection);
            }
        }
        Fate::Accepted
    }

    fn calibration(&self, name: &str, r: &Record, t: Option<&Timed>, cal: &str) -> Option<Reason> {
        let device = r.source.device.as_deref().unwrap_or("");
        let Some(t) = t else {
            return Some(reason(
                "time.missing",
                name,
                Some(&r.id),
                format!("has no source time, so calibration '{cal}' cannot be checked"),
            ));
        };
        let matching: Vec<&CalibrationInfo> = self
            .calibrations
            .iter()
            .filter_map(|c| c.calibration.as_ref())
            .filter(|c| c.name == cal && c.applies_to == device)
            .collect();
        if matching.is_empty() {
            return Some(reason(
                "calibration.missing",
                name,
                Some(&r.id),
                format!("no GOOD calibration '{cal}' for device '{device}'"),
            ));
        }
        let mut expired = false;
        for c in &matching {
            let (Ok(from), Ok(until)) = (
                parse_time(&c.valid_from, "valid_from"),
                parse_time(&c.valid_until, "valid_until"),
            ) else {
                continue;
            };
            if from <= t.at && t.at <= until {
                return None;
            }
            expired |= until < t.at;
        }
        Some(if expired {
            reason(
                "calibration.expired",
                name,
                Some(&r.id),
                format!("calibration '{cal}' for '{device}' had expired at the reading"),
            )
        } else {
            reason(
                "calibration.not_yet_valid",
                name,
                Some(&r.id),
                format!("calibration '{cal}' for '{device}' was not yet valid at the reading"),
            )
        })
    }
}

struct Anchor {
    at: i64,
    domain: Option<String>,
    uncertainty: Option<f64>,
}

/// Rules 3 to 5 for a non-anchor record.
fn window_rule(
    name: &str,
    r: &Record,
    t: Option<&Timed>,
    anchor: &Anchor,
    w: [i64; 2],
) -> Option<Reason> {
    let Some(t) = t else {
        return Some(reason(
            "time.missing",
            name,
            Some(&r.id),
            "has no source time, and a window applies",
        ));
    };
    let combined = if t.domain == anchor.domain {
        0.0
    } else {
        match (t.uncertainty, anchor.uncertainty) {
            (Some(a), Some(b)) => a + b,
            _ => {
                return Some(reason(
                    "time.clock_domain",
                    name,
                    Some(&r.id),
                    format!(
                        "clock '{}' is not the anchor's '{}', and an uncertainty is not stated for both",
                        t.domain.as_deref().unwrap_or("unspecified"),
                        anchor.domain.as_deref().unwrap_or("unspecified"),
                    ),
                ));
            }
        }
    };
    let d = (t.at - anchor.at) as f64;
    let (lo, hi) = (w[0] as f64, w[1] as f64);
    if d - combined >= lo && d + combined <= hi {
        None
    } else if d + combined < lo || d - combined > hi {
        Some(reason(
            "evidence.outside_window",
            name,
            Some(&r.id),
            format!(
                "{} ms from the anchor, the window is [{}, {}] ms",
                t.at - anchor.at,
                w[0],
                w[1]
            ),
        ))
    } else {
        Some(reason(
            "time.ambiguous",
            name,
            Some(&r.id),
            format!(
                "{} ms from the anchor ± {combined} ms straddles the window [{}, {}] ms",
                t.at - anchor.at,
                w[0],
                w[1]
            ),
        ))
    }
}

enum Operand {
    Num(f64, Option<String>),
    Str(String),
    Bool(bool),
}

fn operand(v: &Value) -> Option<Operand> {
    match v {
        Value::Number(n) => n.as_f64().map(|f| Operand::Num(f, None)),
        Value::String(s) => Some(Operand::Str(s.clone())),
        Value::Bool(b) => Some(Operand::Bool(*b)),
        Value::Object(o) if o.len() == 2 => {
            let n = o.get("value")?.as_f64()?;
            let u = o.get("unit")?.as_str()?;
            Some(Operand::Num(n, Some(u.to_owned())))
        }
        _ => None,
    }
}

/// The record's value in the operand's terms, or why it cannot be compared.
fn comparable(record: &Operand, want: &Operand) -> Result<Operand, (&'static str, String)> {
    match (record, want) {
        (Operand::Num(v, from), Operand::Num(_, to)) => match (from, to) {
            (None, None) => Ok(Operand::Num(*v, None)),
            (Some(f), Some(t)) if f == t => Ok(Operand::Num(*v, Some(t.clone()))),
            (Some(f), Some(t)) => openmhs_types::units::convert(*v, f, t)
                .map(|c| Operand::Num(c, Some(t.clone())))
                .map_err(|e| {
                    (
                        "evidence.unit",
                        format!("{v} {f} cannot be compared in {t}: {e}"),
                    )
                }),
            (None, Some(t)) => Err((
                "evidence.unit",
                format!("{v} has no unit; the condition is in {t}"),
            )),
            (Some(f), None) => Err((
                "evidence.unit",
                format!("{v} {f} has a unit; the condition has none"),
            )),
        },
        (Operand::Str(s), Operand::Str(_)) => Ok(Operand::Str(s.clone())),
        (Operand::Bool(b), Operand::Bool(_)) => Ok(Operand::Bool(*b)),
        _ => Err((
            "evidence.type",
            "the value's type does not match the condition's".into(),
        )),
    }
}

fn equal(a: &Operand, b: &Operand) -> bool {
    match (a, b) {
        (Operand::Num(x, _), Operand::Num(y, _)) => x == y,
        (Operand::Str(x), Operand::Str(y)) => x == y,
        (Operand::Bool(x), Operand::Bool(y)) => x == y,
        _ => false,
    }
}

fn number(o: &Operand) -> Option<f64> {
    match o {
        Operand::Num(n, _) => Some(*n),
        _ => None,
    }
}

/// One condition on one record's value.
fn evaluate(c: &Condition, value: &Value) -> Result<bool, (&'static str, String)> {
    let got = operand(value).ok_or((
        "evidence.type",
        "the value is not a number, string, boolean or quantity".to_owned(),
    ))?;
    let one = |v: &Option<Value>| -> Result<Operand, (&'static str, String)> {
        v.as_ref().and_then(operand).ok_or((
            "contract.operand",
            "the condition's operand is not comparable".into(),
        ))
    };
    let ordered =
        |want: Operand, cmp: fn(f64, f64) -> bool| -> Result<bool, (&'static str, String)> {
            let have = comparable(&got, &want)?;
            match (number(&have), number(&want)) {
                (Some(h), Some(w)) => Ok(cmp(h, w)),
                _ => Err(("evidence.type", "an ordering needs numbers".into())),
            }
        };
    match c.op {
        Op::Eq | Op::Ne => {
            let want = one(&c.value)?;
            let have = comparable(&got, &want)?;
            Ok(equal(&have, &want) == (c.op == Op::Eq))
        }
        Op::Lt => ordered(one(&c.value)?, |h, w| h < w),
        Op::Le => ordered(one(&c.value)?, |h, w| h <= w),
        Op::Gt => ordered(one(&c.value)?, |h, w| h > w),
        Op::Ge => ordered(one(&c.value)?, |h, w| h >= w),
        Op::Between => {
            let low = ordered(one(&c.min)?, |h, w| h >= w)?;
            let high = ordered(one(&c.max)?, |h, w| h <= w)?;
            Ok(low && high)
        }
        Op::In => {
            let mut any = false;
            for v in c.values.iter().flatten() {
                let want = operand(v).ok_or((
                    "contract.operand",
                    "an `in` value is not comparable".to_owned(),
                ))?;
                let have = comparable(&got, &want)?;
                any |= equal(&have, &want);
            }
            Ok(any)
        }
    }
}

/// Check `records` against `contract` (RFC 0004, "The rules, in order").
/// Rule 8 (opt-in): records of one item whose value has been repeated for longer than `max_unchanged_ms`, with how long.
///
/// Records are taken in source-time order (ties by id). A run is a stretch of consecutive records holding an equal
/// value; a record's unchanged time is its time minus the start of its run. Records with no usable time are skipped
/// here and are dealt with by the time rules. The run is measured over the records the verifier is given, so a caller
/// that trims the evidence must keep enough history before the window.
fn stale_records<'a>(
    candidates: &[&'a Record],
    times: &BTreeMap<&str, Result<Option<Timed>, String>>,
    max_unchanged_ms: u64,
) -> BTreeMap<&'a str, i64> {
    let mut timed_records: Vec<(i64, &Record)> = candidates
        .iter()
        .filter_map(|r| match times.get(r.id.as_str()) {
            Some(Ok(Some(t))) => Some((t.at, *r)),
            _ => None,
        })
        .collect();
    timed_records.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)));
    let mut stale = BTreeMap::new();
    let mut run: Option<(i64, &Value)> = None;
    for (at, r) in timed_records {
        let start = match run {
            Some((start, value)) if value == &r.value => start,
            _ => at,
        };
        run = Some((start, &r.value));
        let unchanged = at - start;
        if unchanged > i64::try_from(max_unchanged_ms).unwrap_or(i64::MAX) {
            stale.insert(r.id.as_str(), unchanged);
        }
    }
    stale
}

pub fn verify(contract: &Contract, records: &[Record]) -> Result<Report, InputError> {
    contract.check()?;
    let mut ids = BTreeSet::new();
    for r in records {
        if !ids.insert(r.id.as_str()) {
            return Err(InputError::Evidence(format!(
                "record id '{}' appears more than once",
                r.id
            )));
        }
        if let Some(c) = &r.calibration
            && r.source.kind != SourceKind::Calibration
        {
            return Err(InputError::Evidence(format!(
                "record '{}' has a calibration block but is not a calibration record ({})",
                r.id, c.name
            )));
        }
    }
    // Records in a fixed order, so the report does not depend on the order they arrived in.
    let mut sorted: Vec<Record> = records.to_vec();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let mut times: BTreeMap<&str, Result<Option<Timed>, String>> = BTreeMap::new();
    for r in &sorted {
        times.insert(r.id.as_str(), timed(r));
    }
    let ctx = Ctx {
        contract,
        calibrations: sorted
            .iter()
            .filter(|r| r.source.kind == SourceKind::Calibration && r.quality == Quality::Good)
            .collect(),
    };

    let mut items: BTreeMap<String, ItemReport> = BTreeMap::new();
    let mut accepted: BTreeMap<&str, Vec<&Record>> = BTreeMap::new();
    let mut verdict_reasons: Vec<Reason> = Vec::new();

    let first_pass = |name: &str, item: &Item, report: &mut ItemReport| -> Vec<&Record> {
        let mut kept = Vec::new();
        let candidates = ctx.candidates(item, &sorted);
        let stale = item
            .max_unchanged_ms
            .map(|max| stale_records(&candidates, &times, max))
            .unwrap_or_default();
        for r in candidates {
            let t = match times.get(r.id.as_str()) {
                Some(Ok(t)) => t.as_ref(),
                Some(Err(e)) => {
                    report
                        .rejected
                        .push(reason("time.invalid", name, Some(&r.id), e.clone()));
                    continue;
                }
                None => None,
            };
            match ctx.check(name, item, r, t) {
                Fate::Accepted => match stale.get(r.id.as_str()) {
                    Some(unchanged_ms) => report.rejected.push(reason(
                        "evidence.stale",
                        name,
                        Some(&r.id),
                        format!(
                            "the value has been unchanged for {unchanged_ms} ms, and the contract allows {} ms",
                            item.max_unchanged_ms.unwrap_or_default()
                        ),
                    )),
                    None => kept.push(r),
                },
                Fate::Rejected(why) => report.rejected.push(why),
            }
        }
        kept
    };

    // The anchor first: its time defines every window.
    let mut anchor: Option<Anchor> = None;
    let mut anchor_missing = false;
    if let Some(timing) = &contract.timing {
        let name = timing.anchor.as_str();
        let item = &contract.evidence[name];
        let mut report = ItemReport {
            anchor: true,
            accepted: vec![],
            rejected: vec![],
            provenance: vec![],
        };
        let mut kept = Vec::new();
        for r in first_pass(name, item, &mut report) {
            match times.get(r.id.as_str()) {
                Some(Ok(Some(_))) => kept.push(r),
                _ => report.rejected.push(reason(
                    "time.missing",
                    name,
                    Some(&r.id),
                    "the anchor needs a source time",
                )),
            }
        }
        let distinct: BTreeSet<i64> = kept
            .iter()
            .filter_map(|r| match times.get(r.id.as_str()) {
                Some(Ok(Some(t))) => Some(t.at),
                _ => None,
            })
            .collect();
        if distinct.len() == 1
            && let Some(Some(Ok(Some(t)))) = kept.first().map(|r| times.get(r.id.as_str()))
        {
            anchor = Some(Anchor {
                at: t.at,
                domain: t.domain.clone(),
                uncertainty: t.uncertainty,
            });
        } else {
            anchor_missing = true;
            let why = if kept.is_empty() {
                reason(
                    "timing.anchor_missing",
                    name,
                    None,
                    "no record survived the rules, so no window can be placed",
                )
            } else {
                reason(
                    "timing.anchor_ambiguous",
                    name,
                    None,
                    format!("{} anchor records at different times", kept.len()),
                )
            };
            verdict_reasons.push(why);
            // Why the anchor's records were discarded is what makes this actionable: an expired calibration,
            // another attempt's id. Found on real data, where "no anchor" alone hid the cause.
            verdict_reasons.extend(report.rejected.iter().cloned());
        }
        report.accepted = kept.iter().map(|r| r.id.clone()).collect();
        report.provenance = provenance(&kept);
        accepted.insert(name, kept);
        items.insert(name.to_owned(), report);
    }

    for (name, item) in &contract.evidence {
        if items.contains_key(name) {
            continue;
        }
        let mut report = ItemReport {
            anchor: false,
            accepted: vec![],
            rejected: vec![],
            provenance: vec![],
        };
        let mut kept = Vec::new();
        if anchor_missing {
            // Without an anchor there is no window to place this item in.
        } else {
            let window = item
                .window_ms
                .or_else(|| contract.timing.as_ref().and_then(|t| t.default_window_ms));
            for r in first_pass(name, item, &mut report) {
                let t = match times.get(r.id.as_str()) {
                    Some(Ok(t)) => t.as_ref(),
                    _ => None,
                };
                let rejection = match (&anchor, window) {
                    (Some(a), Some(w)) => window_rule(name, r, t, a, w),
                    _ => None,
                };
                match rejection {
                    None => kept.push(r),
                    Some(why) => report.rejected.push(why),
                }
            }
        }
        report.accepted = kept.iter().map(|r| r.id.clone()).collect();
        report.provenance = provenance(&kept);
        accepted.insert(name.as_str(), kept);
        items.insert(name.clone(), report);
    }

    let mut conditions = Vec::new();
    for (index, c) in contract.conditions.iter().enumerate() {
        let used = accepted
            .get(c.evidence.as_str())
            .cloned()
            .unwrap_or_default();
        let mut cr = ConditionReport {
            index,
            name: c.name.clone(),
            evidence: c.evidence.clone(),
            op: c.op,
            result: ConditionResult::Undetermined,
            records: used.iter().map(|r| r.id.clone()).collect(),
            observed: used.iter().map(|r| r.value.clone()).collect(),
            reason: None,
        };
        if anchor_missing
            && contract
                .timing
                .as_ref()
                .is_some_and(|t| t.anchor != c.evidence)
        {
            cr.reason = Some(reason(
                "timing.anchor_missing",
                &c.evidence,
                None,
                "no window, so no evidence for this item could be accepted",
            ));
        } else if used.is_empty() {
            // "Missing" is for evidence that never arrived; "rejected" for evidence that did, and failed a rule.
            cr.reason = Some(match items.get(&c.evidence) {
                Some(i) if !i.rejected.is_empty() => reason(
                    "evidence.rejected",
                    &c.evidence,
                    None,
                    format!(
                        "{} record(s) considered, none passed the rules: see items.{}",
                        i.rejected.len(),
                        c.evidence
                    ),
                ),
                _ => reason(
                    "evidence.missing",
                    &c.evidence,
                    None,
                    "no record with this signal and source kind",
                ),
            });
        } else {
            let mut results = Vec::new();
            let mut failure = None;
            for r in &used {
                match evaluate(c, &r.value) {
                    Ok(b) => results.push(b),
                    Err((code, detail)) => {
                        failure = Some(reason(code, &c.evidence, Some(&r.id), detail));
                        break;
                    }
                }
            }
            if let Some(f) = failure {
                cr.reason = Some(f);
            } else if results.iter().all(|b| *b) {
                cr.result = ConditionResult::True;
            } else if results.iter().all(|b| !*b) {
                cr.result = ConditionResult::False;
                cr.reason = Some(reason(
                    "condition.false",
                    &c.evidence,
                    None,
                    format!("{:?} does not hold on {}", c.op, cr.records.join(", ")),
                ));
            } else {
                cr.reason = Some(reason(
                    "evidence.conflict",
                    &c.evidence,
                    None,
                    format!("records disagree: {}", cr.records.join(", ")),
                ));
            }
        }
        conditions.push(cr);
    }

    let verdict = if conditions
        .iter()
        .any(|c| c.result == ConditionResult::False)
    {
        Verdict::Violated
    } else if conditions
        .iter()
        .any(|c| c.result == ConditionResult::Undetermined)
    {
        Verdict::Unknown
    } else {
        Verdict::Satisfied
    };
    if verdict != Verdict::Satisfied {
        for c in &conditions {
            if c.result != ConditionResult::True
                && let Some(r) = &c.reason
            {
                verdict_reasons.push(r.clone());
            }
        }
        // The specific reason a record was discarded is what makes an UNKNOWN actionable.
        for c in &conditions {
            if c.result == ConditionResult::Undetermined
                && let Some(i) = items.get(&c.evidence)
            {
                verdict_reasons.extend(i.rejected.iter().cloned());
            }
        }
    }
    verdict_reasons.sort();
    verdict_reasons.dedup();

    let mut evidence_values: Vec<Value> = sorted
        .iter()
        .map(|r| serde_json::to_value(r).unwrap_or(Value::Null))
        .collect();
    evidence_values.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    Ok(Report {
        verifier: VERIFIER.to_owned(),
        contract: json!({
            "name": contract.contract, "version": contract.version,
            "sha256": sha256(&canonical(contract)),
        }),
        evidence: json!({ "records": sorted.len(), "sha256": sha256(&canonical(&evidence_values)) }),
        verdict,
        reasons: verdict_reasons,
        conditions,
        items,
    })
}

fn provenance(records: &[&Record]) -> Vec<String> {
    let set: BTreeSet<String> = records
        .iter()
        .map(|r| match r.source.provenance {
            Some(Provenance::Device) => "device".to_owned(),
            Some(Provenance::Simulator) => "simulator".to_owned(),
            None => "unstated".to_owned(),
        })
        .collect();
    set.into_iter().collect()
}
