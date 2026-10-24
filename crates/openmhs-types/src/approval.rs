//! Approvals (spec/authorization.md §5, ADR-0017): a human's decision that one specific
//! command may run, as a durable record with its own small state machine.
//!
//! ```text
//! PENDING ──► APPROVED ──► CONSUMED
//!    │           │
//!    ├──► DENIED │
//!    ├──► EXPIRED ◄┤
//!    └──► REVOKED ◄┘
//! ```
//!
//! `DENIED`, `EXPIRED`, `REVOKED` and `CONSUMED` are final. An approval is spent exactly
//! once, atomically with the command moving on; it is never reopened.

use serde::{Deserialize, Serialize};

use crate::command::RequestOperation;
use crate::time::Timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
    Revoked,
    Consumed,
}

impl ApprovalStatus {
    pub const ALL: [ApprovalStatus; 6] = [
        Self::Pending,
        Self::Approved,
        Self::Denied,
        Self::Expired,
        Self::Revoked,
        Self::Consumed,
    ];

    /// The transition table, as data.
    pub fn can_transition_to(self, to: ApprovalStatus) -> bool {
        use ApprovalStatus::*;
        matches!(
            (self, to),
            (Pending, Approved | Denied | Expired | Revoked)
                | (Approved, Consumed | Expired | Revoked)
        )
    }

    pub fn is_final(self) -> bool {
        !matches!(self, Self::Pending | Self::Approved)
    }

    /// The stored and displayed name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Approved => "APPROVED",
            Self::Denied => "DENIED",
            Self::Expired => "EXPIRED",
            Self::Revoked => "REVOKED",
            Self::Consumed => "CONSUMED",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub id: String,
    pub status: ApprovalStatus,
    /// The principal that asked, and the only one that can spend this approval.
    pub requester: String,
    /// The credential the request came in on, so revoking it can withdraw what it asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester_credential: Option<String>,
    /// Who approved it (once approved), or who denied it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    pub device: String,
    pub capability: String,
    pub operation: RequestOperation,
    /// SHA-256 of the canonical command (requester, target, operation, value, params).
    /// The approval covers exactly this and nothing else.
    pub fingerprint: String,
    /// The command waiting on this approval, or the one that spent it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_at: Option<Timestamp>,
}

impl Approval {
    pub fn is_expired_at(&self, now: Timestamp) -> bool {
        now >= self.expires_at
    }
}

/// Deterministic JSON: object keys sorted, no whitespace, numbers as `serde_json` writes
/// them. Two equal values always produce the same bytes, which is what fingerprints and
/// signatures need.
pub fn canonical_json(value: &serde_json::Value) -> String {
    fn write(v: &serde_json::Value, out: &mut String) {
        match v {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                out.push('{');
                for (i, k) in keys.into_iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::Value::String(k.clone()).to_string());
                    out.push(':');
                    write(&map[k], out);
                }
                out.push('}');
            }
            serde_json::Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write(item, out);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut out = String::new();
    write(value, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn status() -> impl Strategy<Value = ApprovalStatus> {
        proptest::sample::select(ApprovalStatus::ALL.to_vec())
    }

    proptest! {
        /// Whatever sequence of requested transitions is attempted, an approval that
        /// reaches a final state never leaves it, and CONSUMED is only reachable from
        /// APPROVED.
        #[test]
        fn final_states_are_final(steps in proptest::collection::vec(status(), 0..20)) {
            let mut current = ApprovalStatus::Pending;
            let mut was_approved = false;
            for to in steps {
                let before = current;
                if current.can_transition_to(to) {
                    current = to;
                }
                if before.is_final() {
                    prop_assert_eq!(current, before);
                }
                if current == ApprovalStatus::Consumed {
                    prop_assert!(was_approved);
                }
                was_approved |= current == ApprovalStatus::Approved;
            }
        }

        #[test]
        fn nothing_reaches_consumed_except_from_approved(from in status()) {
            prop_assert_eq!(
                from.can_transition_to(ApprovalStatus::Consumed),
                from == ApprovalStatus::Approved
            );
        }
    }

    #[test]
    fn nothing_goes_back_to_pending() {
        for s in ApprovalStatus::ALL {
            assert!(!s.can_transition_to(ApprovalStatus::Pending), "{s:?}");
        }
    }

    #[test]
    fn canonical_json_ignores_key_order_and_whitespace() {
        let a: serde_json::Value =
            serde_json::from_str(r#"{"b": 1, "a": {"y": [1, 2], "x": "s"}}"#).unwrap();
        let b: serde_json::Value =
            serde_json::from_str(r#"{"a":{"x":"s","y":[1,2]},"b":1}"#).unwrap();
        assert_eq!(canonical_json(&a), canonical_json(&b));
        assert_eq!(canonical_json(&a), r#"{"a":{"x":"s","y":[1,2]},"b":1}"#);
    }
}
