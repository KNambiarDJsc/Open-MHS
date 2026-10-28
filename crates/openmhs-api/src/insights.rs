//! What the journal says happened: evidence an agent can iterate on (ADR-0022).
//!
//! [`summarize`] is a pure function over command receipts. It reads nothing but what the
//! gateway recorded, invents nothing, and writes nothing back, so it cannot widen what an
//! agent may do: limits, policy and descriptors are never changed by what it reports.
//! Every figure names the commands it came from. Text written by devices or by an agent
//! is never copied in; only codes, numbers, states and ids.

use std::collections::BTreeMap;

use openmhs_types::{CommandState, Receipt, Value};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How many of the most recent command ids to name as evidence per section.
const EVIDENCE: usize = 5;

/// schemars tags Rust integer widths with formats such as `int64` that JSON Schema does not
/// define, and strict validators in MCP clients warn about them. `minimum` already carries
/// the bound, so drop them.
pub fn standard_formats(schema: &mut schemars::Schema) {
    schemars::transform::transform_subschemas(&mut standard_formats, schema);
    if let Some(obj) = schema.as_object_mut()
        && obj
            .get("format")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|f| f.trim_start_matches('u').starts_with("int"))
    {
        obj.remove("format");
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Insights {
    pub device: String,
    pub window: Window,
    pub capabilities: Vec<CapabilityInsights>,
}

/// The commands the figures are computed from. A gateway lists at most a few hundred of
/// the newest commands, so older history is not included; `truncated` says when that may
/// matter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = standard_formats)]
pub struct Window {
    /// Commands for this device that were examined.
    pub commands: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest: Option<String>,
    /// The listing was full, so older commands may exist that are not counted.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = standard_formats)]
pub struct CapabilityInsights {
    pub capability: String,
    pub attempts: usize,
    /// Commands by final state, e.g. `VERIFIED`, `DENIED`, `UNKNOWN_OUTCOME`.
    pub states: BTreeMap<String, usize>,
    /// Why commands were refused, most frequent first, with the numbers from the latest
    /// refusal of each kind (the bounds the agent ran into).
    pub refusals: Vec<Refusal>,
    /// Values the device was observed to hold after a command (readback confirmed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified: Option<VerifiedValues>,
    /// How far the observed value was from the requested one, over commands with a numeric
    /// readback. A large or growing figure is a property of the device, not of the agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracking_error: Option<TrackingError>,
    /// Time from sending a command to its readback being confirmed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify_latency_ms: Option<Latency>,
    /// Commands whose outcome is not known. Reconcile them before sending another.
    pub unresolved: Vec<String>,
    /// The newest commands this section is computed from, newest first.
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = standard_formats)]
pub struct Refusal {
    pub code: String,
    pub count: usize,
    pub latest_command: String,
    /// The numbers the latest refusal reported (limits, ranges, units).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_details: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = standard_formats)]
pub struct VerifiedValues {
    pub count: usize,
    pub unit: String,
    pub lowest: f64,
    pub highest: f64,
    pub latest: f64,
    pub latest_at: String,
    pub latest_command: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = standard_formats)]
pub struct TrackingError {
    pub samples: usize,
    pub unit: String,
    pub mean_abs: f64,
    pub max_abs: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = standard_formats)]
pub struct Latency {
    pub samples: usize,
    pub median: i64,
    pub max: i64,
}

/// Count of one refusal code, the newest command that had it, and that command's numbers.
type RefusalAcc<'a> = (
    usize,
    &'a Receipt,
    Option<&'a serde_json::Map<String, serde_json::Value>>,
);

#[derive(Default)]
struct Acc<'a> {
    attempts: usize,
    states: BTreeMap<String, usize>,
    refusals: BTreeMap<String, RefusalAcc<'a>>,
    verified: Vec<(&'a Receipt, f64, String)>,
    errors: Vec<(f64, String)>,
    latency: Vec<i64>,
    unresolved: Vec<String>,
    evidence: Vec<String>,
}

