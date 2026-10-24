//! Agent-facing views of gateway results, with concrete next steps.
//!
//! An agent replans from structured reasons, not prose. Every refusal or uncertain
//! outcome therefore carries the stable error codes, the policy rule involved, the
//! numbers from the violated constraint, and `next_steps` derived from them, e.g. "change
//! by at most 2 V per 1000 ms". The guidance is advisory text for a model; the decision
//! itself was already made by the gateway.

use openmhs_types::receipt::{Decision, PreflightResult, VerificationRecord};
use openmhs_types::{CommandState, ErrorObject, Outcome, Receipt};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

/// Why something was refused or did not go as requested.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Reason {
    /// Stable code from spec/errors.md, e.g. `command.value.out_of_range`.
    pub code: String,
    pub message: String,
    /// Policy rule or precondition id involved, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    /// Numbers from the violated constraint, e.g. `{"min": 0, "max": 12, "unit": "V"}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Map<String, Value>>,
}

impl From<&ErrorObject> for Reason {
    fn from(e: &ErrorObject) -> Self {
        Self {
            code: e.code.clone(),
            message: e.message.clone(),
            rule_id: e.rule_id.clone(),
            details: e.details.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Verification {
    /// `readback` or `none`.
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<Value>,
    /// The reading used as evidence (value, timestamp, quality).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn to_json<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

impl From<&VerificationRecord> for Verification {
    fn from(v: &VerificationRecord) -> Self {
        Self {
            mode: name(&v.mode),
            capability: v.capability.clone(),
            expected: v.expected.as_ref().map(to_json),
            observed: v.observed.as_ref().map(to_json),
            matched: v.matched,
            reason: v.reason.clone(),
        }
    }
}

/// A command as an agent should see it.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CommandView {
    /// Use with `get_command`, `reconcile_command` and `cancel_command`.
    pub command_id: String,
    pub device: String,
    pub capability: String,
    /// Lifecycle state, e.g. `VERIFIED`, `DENIED`, `AWAITING_APPROVAL`, `UNKNOWN_OUTCOME`.
    pub state: String,
    /// `succeeded`, `unverified`, `failed`, `denied`, `not_executed`, `uncertain` or `pending`.
    pub outcome: String,
    /// True only if the physical effect was confirmed by reading the device back.
    pub confirmed: bool,
    /// Whether anything may have reached the device.
    pub reached_device: bool,
    pub summary: String,
    pub reasons: Vec<Reason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<Verification>,
    /// Device-reported result values, e.g. `final_position`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    pub next_steps: Vec<String>,
}

/// Wire name of a serde enum value.
pub fn name<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(Value::String(s)) => s,
        Ok(other) => other.to_string(),
        Err(_) => "?".into(),
    }
}

fn detail(r: &Reason, key: &str) -> Option<String> {
    r.details.as_ref()?.get(key).map(|v| match v {
        Value::String(s) => s.clone(),
        // 2.0 reads as "2": these numbers are quoted to a model, not parsed back.
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
            _ => n.to_string(),
        },
        other => other.to_string(),
    })
}

