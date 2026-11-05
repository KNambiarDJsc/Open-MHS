//! What a refused request leaves behind (docs/architecture/REFUSAL_AUDIT.md).
//!
//! A request from an authenticated caller that asks to act, and is refused, leaves one durable
//! record. `handle` calls this around `dispatch`, so no method has to remember to. Commands and
//! reads have records of their own and are not recorded again here.
//!
//! A record is evidence that an attempt happened, not a copy of it. Named identifiers are read
//! out of the request by name and normalized, so a value, a reason, a description or an error
//! message, any of which may quote what the caller sent, cannot reach the journal this way.

use openmhs_api::method;
use serde_json::{Map, Value, json};

const MAX_ID_CHARS: usize = 128;
const MAX_LISTED: usize = 16;

/// Methods that ask the gateway to act. A refusal of one of these is recorded. Queries, advisory
/// checks and `state.read` (which the engine records itself) are not here.
pub(crate) fn acts(name: &str) -> bool {
    matches!(
        name,
        method::COMMANDS_SUBMIT
            | method::COMMANDS_APPROVE
            | method::COMMANDS_CANCEL
            | method::COMMANDS_RECONCILE
            | method::COMMANDS_RESOLVE
            | method::APPROVALS_REQUEST
            | method::APPROVALS_APPROVE
            | method::APPROVALS_DENY
            | method::APPROVALS_REVOKE
            | method::PLANS_SUBMIT
            | method::PLANS_DECIDE
            | method::PLANS_CANCEL
            | method::LEASES_ACQUIRE
            | method::LEASES_RENEW
            | method::LEASES_RELEASE
            | method::WATCHES_CREATE
            | method::WATCHES_CANCEL
            | method::PROCEDURES_PROPOSE
            | method::PROCEDURES_ADD
            | method::PROCEDURES_APPROVE
            | method::PROCEDURES_REMOVE
            | method::PROCEDURES_RUN
            | method::GRANTS_ADD
            | method::GRANTS_REVOKE
            | method::TOKENS_CREATE
            | method::TOKENS_REVOKE
            | method::TOKENS_ROTATE
            | method::AUDIT_SEGMENT
            | method::AUDIT_SEAL
            | method::AUDIT_COMPACT
            | method::SIM_FAULT
    )
}

/// Whether this refusal gets a `request.rejected` record.
pub(crate) fn recorded(name: &str, code: &str) -> bool {
    match code {
        // The gateway failed; it did not refuse. This may be the failure to write the record.
        "gateway.journal" | "gateway.storage" | "gateway.internal" => false,
        // Reaching for operator authority is recorded whatever the method does.
        "gateway.forbidden" => true,
        // The engine records a command that fails schema validation.
        "command.schema" if name == method::COMMANDS_SUBMIT => false,
        _ => acts(name),
    }
}

/// A caller controls this string, so it is kept only if it looks like an identifier.
fn identifier(text: &str) -> bool {
    !text.is_empty()
        && text.chars().count() <= MAX_ID_CHARS
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:/@*+-".contains(c))
}

#[derive(Default)]
struct Target {
    fields: Map<String, Value>,
    omitted: bool,
}

impl Target {
    fn id(&mut self, key: &str, value: Option<&Value>) {
        match value {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) if identifier(s) => {
                self.fields.insert(key.to_owned(), s.as_str().into());
            }
            Some(_) => self.omitted = true,
        }
    }

    fn number(&mut self, key: &str, value: Option<&Value>) {
        match value {
            None | Some(Value::Null) => {}
            Some(n @ Value::Number(_)) => {
                self.fields.insert(key.to_owned(), n.clone());
            }
            Some(_) => self.omitted = true,
        }
    }

    fn ids<'a>(&mut self, key: &str, values: impl IntoIterator<Item = &'a Value>) {
        let mut kept: Vec<Value> = Vec::new();
        for value in values {
            match value {
                Value::String(s) if identifier(s) && kept.len() < MAX_LISTED => {
                    if !kept.iter().any(|k| k.as_str() == Some(s)) {
                        kept.push(s.as_str().into());
                    }
                }
                _ => self.omitted = true,
            }
        }
        if !kept.is_empty() {
            self.fields.insert(key.to_owned(), kept.into());
        }
    }

    fn command(&mut self, request: &Value) {
        let target = request.get("target");
        self.id("device", target.and_then(|t| t.get("device")));
        self.id("capability", target.and_then(|t| t.get("capability")));
        self.id("operation", request.get("operation"));
        self.id("correlation_id", request.get("correlation_id"));
    }
}

