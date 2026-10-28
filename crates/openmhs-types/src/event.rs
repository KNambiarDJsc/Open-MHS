//! The state fabric's events (ADR-0023): what the gateway saw happen, in order.
//!
//! An event is a notification, not a source of truth. An observation event carries the
//! reading exactly as the adapter returned it (timestamp, quality, source), so a consumer
//! can judge its age and quality the same way the engine does; the journal, receipts and a
//! fresh read remain the evidence. Events live in a bounded in-memory buffer and are lost on
//! restart, and a consumer that falls behind is told how many it missed, never silently.

use serde::{Deserialize, Serialize};

use crate::availability::Availability;
use crate::lifecycle::CommandState;
use crate::state::StateValue;
use crate::time::Timestamp;

/// Why the gateway took a reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationCause {
    /// An audited `state.read`.
    Read,
    /// A live check before a command (preconditions, slew, freshness).
    Check,
    /// Readback while verifying a command.
    Verify,
    /// Readback while reconciling an `UNKNOWN_OUTCOME` command.
    Reconcile,
    /// Seeding a twin for a plan dry run.
    Preflight,
    /// A sample taken for a watch (`watches.create`).
    Watch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventBody {
    /// The gateway read a value from a device.
    Observation {
        device: String,
        capability: String,
        state: StateValue,
        cause: ObservationCause,
        /// The command this reading was taken for (verification, reconciliation).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command_id: Option<String>,
    },
    /// A device's availability changed (ADR-0019).
    Availability {
        device: String,
        from: Availability,
        to: Availability,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// A command moved to a new state.
    Command {
        command_id: String,
        principal: String,
        device: String,
        capability: String,
        state: CommandState,
    },
}

impl EventBody {
    pub fn device(&self) -> &str {
        match self {
            Self::Observation { device, .. }
            | Self::Availability { device, .. }
            | Self::Command { device, .. } => device,
        }
    }

    pub fn kind(&self) -> EventKind {
        match self {
            Self::Observation { .. } => EventKind::Observation,
            Self::Availability { .. } => EventKind::Availability,
            Self::Command { .. } => EventKind::Command,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Observation,
    Availability,
    Command,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Increases by one per event published by this gateway process. Restarts begin again
    /// at 1 with a new `stream` id.
    pub seq: u64,
    /// When the gateway published it (not when a reading was measured: see `state`).
    pub at: Timestamp,
    #[serde(flatten)]
    pub body: EventBody,
}

/// One page of events (`events.poll`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventBatch {
    /// Identifies this gateway process's stream. A cursor from another stream (a restart)
    /// is not valid here; start again from `next`.
    pub stream: String,
    pub events: Vec<Event>,
    /// Pass as `after` to continue.
    pub next: u64,
    /// Events after the cursor that were dropped from the bounded buffer before this poll
    /// read them, so cannot be delivered. Zero unless the consumer fell behind.
    pub missed: u64,
    /// Whether the cursor belonged to another stream (the gateway restarted).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reset: bool,
}

/// A periodic sampler of one capability, for one principal (`watches.create`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Watch {
    pub id: String,
    pub principal: String,
    pub device: String,
    pub capability: String,
    pub interval_ms: u64,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
}