/// Next steps for one refusal reason.
pub fn steps_for_reason(r: &Reason) -> Vec<String> {
    let unit = detail(r, "unit").unwrap_or_default();
    let step = |s: String| vec![s];
    match r.code.as_str() {
        "policy.denied.no_grant" | "policy.denied.explicit" | "policy.denied.side_effect" => step(
            "You are not permitted to do this. Do not retry it or look for a workaround; \
             if it is needed, tell the human operator which permission is missing."
                .into(),
        ),
        "command.value.out_of_range" => {
            let range = match (detail(r, "min"), detail(r, "max")) {
                (Some(min), Some(max)) => format!(" ({min}..{max} {unit})"),
                (None, Some(max)) => format!(" (at most {max} {unit})"),
                (Some(min), None) => format!(" (at least {min} {unit})"),
                (None, None) => String::new(),
            };
            step(format!(
                "Choose a value inside the declared range{range}. Do not exceed it to compensate for anything."
            ))
        }
        "command.slew_exceeded" => {
            let max = detail(r, "max_change").unwrap_or_else(|| "the limit".into());
            let per = detail(r, "per_ms").unwrap_or_else(|| "?".into());
            let u = if unit.is_empty() { String::new() } else { format!(" {unit}") };
            step(format!(
                "Change by at most {max}{u} per {per} ms: approach the target in steps of at most \
                 {max}{u}, waiting at least {per} ms between steps, and read the value back between steps."
            ))
        }
        "command.rate_limited" => {
            let per = detail(r, "per_ms").unwrap_or_else(|| "a while".into());
            step(format!(
                "Too many changes recently. Wait about {per} ms before trying again."
            ))
        }
        "command.precondition.failed" => step(format!(
            "Precondition '{}' does not hold. Read the state it depends on; fix it only if that \
             is itself allowed and safe, otherwise ask the operator.",
            r.rule_id.as_deref().unwrap_or("?")
        )),
        "command.state.stale" => step(
            "Telemetry needed for this decision is missing, stale or bad quality, so the gateway \
             refused rather than guess. Do not assume the device's state; wait, check it with \
             read_state, and retry once readings are fresh. If it stays stale, tell the operator."
                .into(),
        ),
        "command.value.type_mismatch" | "command.value.unit_unknown" | "command.value.unit_mismatch" => {
            step(
                "Send a value of the declared type. Numbers with units must be quantity objects \
                 like {\"value\": 5, \"unit\": \"V\"}; units are never guessed. See describe_device."
                    .into(),
            )
        }
        "command.value.not_allowed" | "command.param.missing" | "command.param.unknown"
        | "command.operation.unsupported" => step(
            "Check the capability's declared values and parameters with describe_device.".into(),
        ),
        "command.device.unknown" | "command.capability.unknown" => {
            step("Use list_devices and describe_device to find valid targets.".into())
        }
        "lease.held" | "lease.conflict" => step(
            "Another holder has leased this device (or something it shares). Wait until the \
             lease ends or ask the operator; never try to work around a lease."
                .into(),
        ),
        "lease.not_authorized" => step(
            "You may lease only devices you could operate. Leave the others out, or tell the \
             operator which permission is missing."
                .into(),
        ),
        "gateway.restarted" => step(
            "The gateway restarted before this command was checked; nothing was sent. \
             Submit it again if it is still needed."
                .into(),
        ),
        _ => vec![],
    }
}

/// Next steps for a lifecycle state.
fn steps_for_state(state: CommandState) -> Vec<String> {
    use CommandState::*;
    let s = |t: &str| vec![t.to_owned()];
    match state {
        AwaitingApproval => s(
            "A human must approve this before anything happens. Do not resubmit. Call \
             get_command with wait_ms to wait for the decision, or carry on with other work.",
        ),
        UnknownOutcome | TimedOut => s(
            "It may or may not have taken effect. Do NOT resubmit or retry. Call \
             reconcile_command to settle it by reading the device, then plan from the result.",
        ),
        Failed | Rejected => s(
            "The device did not carry it out as requested. Read the device's state before \
             deciding what to do next; do not blindly retry.",
        ),
        VerificationFailed => s(
            "The device reported success but reading it back contradicts that. Read the state, \
             and tell the operator: the device may be faulty or miscalibrated.",
        ),
        Unverifiable => s(
            "The device says it applied the change, but nothing confirmed it. Treat the effect as \
             unconfirmed.",
        ),
        ReconciledNotApplied => {
            s("Confirmed: it did not take effect. You may submit a new command if still needed.")
        }
        Expired | Cancelled => s("It was never sent. Submit again if it is still needed."),
        _ => vec![],
    }
}

