//! Device availability (ADR-0019): whether the gateway can reach a device now, and whether
//! it is free. Mirrors the API's `devices.status` object.
//!
//! Availability is the gateway's observation, not a guarantee: a device marked `ONLINE`
//! can stop answering at any moment, and every command still fails closed on its own
//! reads and readback. It exists so that callers can tell "switched off" from "refused",
//! and so that a device that does not answer is refused before anything is sent to it.

use serde::{Deserialize, Serialize};

use crate::time::Timestamp;

/// Whether the device answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Availability {
    /// The last check reached the device and it reported itself healthy.
    Online,
    /// The device answers but reports a problem. Commands are not refused for it.
    Degraded,
    /// The device did not answer the last check. Commands, except stops, are refused
    /// before anything is sent (`device.unavailable`).
    Offline,
}

impl Availability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Online => "ONLINE",
            Self::Degraded => "DEGRADED",
            Self::Offline => "OFFLINE",
        }
    }
}

/// The lease that holds a device, as status shows it. Leases are visible to every caller
/// (spec/topology.md §Leases), so this discloses nothing `leases.list` does not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseHold {
    pub id: String,
    pub holder: String,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceStatus {
    pub device: String,
    pub availability: Availability,
    /// Why the device is degraded or offline, as its adapter put it. Device-written text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// When the device entered this availability.
    pub since: Timestamp,
    /// When the gateway last checked it, by a probe or by a call that reached it.
    pub checked_at: Timestamp,
    /// A state-changing command is running on the device now.
    pub busy: bool,
    /// The active lease that claims the device, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<LeaseHold>,
}