/// Summarize `receipts` (any order) for `device`, optionally one capability. `limit` is the
/// listing size the receipts came from, to tell whether the window was cut off.
pub fn summarize(
    receipts: &[Receipt],
    device: &str,
    capability: Option<&str>,
    limit: usize,
) -> Insights {
    let mut mine: Vec<&Receipt> = receipts
        .iter()
        .filter(|r| r.request.target.device == device)
        .filter(|r| capability.is_none_or(|c| r.request.target.capability == c))
        .collect();
    mine.sort_by_key(|r| std::cmp::Reverse(r.created_at));

    let mut by_cap: BTreeMap<&str, Acc> = BTreeMap::new();
    for r in &mine {
        let acc = by_cap.entry(&r.request.target.capability).or_default();
        acc.attempts += 1;
        if acc.evidence.len() < EVIDENCE {
            acc.evidence.push(r.command_id.clone());
        }
        *acc.states.entry(wire(r.state)).or_default() += 1;

        if r.state == CommandState::UnknownOutcome {
            acc.unresolved.push(r.command_id.clone());
        }
        if matches!(
            r.state,
            CommandState::Denied | CommandState::Rejected | CommandState::Failed
        ) {
            for e in &r.policy.errors {
                // Newest first, so the first one seen is the latest of its kind.
                let entry =
                    acc.refusals
                        .entry(e.code.clone())
                        .or_insert((0, r, e.details.as_ref()));
                entry.0 += 1;
            }
        }
        if r.state == CommandState::Verified {
            if let Some(v) = &r.verification
                && let (Some(Value::Quantity(want)), Some(seen)) = (&v.expected, &v.observed)
                && let Value::Quantity(got) = &seen.value
                && got.unit == want.unit
            {
                acc.verified.push((r, got.value, got.unit.clone()));
                acc.errors
                    .push(((got.value - want.value).abs(), got.unit.clone()));
            }
            let sent = r
                .transitions
                .iter()
                .find(|t| t.state == CommandState::Dispatched);
            let done = r
                .transitions
                .iter()
                .find(|t| t.state == CommandState::Verified);
            if let (Some(a), Some(b)) = (sent, done) {
                acc.latency.push(a.at.ms_until(b.at).max(0));
            }
        }
    }

    let capabilities = by_cap
        .into_iter()
        .map(|(name, acc)| finish(name, acc))
        .collect();
    Insights {
        device: device.to_owned(),
        window: Window {
            commands: mine.len(),
            oldest: mine.last().map(|r| r.created_at.to_rfc3339()),
            newest: mine.first().map(|r| r.created_at.to_rfc3339()),
            truncated: limit > 0 && receipts.len() >= limit,
        },
        capabilities,
    }
}

fn finish(name: &str, acc: Acc<'_>) -> CapabilityInsights {
    let mut refusals: Vec<Refusal> = acc
        .refusals
        .into_iter()
        .map(|(code, (count, r, details))| Refusal {
            code,
            count,
            latest_command: r.command_id.clone(),
            latest_details: details.cloned(),
        })
        .collect();
    refusals.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.code.cmp(&b.code)));

    // `acc.verified` is newest first. Quantities of one unit only: the unit of the newest.
    let verified = acc.verified.first().map(|(newest, latest, unit)| {
        let same: Vec<f64> = acc
            .verified
            .iter()
            .filter(|(_, _, u)| u == unit)
            .map(|(_, v, _)| *v)
            .collect();
        VerifiedValues {
            count: same.len(),
            unit: unit.clone(),
            lowest: same.iter().copied().fold(f64::INFINITY, f64::min),
            highest: same.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            latest: *latest,
            latest_at: newest.created_at.to_rfc3339(),
            latest_command: newest.command_id.clone(),
        }
    });
    let tracking_error = verified.as_ref().and_then(|v| {
        let errs: Vec<f64> = acc
            .errors
            .iter()
            .filter(|(_, u)| *u == v.unit)
            .map(|(e, _)| *e)
            .collect();
        (!errs.is_empty()).then(|| TrackingError {
            samples: errs.len(),
            unit: v.unit.clone(),
            mean_abs: errs.iter().sum::<f64>() / errs.len() as f64,
            max_abs: errs.iter().copied().fold(0.0, f64::max),
        })
    });
    let verify_latency_ms = (!acc.latency.is_empty()).then(|| {
        let mut l = acc.latency.clone();
        l.sort_unstable();
        Latency {
            samples: l.len(),
            median: l[l.len() / 2],
            max: l[l.len() - 1],
        }
    });
    CapabilityInsights {
        capability: name.to_owned(),
        attempts: acc.attempts,
        states: acc.states,
        refusals,
        verified,
        tracking_error,
        verify_latency_ms,
        unresolved: acc.unresolved,
        evidence: acc.evidence,
    }
}