pub(crate) fn dedup(steps: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in steps {
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

pub fn command_view(r: &Receipt) -> CommandView {
    let adapter_error = r.adapter_result.as_ref().and_then(|a| a.error.as_ref());
    let reasons: Vec<Reason> = r
        .policy
        .errors
        .iter()
        .map(Reason::from)
        // The adapter's message is the device's words, not the gateway's.
        .chain(adapter_error.map(|e| Reason {
            message: crate::untrusted::quote(&e.message),
            ..Reason::from(e)
        }))
        .collect();
    let mut next_steps: Vec<String> = Vec::new();
    for reason in &reasons {
        next_steps.extend(steps_for_reason(reason));
    }
    next_steps.extend(steps_for_state(r.state));

    let target = format!(
        "{} on {}",
        r.request.target.capability, r.request.target.device
    );
    let summary = match r.outcome {
        Outcome::Succeeded => format!("{target}: done and confirmed by reading the device back."),
        Outcome::Denied => format!(
            "{target}: refused ({}); nothing was sent to the device.",
            reasons
                .first()
                .map_or_else(|| "denied".into(), |e| e.message.clone())
        ),
        Outcome::NotExecuted => format!("{target}: not executed; nothing was sent to the device."),
        Outcome::Pending if r.state == CommandState::AwaitingApproval => {
            format!("{target}: waiting for human approval; nothing has been sent yet.")
        }
        Outcome::Pending => format!("{target}: in progress ({}).", name(&r.state)),
        Outcome::Uncertain => {
            format!("{target}: outcome UNKNOWN; it may or may not have taken effect.")
        }
        Outcome::Unverified => format!("{target}: device reports applied; not confirmed."),
        Outcome::Failed => format!("{target}: failed ({}).", name(&r.state)),
    };
    CommandView {
        command_id: r.command_id.clone(),
        device: r.request.target.device.clone(),
        capability: r.request.target.capability.clone(),
        state: name(&r.state),
        outcome: name(&r.outcome),
        confirmed: r.outcome == Outcome::Succeeded,
        reached_device: r.state.may_have_reached_device(),
        summary,
        reasons,
        verification: r.verification.as_ref().map(Verification::from),
        result: r
            .adapter_result
            .as_ref()
            .and_then(|a| a.result.as_ref())
            .filter(|m| !m.is_empty())
            .and_then(|m| serde_json::to_value(m).ok())
            .map(|mut v| {
                crate::untrusted::device_value(&mut v);
                v
            }),
        next_steps: dedup(next_steps),
    }
}

/// Whether an MCP client should flag the result as an error (`isError`). Anything that
/// did not achieve the request, or whose effect is unknown, is flagged, so a model never
/// mistakes it for success. Pending approval is not an error.
pub fn is_error(r: &Receipt) -> bool {
    matches!(
        r.outcome,
        Outcome::Denied | Outcome::Failed | Outcome::NotExecuted | Outcome::Uncertain
    )
}

/// The answer to "would this be allowed now?".
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CheckView {
    /// `allow`, `require_approval` or `deny`.
    pub decision: String,
    pub would_require_approval: bool,
    pub reasons: Vec<Reason>,
    /// Policy rules that matched.
    pub rules: Vec<String>,
    pub next_steps: Vec<String>,
    /// Always true: state can change before a real submission, which is checked again.
    pub advisory: bool,
}

pub fn check_view(p: &PreflightResult) -> CheckView {
    let reasons: Vec<Reason> = p.errors.iter().map(Reason::from).collect();
    let mut next_steps: Vec<String> = reasons.iter().flat_map(steps_for_reason).collect();
    match p.decision {
        Decision::Allow => next_steps.push(
            "Allowed right now. Submit it with submit_command; it is checked again then.".into(),
        ),
        Decision::RequireApproval => next_steps.push(
            "Allowed only with human approval. Submitting will hold it until a human decides."
                .into(),
        ),
        Decision::Deny => {}
    }
    CheckView {
        decision: name(&p.decision),
        would_require_approval: p.decision == Decision::RequireApproval,
        reasons,
        rules: p.rules.iter().map(|r| r.rule_id.clone()).collect(),
        next_steps: dedup(next_steps),
        advisory: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reason(code: &str, details: Value) -> Reason {
        Reason {
            code: code.into(),
            message: "m".into(),
            rule_id: Some("rule-1".into()),
            details: details.as_object().cloned(),
        }
    }

    #[test]
    fn slew_guidance_uses_the_declared_numbers() {
        let s = steps_for_reason(&reason(
            "command.slew_exceeded",
            json!({"max_change": 2.0, "per_ms": 1000, "unit": "V"}),
        ));
        assert!(s[0].contains("at most 2 V per 1000 ms"), "{s:?}");
    }

    #[test]
    fn range_guidance_names_the_range() {
        let s = steps_for_reason(&reason(
            "command.value.out_of_range",
            json!({"min": 0, "max": 12, "unit": "V"}),
        ));
        assert!(s[0].contains("(0..12 V)"), "{s:?}");
    }

    #[test]
    fn permission_denials_say_not_to_work_around() {
        let s = steps_for_reason(&reason("policy.denied.no_grant", json!({})));
        assert!(s[0].contains("Do not retry"));
    }

    #[test]
    fn uncertain_outcomes_forbid_resubmission() {
        let s = steps_for_state(CommandState::UnknownOutcome);
        assert!(s[0].contains("Do NOT resubmit"));
    }
}
