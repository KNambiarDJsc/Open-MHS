//! Leases (spec/topology.md, ADR-0018): exclusive use of devices and shared resources for
//! a while. Mirrors the API's lease object.

use serde::{Deserialize, Serialize};

use crate::graph::Claim;
use crate::time::Timestamp;

/// The longest a lease can be asked to live.
pub const MAX_LEASE_TTL_MS: u64 = 24 * 60 * 60 * 1_000;

/// Where a lease is in its life. Only `Active` holds anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LeaseStatus {
    Active,
    Released,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub id: String,
    /// The principal that holds it. Its commands on claimed devices are not blocked.
    pub holder: String,
    /// The credential it was taken with. Revoking that credential releases the lease.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder_credential: Option<String>,
    /// What was asked for.
    pub devices: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<String>,
    /// What that claims, by the graph's rule, fixed when the lease was taken.
    pub claim: Claim,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The plan this lease was taken for, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_by: Option<String>,
}

impl Lease {
    pub fn status_at(&self, now: Timestamp) -> LeaseStatus {
        if self.released_at.is_some() {
            LeaseStatus::Released
        } else if now >= self.expires_at {
            LeaseStatus::Expired
        } else {
            LeaseStatus::Active
        }
    }

    pub fn is_active_at(&self, now: Timestamp) -> bool {
        self.status_at(now) == LeaseStatus::Active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lease_is_active_until_released_or_due() {
        let mut l = Lease {
            id: "lse_1".into(),
            holder: "agent:a".into(),
            holder_credential: None,
            devices: vec!["d".into()],
            resources: vec![],
            claim: Claim::default(),
            reason: None,
            plan_id: None,
            created_at: Timestamp::from_millis(0),
            expires_at: Timestamp::from_millis(100),
            released_at: None,
            released_by: None,
        };
        assert_eq!(l.status_at(Timestamp::from_millis(99)), LeaseStatus::Active);
        assert_eq!(
            l.status_at(Timestamp::from_millis(100)),
            LeaseStatus::Expired
        );
        l.released_at = Some(Timestamp::from_millis(50));
        assert_eq!(
            l.status_at(Timestamp::from_millis(10)),
            LeaseStatus::Released
        );
    }
}