fn wire(state: CommandState) -> String {
    match serde_json::to_value(state) {
        Ok(serde_json::Value::String(s)) => s,
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use openmhs_types::receipt::{TransitionRecord, VerificationMode, VerificationRecord};
    use openmhs_types::{
        CommandRequest, ErrorObject, Quality, RequestOperation, Source, StateValue, Target,
        Timestamp,
    };

    use super::*;

    fn quantity(v: f64) -> Value {
        Value::Quantity(openmhs_types::Quantity {
            value: v,
            unit: "V".into(),
        })
    }

    fn receipt(id: &str, device: &str, cap: &str, at: i64, state: CommandState) -> Receipt {
        let request = CommandRequest {
            target: Target {
                device: device.into(),
                capability: cap.into(),
            },
            operation: RequestOperation::Write,
            value: None,
            params: None,
            idempotency_key: None,
            correlation_id: None,
            deadline_ms: None,
        };
        let mut r = Receipt::created(
            id.into(),
            "agent:a".into(),
            request,
            Timestamp::from_millis(at),
        );
        r.state = state;
        r
    }

    fn verified(id: &str, at: i64, want: f64, got: f64, latency: i64) -> Receipt {
        let mut r = receipt(id, "d", "c", at, CommandState::Verified);
        for (state, offset) in [
            (CommandState::Dispatched, 5),
            (CommandState::Verified, 5 + latency),
        ] {
            r.transitions.push(TransitionRecord {
                state,
                at: Timestamp::from_millis(at + offset),
                detail: None,
            });
        }
        r.verification = Some(VerificationRecord {
            mode: VerificationMode::Readback,
            capability: Some("c".into()),
            expected: Some(quantity(want)),
            observed: Some(StateValue {
                value: quantity(got),
                timestamp: Timestamp::from_millis(at + 10),
                source: Source::Device,
                quality: Quality::Good,
            }),
            matched: Some(true),
            reason: None,
        });
        r
    }

    fn denied(id: &str, at: i64, code: &str, max: f64) -> Receipt {
        let mut r = receipt(id, "d", "c", at, CommandState::Denied);
        let mut e = ErrorObject::new(code, "refused");
        e.details = serde_json::json!({ "max_change": max })
            .as_object()
            .cloned();
        r.policy.errors.push(e);
        r
    }

    #[test]
    fn refusals_are_counted_with_the_latest_numbers_and_ordered_by_frequency() {
        let rs = vec![
            denied("old", 1, "command.slew_exceeded", 5.0),
            denied("mid", 2, "command.slew_exceeded", 3.0),
            denied("new", 3, "command.slew_exceeded", 2.0),
            denied("x", 4, "command.value.out_of_range", 0.0),
        ];
        let i = summarize(&rs, "d", None, 500);
        let c = &i.capabilities[0];
        assert_eq!(c.attempts, 4);
        assert_eq!(c.refusals[0].code, "command.slew_exceeded");
        assert_eq!(c.refusals[0].count, 3);
        assert_eq!(c.refusals[0].latest_command, "new", "the newest kept");
        assert_eq!(
            c.refusals[0].latest_details.as_ref().unwrap()["max_change"],
            2.0
        );
        assert!(c.verified.is_none() && c.tracking_error.is_none());
    }

    #[test]
    fn confirmed_values_error_and_latency_come_only_from_verified_commands() {
        let rs = vec![
            verified("a", 100, 2.0, 2.0, 40),
            verified("b", 200, 4.0, 4.5, 60),
            verified("c", 300, 3.0, 3.0, 400),
            denied("n", 400, "command.slew_exceeded", 2.0),
        ];
        let c = &summarize(&rs, "d", Some("c"), 500).capabilities[0];
        let v = c.verified.as_ref().unwrap();
        assert_eq!((v.count, v.lowest, v.highest, v.latest), (3, 2.0, 4.5, 3.0));
        assert_eq!(v.latest_command, "c");
        let t = c.tracking_error.as_ref().unwrap();
        assert_eq!(t.samples, 3);
        assert_eq!(t.max_abs, 0.5);
        assert!((t.mean_abs - 0.5 / 3.0).abs() < 1e-9);
        let l = c.verify_latency_ms.as_ref().unwrap();
        assert_eq!((l.samples, l.median, l.max), (3, 60, 400));
    }

    #[test]
    fn unresolved_outcomes_are_listed_and_other_devices_are_ignored() {
        let rs = vec![
            receipt("u", "d", "c", 1, CommandState::UnknownOutcome),
            receipt("other", "e", "c", 2, CommandState::UnknownOutcome),
        ];
        let i = summarize(&rs, "d", None, 500);
        assert_eq!(i.window.commands, 1);
        assert_eq!(i.capabilities[0].unresolved, ["u"]);
        assert!(summarize(&rs, "nope", None, 500).capabilities.is_empty());
    }

    #[test]
    fn a_full_listing_is_reported_as_possibly_truncated() {
        let rs: Vec<Receipt> = (0..3)
            .map(|n| receipt(&format!("r{n}"), "d", "c", n, CommandState::Denied))
            .collect();
        assert!(summarize(&rs, "d", None, 3).window.truncated);
        assert!(!summarize(&rs, "d", None, 500).window.truncated);
    }

    #[test]
    fn mixed_units_are_not_averaged_together() {
        let mut mv = verified("mv", 50, 1.0, 1.0, 10);
        if let Some(v) = mv.verification.as_mut() {
            v.observed.as_mut().unwrap().value = Value::Quantity(openmhs_types::Quantity {
                value: 1000.0,
                unit: "mV".into(),
            });
        }
        let rs = vec![verified("v", 100, 2.0, 2.0, 10), mv];
        let c = &summarize(&rs, "d", None, 500).capabilities[0];
        let v = c.verified.as_ref().unwrap();
        assert_eq!((v.unit.as_str(), v.count, v.highest), ("V", 1, 2.0));
    }
}
