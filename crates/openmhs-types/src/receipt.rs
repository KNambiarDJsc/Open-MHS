//! Command receipts (`receipt.schema.json`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::command::CommandRequest;
use crate::error::ErrorObject;
use crate::lifecycle::{CommandState, Outcome};
use crate::state::StateValue;
use crate::time::Timestamp;
use crate::value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny,
    RequireApproval,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleMatch {
    pub rule_id: String,
    pub effect: Decision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRecord {
    pub decision: Decision,
    pub rules: Vec<RuleMatch>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ErrorObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledged: Option<bool>,
    /// An opaque identifier the device or its server gave the accepted work, as reported by
    /// the adapter (for example a LabThings invocation id). For matching an unresolved
    /// command to the work it started; never used to decide anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_applied: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObject>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationMode {
    Readback,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationRecord {
    pub mode: VerificationMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<StateValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionRecord {
    pub state: CommandState,
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub command_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    pub principal: String,
    pub request: CommandRequest,
    pub state: CommandState,
    pub outcome: Outcome,
    pub created_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_at: Option<Timestamp>,
    pub transitions: Vec<TransitionRecord>,
    pub policy: PolicyRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_result: Option<AdapterResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<VerificationRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_hash: Option<String>,
}

impl Receipt {
    /// A new receipt in `CREATED`. The policy record starts as a pending denial and is
    /// filled in during validation.
    pub fn created(
        command_id: String,
        principal: String,
        request: CommandRequest,
        created_at: Timestamp,
    ) -> Self {
        let deadline_at = Some(created_at.plus_ms(request.deadline_ms()));
        Self {
            command_id,
            correlation_id: request.correlation_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            principal,
            request,
            state: CommandState::Created,
            outcome: CommandState::Created.outcome(),
            created_at,
            deadline_at,
            transitions: vec![TransitionRecord {
                state: CommandState::Created,
                at: created_at,
                detail: None,
            }],
            policy: PolicyRecord {
                decision: Decision::Deny,
                rules: Vec::new(),
                errors: Vec::new(),
                approved_by: None,
            },
            adapter_result: None,
            verification: None,
            audit_hash: None,
        }
    }

    /// Detail of the most recent transition.
    pub fn last_detail(&self) -> Option<&str> {
        self.transitions.last().and_then(|t| t.detail.as_deref())
    }
}

/// What would happen if a request were submitted now (`commands.preflight`).
///
/// Advisory only: device state, policy and rate-limit history can change before a real
/// submission, which is checked again from scratch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreflightResult {
    pub decision: Decision,
    pub rules: Vec<RuleMatch>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ErrorObject>,
    pub checked_at: Timestamp,
}
