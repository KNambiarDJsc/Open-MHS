//! Grant-based authorization (spec/authorization.md §3–4).

use openmhs_types::ErrorObject;
use openmhs_types::descriptor::{Capability, Operation, SideEffect};
use openmhs_types::receipt::{Decision, RuleMatch};
use serde::{Deserialize, Serialize};

use crate::glob;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Allow,
    RequireApproval,
    Deny,
}

impl From<Effect> for Decision {
    fn from(e: Effect) -> Self {
        match e {
            Effect::Allow => Decision::Allow,
            Effect::RequireApproval => Decision::RequireApproval,
            Effect::Deny => Decision::Deny,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub id: String,
    /// Principal id or pattern.
    pub principal: String,
    /// Device id or pattern.
    pub device: String,
    /// Capability name or pattern.
    pub capability: String,
    pub operations: Vec<Operation>,
    pub effect: Effect,
    /// For `allow`/`require_approval`: only applies to capabilities whose side-effect
    /// class is at most this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_side_effect: Option<SideEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Grant {
    fn matches(&self, principal: &str, device: &str, capability: &str, op: Operation) -> bool {
        glob::matches(&self.principal, principal)
            && glob::matches(&self.device, device)
            && glob::matches(&self.capability, capability)
            && self.operations.contains(&op)
    }

    fn covers_side_effect(&self, side_effect: SideEffect) -> bool {
        self.max_side_effect.is_none_or(|max| side_effect <= max)
    }

    fn rule_match(&self) -> RuleMatch {
        RuleMatch {
            rule_id: self.id.clone(),
            effect: self.effect.into(),
            reason: self.reason.clone(),
        }
    }
}

/// Operator policy. The default policy has no grants and therefore denies everything.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default)]
    pub grants: Vec<Grant>,
    /// Principals allowed to approve commands that require approval.
    #[serde(default)]
    pub approvers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthzDecision {
    pub decision: Decision,
    /// The grants that produced the decision.
    pub rules: Vec<RuleMatch>,
    /// Set when the decision is `Deny`.
    pub error: Option<ErrorObject>,
}

impl Policy {
    pub fn authorize(
        &self,
        principal: &str,
        device: &str,
        capability: &Capability,
        op: Operation,
    ) -> AuthzDecision {
        let matching: Vec<&Grant> = self
            .grants
            .iter()
            .filter(|g| g.matches(principal, device, &capability.name, op))
            .collect();

        let denies: Vec<&Grant> = matching
            .iter()
            .copied()
            .filter(|g| g.effect == Effect::Deny)
            .collect();
        if !denies.is_empty() {
            let rules: Vec<RuleMatch> = denies.iter().map(|g| g.rule_match()).collect();
            let error = ErrorObject::new(
                "policy.denied.explicit",
                format!("denied by rule '{}'", denies[0].id),
            )
            .rule(denies[0].id.clone());
            return AuthzDecision {
                decision: Decision::Deny,
                rules,
                error: Some(error),
            };
        }

        let (covering, too_narrow): (Vec<&Grant>, Vec<&Grant>) = matching
            .iter()
            .copied()
            .partition(|g| g.covers_side_effect(capability.side_effect));

        let approvals: Vec<&Grant> = covering
            .iter()
            .copied()
            .filter(|g| g.effect == Effect::RequireApproval)
            .collect();
        let allows: Vec<&Grant> = covering
            .iter()
            .copied()
            .filter(|g| g.effect == Effect::Allow)
            .collect();

        if !approvals.is_empty() {
            let rules = approvals
                .iter()
                .chain(allows.iter())
                .map(|g| g.rule_match())
                .collect();
            return AuthzDecision {
                decision: Decision::RequireApproval,
                rules,
                error: None,
            };
        }
        if !allows.is_empty() {
            let rules = allows.iter().map(|g| g.rule_match()).collect();
            return AuthzDecision {
                decision: Decision::Allow,
                rules,
                error: None,
            };
        }
        if let Some(g) = too_narrow.first() {
            let error = ErrorObject::new(
                "policy.denied.side_effect",
                format!(
                    "rule '{}' does not cover side effect '{:?}' of '{}'",
                    g.id, capability.side_effect, capability.name
                )
                .to_lowercase(),
            )
            .rule(g.id.clone());
            let rules = too_narrow.iter().map(|g| g.rule_match()).collect();
            return AuthzDecision {
                decision: Decision::Deny,
                rules,
                error: Some(error),
            };
        }
        AuthzDecision {
            decision: Decision::Deny,
            rules: Vec::new(),
            error: Some(ErrorObject::new(
                "policy.denied.no_grant",
                format!(
                    "no grant allows '{principal}' to {} '{}' on '{device}'",
                    format!("{op:?}").to_lowercase(),
                    capability.name
                ),
            )),
        }
    }

    pub fn is_approver(&self, principal: &str) -> bool {
        self.approvers.iter().any(|a| glob::matches(a, principal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmhs_types::descriptor::{CapabilityKind, ValueSpec, ValueType};

    fn cap(name: &str, side_effect: SideEffect) -> Capability {
        Capability {
            name: name.into(),
            version: None,
            kind: CapabilityKind::Property,
            description: None,
            component: None,
            operations: vec![Operation::Read, Operation::Write],
            value: Some(ValueSpec {
                ty: ValueType::Boolean,
                quantity: None,
                unit: None,
                range: None,
                values: None,
                max_length: None,
                description: None,
            }),
            params: None,
            result: None,
            payload: None,
            side_effect,
            idempotent: None,
            stop: false,
            freshness: None,
            rate_limit: None,
            slew: None,
            preconditions: vec![],
            verification: None,
            timing: None,
            extensions: None,
        }
    }

    fn grant(id: &str, cap: &str, ops: &[Operation], effect: Effect) -> Grant {
        Grant {
            id: id.into(),
            principal: "agent.a".into(),
            device: "dev-*".into(),
            capability: cap.into(),
            operations: ops.to_vec(),
            effect,
            max_side_effect: None,
            reason: None,
        }
    }

    #[test]
    fn empty_policy_denies_everything() {
        let d = Policy::default().authorize(
            "agent.a",
            "dev-1",
            &cap("x", SideEffect::None),
            Operation::Read,
        );
        assert_eq!(d.decision, Decision::Deny);
        assert_eq!(d.error.unwrap().code, "policy.denied.no_grant");
    }

    #[test]
    fn allow_requires_matching_operation() {
        let p = Policy {
            grants: vec![grant("g1", "x", &[Operation::Read], Effect::Allow)],
            approvers: vec![],
        };
        let c = cap("x", SideEffect::Reversible);
        assert_eq!(
            p.authorize("agent.a", "dev-1", &c, Operation::Read)
                .decision,
            Decision::Allow
        );
        assert_eq!(
            p.authorize("agent.a", "dev-1", &c, Operation::Write)
                .decision,
            Decision::Deny
        );
        assert_eq!(
            p.authorize("agent.b", "dev-1", &c, Operation::Read)
                .decision,
            Decision::Deny
        );
    }

    #[test]
    fn deny_overrides_allow_and_approval_overrides_allow() {
        let c = cap("x", SideEffect::Reversible);
        let p = Policy {
            grants: vec![
                grant("allow", "*", &[Operation::Write], Effect::Allow),
                grant("deny", "x", &[Operation::Write], Effect::Deny),
            ],
            approvers: vec![],
        };
        let d = p.authorize("agent.a", "dev-1", &c, Operation::Write);
        assert_eq!(d.decision, Decision::Deny);
        assert_eq!(d.error.unwrap().code, "policy.denied.explicit");

        let p = Policy {
            grants: vec![
                grant("allow", "*", &[Operation::Write], Effect::Allow),
                grant("ask", "x", &[Operation::Write], Effect::RequireApproval),
            ],
            approvers: vec![],
        };
        let d = p.authorize("agent.a", "dev-1", &c, Operation::Write);
        assert_eq!(d.decision, Decision::RequireApproval);
        assert_eq!(d.rules[0].rule_id, "ask");
    }

    #[test]
    fn max_side_effect_limits_grants() {
        let mut g = grant("g", "*", &[Operation::Write], Effect::Allow);
        g.max_side_effect = Some(SideEffect::Reversible);
        let p = Policy {
            grants: vec![g],
            approvers: vec![],
        };
        assert_eq!(
            p.authorize(
                "agent.a",
                "dev-1",
                &cap("safe", SideEffect::Reversible),
                Operation::Write
            )
            .decision,
            Decision::Allow
        );
        let d = p.authorize(
            "agent.a",
            "dev-1",
            &cap("move", SideEffect::Hazardous),
            Operation::Write,
        );
        assert_eq!(d.decision, Decision::Deny);
        assert_eq!(d.error.unwrap().code, "policy.denied.side_effect");
    }
}