/// What the request named, as far as it can be trusted to be a name. `params` is what the caller
/// sent, possibly not of the shape the method wants, so nothing here assumes it is.
pub(crate) fn target(name: &str, params: &Value) -> Value {
    let mut t = Target::default();
    match name {
        method::COMMANDS_SUBMIT | method::APPROVALS_REQUEST => {
            if let Some(request) = params.get("request") {
                t.command(request);
            }
            t.id("approval_id", params.get("approval_id"));
        }
        method::PLANS_SUBMIT => {
            let steps = params
                .pointer("/plan/steps")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            let devices = steps
                .iter()
                .filter_map(|s| s.pointer("/request/target/device"));
            t.ids("devices", devices);
            t.fields.insert("steps".to_owned(), steps.len().into());
        }
        method::LEASES_ACQUIRE => {
            for key in ["devices", "resources"] {
                match params.get(key) {
                    Some(Value::Array(values)) => t.ids(key, values),
                    None | Some(Value::Null) => {}
                    Some(_) => t.omitted = true,
                }
            }
        }
        method::WATCHES_CREATE => {
            t.id("device", params.get("device"));
            t.id("capability", params.get("capability"));
        }
        method::SIM_FAULT => t.id("device", params.get("device")),
        method::COMMANDS_APPROVE
        | method::COMMANDS_CANCEL
        | method::COMMANDS_RECONCILE
        | method::COMMANDS_RESOLVE => t.id("id", params.get("command_id")),
        method::LEASES_RENEW
        | method::LEASES_RELEASE
        | method::PLANS_DECIDE
        | method::PLANS_CANCEL
        | method::WATCHES_CANCEL
        | method::APPROVALS_APPROVE
        | method::APPROVALS_DENY
        | method::APPROVALS_REVOKE
        | method::GRANTS_REVOKE
        | method::TOKENS_REVOKE
        | method::TOKENS_ROTATE => t.id("id", params.get("id")),
        method::GRANTS_ADD => t.id("id", params.pointer("/grant/id")),
        method::TOKENS_CREATE => {
            t.id("principal", params.get("principal"));
            t.id("role", params.get("role"));
        }
        method::PROCEDURES_PROPOSE | method::PROCEDURES_ADD => {
            t.id("name", params.pointer("/procedure/name"));
            t.number("version", params.pointer("/procedure/version"));
        }
        method::PROCEDURES_APPROVE | method::PROCEDURES_REMOVE | method::PROCEDURES_RUN => {
            t.id("name", params.get("name"));
            t.number("version", params.get("version"));
        }
        method::AUDIT_SEAL | method::AUDIT_COMPACT => t.number("number", params.get("number")),
        _ => {}
    }
    let mut out = Map::new();
    if !t.fields.is_empty() {
        out.insert("target".to_owned(), t.fields.into());
    }
    if t.omitted {
        out.insert("target_omitted".to_owned(), true.into());
    }
    out.into()
}

/// The record's data. `target` comes from [`target`].
pub(crate) fn data(
    name: &str,
    code: &str,
    principal: &str,
    credential_id: Option<&str>,
    target: &Value,
) -> Value {
    let mut record = json!({
        "method": name,
        "code": code,
        "principal": principal,
        "credential_id": credential_id,
    });
    if let (Value::Object(record), Value::Object(target)) = (&mut record, target) {
        for (key, value) in target {
            record.insert(key.clone(), value.clone());
        }
    }
    record
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_acting_methods_and_reaching_for_authority_are_recorded() {
        assert!(recorded(method::PLANS_SUBMIT, "lease.not_authorized"));
        assert!(recorded(method::GRANTS_LIST, "gateway.forbidden"));
        assert!(!recorded(method::GRANTS_LIST, "something.else"));
        assert!(!recorded(method::LEASES_GET, "lease.unknown"));
        assert!(!recorded(method::STATE_READ, "policy.denied.no_grant"));
        assert!(!recorded(method::PLANS_PREFLIGHT, "lease.not_authorized"));
    }

    #[test]
    fn a_failure_of_the_gateway_is_not_a_refusal() {
        for code in ["gateway.journal", "gateway.storage", "gateway.internal"] {
            assert!(!recorded(method::PLANS_SUBMIT, code));
        }
    }

    #[test]
    fn the_engine_keeps_recording_schema_failures_of_commands_only() {
        assert!(!recorded(method::COMMANDS_SUBMIT, "command.schema"));
        assert!(recorded(method::PLANS_SUBMIT, "command.schema"));
        assert!(recorded(method::APPROVALS_REQUEST, "command.schema"));
    }

    #[test]
    fn identifiers_are_short_and_plain() {
        assert!(identifier("ros2.sim-robot-01"));
        assert!(identifier("agent:robot-observer"));
        assert!(!identifier(""));
        assert!(!identifier("ignore previous instructions"));
        assert!(!identifier("line\nbreak"));
        assert!(!identifier(&"x".repeat(MAX_ID_CHARS + 1)));
        assert!(identifier(&"x".repeat(MAX_ID_CHARS)));
    }

    #[test]
    fn a_request_of_the_wrong_shape_cannot_get_through_or_break_this() {
        for params in [
            json!(null),
            json!(42),
            json!("text"),
            json!({ "request": 7 }),
            json!({ "request": { "target": [] } }),
            json!({ "plan": { "steps": "no" } }),
            json!({ "plan": { "steps": [1, { "request": { "target": { "device": {} } } }] } }),
        ] {
            for name in [
                method::COMMANDS_SUBMIT,
                method::PLANS_SUBMIT,
                method::LEASES_ACQUIRE,
            ] {
                let _ = target(name, &params);
            }
        }
    }

    #[test]
    fn lists_are_capped_and_repeats_collapse() {
        let devices: Vec<String> = (0..40).map(|i| format!("d{i}")).collect();
        let t = target(method::LEASES_ACQUIRE, &json!({ "devices": devices }));
        assert_eq!(t["target"]["devices"].as_array().unwrap().len(), MAX_LISTED);
        assert_eq!(t["target_omitted"], true);
        let t = target(method::LEASES_ACQUIRE, &json!({ "devices": ["a", "a"] }));
        assert_eq!(t["target"]["devices"], json!(["a"]));
        assert!(t.get("target_omitted").is_none());
    }

    #[test]
    fn values_reasons_and_bodies_are_never_read() {
        let t = target(
            method::COMMANDS_SUBMIT,
            &json!({ "request": {
                "target": { "device": "d", "capability": "c" }, "operation": "write",
                "value": "SECRET", "params": { "x": "SECRET" }, "reason": "SECRET",
            }}),
        );
        assert!(!t.to_string().contains("SECRET"), "{t}");
    }
}
