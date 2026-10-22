//! Command lifecycle state machine (spec/lifecycle.md, ADR-0002).
//!
//! The transition table is defined once, here, as data.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CommandState {
    Created,
    Validating,
    Denied,
    AwaitingApproval,
    Approved,
    Expired,
    Cancelled,
    Dispatched,
    Accepted,
    Rejected,
    Failed,
    TimedOut,
    Applied,
    Verified,
    VerificationFailed,
    Unverifiable,
    UnknownOutcome,
    ReconciledApplied,
    ReconciledNotApplied,
}

/// Coarse outcome class an agent can plan around.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Not finished yet.
    Pending,
    /// Applied and confirmed by readback (or by reconciliation).
    Succeeded,
    /// Device reported it applied, but no independent readback confirmed it.
    Unverified,
    /// Device or adapter reported failure, or readback contradicted the request.
    Failed,
    /// Policy refused it. Nothing was sent to the device.
    Denied,
    /// Cancelled or expired before dispatch. Nothing was sent to the device.
    NotExecuted,
    /// It may or may not have taken effect. Needs reconciliation.
    Uncertain,
}

use CommandState::*;

/// Every legal transition. Anything not listed is illegal.
pub const TRANSITIONS: &[(CommandState, &[CommandState])] = &[
    (Created, &[Validating]),
    (Validating, &[Denied, AwaitingApproval, Approved]),
    (AwaitingApproval, &[Approved, Denied, Expired, Cancelled]),
    (Approved, &[Dispatched, Cancelled, Expired]),
    // UNKNOWN_OUTCOME directly from DISPATCHED/ACCEPTED is crash recovery (spec/lifecycle.md §4).
    (
        Dispatched,
        &[Accepted, Rejected, Failed, TimedOut, UnknownOutcome],
    ),
    (Accepted, &[Applied, Failed, TimedOut, UnknownOutcome]),
    (Applied, &[Verified, VerificationFailed, Unverifiable]),
    (TimedOut, &[UnknownOutcome]),
    (UnknownOutcome, &[ReconciledApplied, ReconciledNotApplied]),
];

impl CommandState {
    pub const ALL: [CommandState; 19] = [
        Created,
        Validating,
        Denied,
        AwaitingApproval,
        Approved,
        Expired,
        Cancelled,
        Dispatched,
        Accepted,
        Rejected,
        Failed,
        TimedOut,
        Applied,
        Verified,
        VerificationFailed,
        Unverifiable,
        UnknownOutcome,
        ReconciledApplied,
        ReconciledNotApplied,
    ];

