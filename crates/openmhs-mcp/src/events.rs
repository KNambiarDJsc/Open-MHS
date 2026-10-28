//! The state fabric for a model (ADR-0023): watch a measurement, then poll for what changed.
//!
//! Events are notifications. The views say when each reading was measured and how good it
//! is, and leave "usable for a decision" to a fresh `read_state` or to the gateway's own
//! checks. Text a device wrote is quoted as device text, as everywhere else.

use openmhs_types::event::{Event, EventBatch, EventBody, Watch};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{guidance, standard_formats, untrusted};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = standard_formats)]
pub struct WatchArgs {
    /// Device id from list_devices.
    pub device: String,
    /// A capability whose operations include "subscribe" (describe_device shows them).
    pub capability: String,
    /// How often to sample, in ms (at least 200).
    pub interval_ms: u64,
    /// How long to keep sampling, in ms (default 300000, at most 3600000).
    #[serde(default)]
    pub ttl_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelWatchArgs {
    /// The watch id from watch_state.
    pub watch_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = standard_formats)]
pub struct PollArgs {
    /// `next` from the previous poll. Omit to get only what happens from now on.
    #[serde(default)]
    pub after: Option<u64>,
    /// `stream` from the previous poll, so a gateway restart is noticed (`reset`).
    #[serde(default)]
    pub stream: Option<String>,
    /// Only these devices (`*` patterns allowed).
    #[serde(default)]
    pub devices: Vec<String>,
    /// Only these kinds: "observation", "availability", "command".
    #[serde(default)]
    pub kinds: Vec<String>,
    /// Wait up to this long for an event if there is none yet (at most 30000).
    #[serde(default)]
    pub wait_ms: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(transform = standard_formats)]
pub struct WatchView {
    pub watch_id: String,
    pub device: String,
    pub capability: String,
    pub interval_ms: u64,
    pub expires_at: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WatchList {
    pub watches: Vec<WatchView>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(transform = standard_formats)]
pub struct EventView {
    pub seq: u64,
    /// `observation`, `availability` or `command`.
    pub kind: String,
    pub device: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    /// Observation: the value, e.g. {"value": 2.01, "unit": "V"}.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// Observation: when the device measured it (RFC 3339).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_at: Option<String>,
    /// Observation: `GOOD`, `UNCERTAIN` or `BAD`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// Observation: why it was taken (`watch`, `read`, `check`, `verify`, `reconcile`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    /// The command an observation was taken for, or the command that changed state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_id: Option<String>,
    /// Command: its new state. Availability: the new availability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Availability: why, in the adapter's words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(transform = standard_formats)]
pub struct EventsOutput {
    pub stream: String,
    /// Pass as `after` next time.
    pub next: u64,
    /// Events you fell too far behind to receive. Not zero: read_state for current values.
    pub missed: u64,
    /// The gateway restarted since your last poll; earlier cursors are void.
    pub reset: bool,
    pub events: Vec<EventView>,
}

pub fn watch_view(w: &Watch) -> WatchView {
    WatchView {
        watch_id: w.id.clone(),
        device: w.device.clone(),
        capability: w.capability.clone(),
        interval_ms: w.interval_ms,
        expires_at: w.expires_at.to_rfc3339(),
    }
}

fn event_view(e: &Event) -> EventView {
    let mut v = EventView {
        seq: e.seq,
        kind: guidance::name(&e.body.kind()),
        device: e.body.device().to_owned(),
        capability: None,
        value: None,
        measured_at: None,
        quality: None,
        cause: None,
        command_id: None,
        state: None,
        reason: None,
    };
    match &e.body {
        EventBody::Observation {
            capability,
            state,
            cause,
            command_id,
            ..
        } => {
            let mut value = serde_json::to_value(&state.value).unwrap_or(Value::Null);
            untrusted::device_value(&mut value);
            v.capability = Some(capability.clone());
            v.value = Some(value);
            v.measured_at = Some(state.timestamp.to_rfc3339());
            v.quality = Some(guidance::name(&state.quality));
            v.cause = Some(guidance::name(cause));
            v.command_id = command_id.clone();
        }
        EventBody::Availability { to, reason, .. } => {
            v.state = Some(to.as_str().to_owned());
            v.reason = reason.as_deref().map(untrusted::quote);
        }
        EventBody::Command {
            command_id,
            capability,
            state,
            ..
        } => {
            v.capability = Some(capability.clone());
            v.command_id = Some(command_id.clone());
            v.state = Some(guidance::name(state));
        }
    }
    v
}

pub fn events_output(b: &EventBatch) -> EventsOutput {
    EventsOutput {
        stream: b.stream.clone(),
        next: b.next,
        missed: b.missed,
        reset: b.reset,
        events: b.events.iter().map(event_view).collect(),
    }
}
