//! Who is calling and what their credential allows (spec/authorization.md §7).
//!
//! A [`Caller`] is built by whoever verified a credential, never from a request body, and
//! travels with the call into the engine. Its scopes are a ceiling: a call needs a scope
//! that permits it *and* a grant that allows it. Neither widens the other.

use openmhs_types::descriptor::Operation;
use serde::{Deserialize, Serialize};

use crate::glob;

/// What kind of principal a credential belongs to. Derived from the principal id, so a
/// credential cannot claim to be a human while carrying an agent's name or the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    Human,
    Agent,
    Service,
}

impl PrincipalKind {
    /// The kind a principal id implies: `agent:*` is an agent, `service:*` a service,
    /// anything else a human.
    pub fn of(principal: &str) -> Self {
        match principal.split_once(':').map(|(kind, _)| kind) {
            Some("agent") => Self::Agent,
            Some("service") => Self::Service,
            _ => Self::Human,
        }
    }
}

/// How the caller proved who they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    /// A bearer credential issued by `tokens.create`.
    Credential,
    /// The operator token the gateway wrote to its runtime file at start.
    OperatorRuntime,
    /// Code linked into the same process (tests, conformance, embedding). No credential
    /// was checked: the program is trusted by construction.
    InProcess,
}

/// One entry of a credential's ceiling: which devices and capabilities (`*` patterns, as in
/// grants) and which operations. A credential with no scopes can do nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub device: String,
    pub capability: String,
    pub operations: Vec<Operation>,
}

impl Scope {
    /// Every device, capability and operation. Used for the operator and in-process
    /// callers, whose limits come from policy alone.
    pub fn any() -> Self {
        Self {
            device: "*".into(),
            capability: "*".into(),
            operations: vec![
                Operation::Read,
                Operation::Write,
                Operation::Subscribe,
                Operation::Invoke,
            ],
        }
    }

    pub fn permits(&self, device: &str, capability: &str, operation: Operation) -> bool {
        self.operations.contains(&operation)
            && glob::matches(&self.device, device)
            && glob::matches(&self.capability, capability)
    }
}

/// A verified caller. Fields are private so that the only ways to get one are the
/// constructors below, each named for how the identity was established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    principal: String,
    kind: PrincipalKind,
    credential_id: Option<String>,
    method: AuthMethod,
    scopes: Vec<Scope>,
}

impl Caller {
    /// A caller whose bearer credential the gateway has just verified.
    pub fn authenticated(
        principal: impl Into<String>,
        credential_id: impl Into<String>,
        method: AuthMethod,
        scopes: Vec<Scope>,
    ) -> Self {
        let principal = principal.into();
        Self {
            kind: PrincipalKind::of(&principal),
            principal,
            credential_id: Some(credential_id.into()),
            method,
            scopes,
        }
    }

    /// A caller inside the process, with no credential to check. The scope ceiling is
    /// open; grants still apply. Never build one from data a client sent.
    pub fn in_process(principal: impl Into<String>) -> Self {
        let principal = principal.into();
        Self {
            kind: PrincipalKind::of(&principal),
            principal,
            credential_id: None,
            method: AuthMethod::InProcess,
            scopes: vec![Scope::any()],
        }
    }

    pub fn principal(&self) -> &str {
        &self.principal
    }

    pub fn kind(&self) -> PrincipalKind {
        self.kind
    }

    pub fn credential_id(&self) -> Option<&str> {
        self.credential_id.as_deref()
    }

    pub fn method(&self) -> AuthMethod {
        self.method
    }

    pub fn scopes(&self) -> &[Scope] {
        &self.scopes
    }

    /// Whether some scope of the credential covers this access.
    pub fn in_scope(&self, device: &str, capability: &str, operation: Operation) -> bool {
        self.scopes
            .iter()
            .any(|s| s.permits(device, capability, operation))
    }

    /// Whether some scope reaches `device` at all.
    pub fn in_scope_device(&self, device: &str) -> bool {
        self.scopes.iter().any(|s| glob::matches(&s.device, device))
    }

    /// What the engine keeps to act for this caller later.
    pub fn record(&self) -> CallerRecord {
        CallerRecord {
            principal: self.principal.clone(),
            credential_id: self.credential_id.clone(),
            method: self.method,
            scopes: self.scopes.clone(),
        }
    }

    /// The caller a plan was submitted by, to continue it after an approval or a
    /// restart. Only for records the engine wrote itself; never for data a client sent.
    pub fn from_record(r: &CallerRecord) -> Self {
        Self {
            kind: PrincipalKind::of(&r.principal),
            principal: r.principal.clone(),
            credential_id: r.credential_id.clone(),
            method: r.method,
            scopes: r.scopes.clone(),
        }
    }
}

/// A verified caller as stored by the engine alongside work it carries on with later
/// (plans). Credential revocation ends that work (ADR-0018).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallerRecord {
    pub principal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    pub method: AuthMethod,
    pub scopes: Vec<Scope>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(device: &str, capability: &str, ops: &[Operation]) -> Scope {
        Scope {
            device: device.into(),
            capability: capability.into(),
            operations: ops.to_vec(),
        }
    }

    #[test]
    fn kind_follows_the_principal_id() {
        assert_eq!(PrincipalKind::of("agent:lab"), PrincipalKind::Agent);
        assert_eq!(
            PrincipalKind::of("service:scheduler"),
            PrincipalKind::Service
        );
        assert_eq!(PrincipalKind::of("operator:local"), PrincipalKind::Human);
        assert_eq!(PrincipalKind::of("alice"), PrincipalKind::Human);
    }

    #[test]
    fn no_scopes_means_no_access() {
        let c = Caller::authenticated("agent:a", "tok_1", AuthMethod::Credential, vec![]);
        assert!(!c.in_scope("sim.psu-01", "power.output.enabled", Operation::Read));
    }

    #[test]
    fn a_scope_limits_device_capability_and_operation() {
        let c = Caller::authenticated(
            "agent:a",
            "tok_1",
            AuthMethod::Credential,
            vec![scope("sim.psu-*", "power.output.*", &[Operation::Read])],
        );
        assert!(c.in_scope("sim.psu-01", "power.output.voltage", Operation::Read));
        assert!(!c.in_scope("sim.psu-01", "power.output.voltage", Operation::Write));
        assert!(!c.in_scope("sim.axis-01", "power.output.voltage", Operation::Read));
        assert!(!c.in_scope("sim.psu-01", "motion.position", Operation::Read));
    }

    #[test]
    fn in_process_callers_have_an_open_ceiling_and_say_so() {
        let c = Caller::in_process("agent:test");
        assert!(c.in_scope("any", "thing", Operation::Invoke));
        assert_eq!(c.method(), AuthMethod::InProcess);
        assert_eq!(c.credential_id(), None);
    }
}
