//! The requirement registry. `spec/conformance.md` lists the same requirements in the
//! same order; a test keeps the two identical.

use openmhs_types::descriptor::ConformanceLevel::{self, L0, L1, L2, L3};

use crate::fixture::Fault;

/// One conformance requirement. Ids are stable: they are only ever added, and a released
/// id never changes meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requirement {
    pub id: &'static str,
    pub level: ConformanceLevel,
    /// The fault the fixture must inject to test it, if any.
    pub fault: Option<Fault>,
    pub text: &'static str,
}

const fn req(
    id: &'static str,
    level: ConformanceLevel,
    fault: Option<Fault>,
    text: &'static str,
) -> Requirement {
    Requirement {
        id,
        level,
        fault,
        text,
    }
}

pub const REQUIREMENTS: &[Requirement] = &[
    req("L0.describe", L0, None, "`describe` returns a descriptor"),
    req(
        "L0.descriptor",
        L0,
        None,
        "The descriptor passes schema and semantic validation",
    ),
    req(
        "L0.adapter_id",
        L0,
        None,
        "`adapter_id` equals the descriptor's `adapter.id`",
    ),
    req(
        "L0.describe_stable",
        L0,
        None,
        "Describing twice returns the same descriptor",
    ),
    req(
        "L0.connect",
        L0,
        None,
        "`connect` succeeds and the device registers with the engine",
    ),
    req(
        "L0.health",
        L0,
        None,
        "`health` reports healthy after connecting",
    ),
    req("L0.shutdown", L0, None, "`shutdown` succeeds"),
    req("L1.read", L1, None, "Every readable property can be read"),
    req(
        "L1.read.value",
        L1,
        None,
        "Readings have the declared type and unit, and stay within the declared range or values",
    ),
    req(
        "L1.read.timestamp",
        L1,
        None,
        "Readings are not timestamped in the future, and are as fresh as the descriptor declares",
    ),
    req(
        "L1.read.quality",
        L1,
        None,
        "Readings from a healthy device have `GOOD` quality",
    ),
    req(
        "L1.subscribe",
        L1,
        None,
        "Subscriptions deliver updates. Deferred: the adapter contract has no subscriptions yet",
    ),
    req(
        "L1.disconnect.read",
        L1,
        Some(Fault::LinkDown),
        "With the link down, a read fails with `adapter.disconnected` or `adapter.timeout`, or returns a reading that is not `GOOD`",
    ),
    req(
        "L1.disconnect.health",
        L1,
        Some(Fault::LinkDown),
        "With the link down, `health` does not report healthy",
    ),
    req(
        "L1.reconnect",
        L1,
        Some(Fault::LinkDown),
        "When the link returns, health and `GOOD` readings come back without re-registering",
    ),
    req(
        "L2.command",
        L2,
        None,
        "A valid write or invoke is carried out within the declared `timing.timeout_ms`",
    ),
    req(
        "L2.verification",
        L2,
        None,
        "Verification behaves as declared: readback confirms the change (`VERIFIED`); `mode: none` claims nothing (`UNVERIFIABLE`)",
    ),
    req(
        "L2.reject",
        L2,
        Some(Fault::Refuse),
        "A command the device refuses ends `REJECTED` with `adapter.rejected`",
    ),
    req(
        "L2.malformed",
        L2,
        Some(Fault::Garbled),
        "An answer that cannot be parsed ends `UNKNOWN_OUTCOME` with `adapter.malformed_response`",
    ),
    req(
        "L2.timeout",
        L2,
        Some(Fault::NoResponse),
        "A command that gets no answer ends `UNKNOWN_OUTCOME`, never `FAILED` or a success",
    ),
    req(
        "L2.lost_ack",
        L2,
        Some(Fault::LostAck),
        "A command applied without an answer ends `UNKNOWN_OUTCOME`, and reconciling by readback finds it applied",
    ),
    req(
        "L2.no_retry",
        L2,
        Some(Fault::NoResponse),
        "The adapter never retries a non-idempotent command: each reaches the device at most once",
    ),
    req(
        "L2.recovery",
        L2,
        None,
        "After faults clear, commands are carried out again without re-registering",
    ),
    req(
        "L3.leases",
        L3,
        None,
        "Takes part in multi-device plans with resource leases. Defined in M9",
    ),
];

/// Look up a requirement by id.
pub fn requirement(id: &str) -> Option<&'static Requirement> {
    REQUIREMENTS.iter().find(|r| r.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_levels_ascend() {
        let mut seen = std::collections::BTreeSet::new();
        let mut last = L0;
        for r in REQUIREMENTS {
            assert!(seen.insert(r.id), "duplicate {}", r.id);
            assert!(r.level >= last, "{} is out of level order", r.id);
            assert!(r.id.starts_with(&r.level.to_string()), "{}", r.id);
            last = r.level;
        }
    }
}
