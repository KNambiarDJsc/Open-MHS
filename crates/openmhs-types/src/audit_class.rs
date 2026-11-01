//! Event classes of the audit log (strategy RFC §3, RFC 0002 milestone 6).
//!
//! Every audit kind belongs to exactly one class. The class says what may be done with the
//! record under pressure: `Critical` and `Security` records are never sampled, aggregated
//! or dropped; only `Telemetry` may be aggregated, and only when an operator has opted a
//! device and capability in (`openmhs serve --telemetry`). A kind this table does not know
//! is `Critical` until it is classified, and `tests/event_classes.rs` fails when the code
//! writes a kind that is not in the table.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EventClass {
    /// Physical consequence and its lifecycle: commands, approvals, plans, leases, recovery.
    Critical,
    /// Evidence about who tried what: refusals, credentials, grants, policy.
    Security,
    /// How the gateway ran: start and stop, registration, availability, preflights.
    Operational,
    /// Routine observation: an allowed, successful read.
    Telemetry,
}

impl EventClass {
    pub fn name(self) -> &'static str {
        match self {
            Self::Critical => "CRITICAL",
            Self::Security => "SECURITY",
            Self::Operational => "OPERATIONAL",
            Self::Telemetry => "TELEMETRY",
        }
    }

    /// Whether a record of this class may ever be aggregated or dropped to save space.
    pub fn may_be_aggregated(self) -> bool {
        self == Self::Telemetry
    }
}

/// The audit kinds written as literals, and the class of each.
const KINDS: &[(&str, EventClass)] = &[
    // Critical
    ("command.created", EventClass::Critical),
    ("command.transition", EventClass::Critical),
    ("gateway.recovered", EventClass::Critical),
    ("sim.fault", EventClass::Critical),
    ("procedure.run", EventClass::Critical),
    // The chain's own structure: segment seals and compactions (RFC 0002).
    ("audit.segment_sealed", EventClass::Critical),
    ("audit.segment_compacted", EventClass::Critical),
    // Security
    ("read.denied", EventClass::Security),
    ("request.rejected", EventClass::Security),
    ("credential.rejected", EventClass::Security),
    ("policy.updated", EventClass::Security),
    ("topology.set", EventClass::Security),
    // What may be run changes who can cause what.
    ("procedure.added", EventClass::Security),
    ("procedure.approved", EventClass::Security),
    ("procedure.proposed", EventClass::Security),
    ("procedure.removed", EventClass::Security),
    // Operational
    ("gateway.started", EventClass::Operational),
    ("gateway.stopped", EventClass::Operational),
    ("device.registered", EventClass::Operational),
    ("device.availability", EventClass::Operational),
    ("command.preflight", EventClass::Operational),
    ("plan.preflight", EventClass::Operational),
    ("watch.created", EventClass::Operational),
    ("watch.ended", EventClass::Operational),
    // The summary that stands in for aggregated reads is kept as long as Operational.
    ("read.summary", EventClass::Operational),
    // Telemetry
    ("read", EventClass::Telemetry),
];

/// Families whose kinds are generated from a state name (`approval.consumed`,
/// `plan.waiting_approval`, `lease.expired`, `token.rotated`).
const FAMILIES: &[(&str, EventClass)] = &[
    ("approval.", EventClass::Critical),
    ("plan.", EventClass::Critical),
    ("lease.", EventClass::Critical),
    ("grant.", EventClass::Security),
    ("token.", EventClass::Security),
];

/// The class of `kind`, or `None` if it has not been classified.
pub fn classify(kind: &str) -> Option<EventClass> {
    KINDS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, c)| *c)
        .or_else(|| {
            FAMILIES
                .iter()
                .find(|(prefix, _)| kind.starts_with(prefix))
                .map(|(_, c)| *c)
        })
}

/// The class of `kind`: unclassified kinds are `Critical`, so they are kept in full.
pub fn class_of(kind: &str) -> EventClass {
    classify(kind).unwrap_or(EventClass::Critical)
}

/// Every literal kind in the table, for tests.
pub fn known_kinds() -> impl Iterator<Item = &'static str> {
    KINDS.iter().map(|(k, _)| *k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_kinds_are_critical_and_unclassified() {
        assert_eq!(classify("something.new"), None);
        assert_eq!(class_of("something.new"), EventClass::Critical);
    }

    #[test]
    fn only_telemetry_may_be_aggregated() {
        for kind in known_kinds() {
            assert_eq!(class_of(kind).may_be_aggregated(), kind == "read", "{kind}");
        }
    }

    #[test]
    fn a_kind_is_in_the_table_once() {
        let mut seen = std::collections::HashSet::new();
        for kind in known_kinds() {
            assert!(seen.insert(kind), "{kind} is listed twice");
        }
    }

    #[test]
    fn families_do_not_override_a_listed_kind() {
        assert_eq!(class_of("plan.preflight"), EventClass::Operational);
        assert_eq!(class_of("plan.succeeded"), EventClass::Critical);
    }
}