    /// States reachable in one step from `self`.
    pub fn successors(self) -> &'static [CommandState] {
        TRANSITIONS
            .iter()
            .find(|(from, _)| *from == self)
            .map_or(&[], |(_, to)| to)
    }

    pub fn can_transition_to(self, next: CommandState) -> bool {
        self.successors().contains(&next)
    }

    /// No further transitions are possible.
    pub fn is_terminal(self) -> bool {
        self.successors().is_empty()
    }

    pub fn outcome(self) -> Outcome {
        match self {
            Created | Validating | AwaitingApproval | Approved | Dispatched | Accepted
            | Applied => Outcome::Pending,
            Verified | ReconciledApplied => Outcome::Succeeded,
            Unverifiable => Outcome::Unverified,
            Rejected | Failed | VerificationFailed | ReconciledNotApplied => Outcome::Failed,
            Denied => Outcome::Denied,
            Expired | Cancelled => Outcome::NotExecuted,
            TimedOut | UnknownOutcome => Outcome::Uncertain,
        }
    }

    /// Whether the command may have been sent to the device in this state.
    pub fn may_have_reached_device(self) -> bool {
        !matches!(
            self,
            Created | Validating | Denied | AwaitingApproval | Approved | Expired | Cancelled
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn table_has_unique_sources_and_no_self_loops() {
        let mut seen = HashSet::new();
        for (from, tos) in TRANSITIONS {
            assert!(seen.insert(*from), "{from:?} listed twice");
            assert!(!tos.contains(from), "{from:?} loops to itself");
        }
    }

    #[test]
    fn exhaustive_transition_matrix() {
        // The full 19x19 matrix, spelled out independently of TRANSITIONS.
        let legal: HashSet<(CommandState, CommandState)> = [
            (Created, Validating),
            (Validating, Denied),
            (Validating, AwaitingApproval),
            (Validating, Approved),
            (AwaitingApproval, Approved),
            (AwaitingApproval, Denied),
            (AwaitingApproval, Expired),
            (AwaitingApproval, Cancelled),
            (Approved, Dispatched),
            (Approved, Cancelled),
            (Approved, Expired),
            (Dispatched, Accepted),
            (Dispatched, Rejected),
            (Dispatched, Failed),
            (Dispatched, TimedOut),
            (Dispatched, UnknownOutcome),
            (Accepted, Applied),
            (Accepted, Failed),
            (Accepted, TimedOut),
            (Accepted, UnknownOutcome),
            (Applied, Verified),
            (Applied, VerificationFailed),
            (Applied, Unverifiable),
            (TimedOut, UnknownOutcome),
            (UnknownOutcome, ReconciledApplied),
            (UnknownOutcome, ReconciledNotApplied),
        ]
        .into_iter()
        .collect();

        for a in CommandState::ALL {
            for b in CommandState::ALL {
                assert_eq!(
                    a.can_transition_to(b),
                    legal.contains(&(a, b)),
                    "{a:?} -> {b:?}"
                );
            }
        }
    }

    #[test]
    fn every_state_reachable_from_created() {
        let mut reached = HashSet::from([Created]);
        let mut frontier = vec![Created];
        while let Some(s) = frontier.pop() {
            for n in s.successors() {
                if reached.insert(*n) {
                    frontier.push(*n);
                }
            }
        }
        assert_eq!(reached.len(), CommandState::ALL.len());
    }

    #[test]
    fn timeout_never_means_failure() {
        assert!(!TimedOut.can_transition_to(Failed));
        assert_eq!(TimedOut.outcome(), Outcome::Uncertain);
        assert_eq!(UnknownOutcome.outcome(), Outcome::Uncertain);
        assert!(!UnknownOutcome.is_terminal());
    }

    #[test]
    fn acknowledgement_is_not_success() {
        assert_eq!(Accepted.outcome(), Outcome::Pending);
        assert_eq!(Applied.outcome(), Outcome::Pending);
        assert_eq!(Unverifiable.outcome(), Outcome::Unverified);
    }

    #[test]
    fn terminal_states_are_exactly_the_documented_ones() {
        let terminal: HashSet<_> = CommandState::ALL
            .into_iter()
            .filter(|s| s.is_terminal())
            .collect();
        let expected = HashSet::from([
            Denied,
            Expired,
            Cancelled,
            Rejected,
            Failed,
            Verified,
            VerificationFailed,
            Unverifiable,
            ReconciledApplied,
            ReconciledNotApplied,
        ]);
        assert_eq!(terminal, expected);
    }

    #[test]
    fn states_before_dispatch_never_reached_device() {
        for s in CommandState::ALL {
            if !s.may_have_reached_device() {
                assert!(
                    matches!(
                        s.outcome(),
                        Outcome::Pending | Outcome::Denied | Outcome::NotExecuted
                    ),
                    "{s:?}"
                );
            }
        }
    }

    #[test]
    fn serde_names_match_schema() {
        let schema: serde_json::Value =
            serde_json::from_str(crate::schema::COMMON_SCHEMA).expect("common schema parses");
        let names: Vec<String> = schema["$defs"]["command_state"]["enum"]
            .as_array()
            .expect("enum")
            .iter()
            .map(|v| v.as_str().expect("str").to_owned())
            .collect();
        let ours: Vec<String> = CommandState::ALL
            .iter()
            .map(|s| {
                serde_json::to_value(s)
                    .expect("ser")
                    .as_str()
                    .expect("str")
                    .to_owned()
            })
            .collect();
        assert_eq!(names, ours);

        let outcomes: Vec<&str> = schema["$defs"]["outcome"]["enum"]
            .as_array()
            .expect("enum")
            .iter()
            .map(|v| v.as_str().expect("str"))
            .collect();
        for s in CommandState::ALL {
            let o = serde_json::to_value(s.outcome()).expect("ser");
            assert!(
                outcomes.contains(&o.as_str().expect("str")),
                "{o} missing from schema"
            );
        }
    }
}
