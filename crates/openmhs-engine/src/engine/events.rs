//! The state fabric (ADR-0023): a bounded, ordered stream of what the gateway saw happen,
//! and watches that sample a capability so that the stream has something to carry.
//!
//! - **Pull, not push.** Consumers poll with a cursor (`after`) and may wait for new events.
//!   A consumer that falls behind loses the oldest events and is told how many (`missed`);
//!   the gateway never blocks, buffers without bound, or drops silently.
//! - **Not authoritative.** An observation event carries the adapter's reading unchanged
//!   (timestamp, quality, source). Commands still decide on fresh reads of their own, and the
//!   journal and receipts remain the evidence.
//! - **Authorized as `subscribe`.** An observation is delivered only for a capability whose
//!   descriptor declares `subscribe`, to a caller whose credential scopes and grants allow
//!   `subscribe` on it, decided when the event is delivered. Availability events follow the
//!   credential's device scope; command events go to their principal (all, for the operator).
//! - **Ephemeral.** Events and watches live in memory and end with the process; a cursor
//!   from an earlier process is answered with `reset`.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use openmhs_policy::Caller;
use openmhs_types::descriptor::{CapabilityKind, Operation};
use openmhs_types::event::{Event, EventBatch, EventBody, EventKind, ObservationCause, Watch};
use openmhs_types::receipt::Decision;
use openmhs_types::{ErrorObject, Receipt, StateValue, Timestamp};
use tracing::warn;

use super::{Device, Engine, lock, scope_denied};
use crate::error::EngineError;

/// Events kept in memory by default. At a few hundred bytes each, a few megabytes.
pub const DEFAULT_EVENT_CAPACITY: usize = 8_192;
/// The most events one poll returns.
pub const MAX_EVENTS_PER_POLL: usize = 500;
/// The longest a poll waits for a new event.
pub const MAX_POLL_WAIT_MS: u64 = 30_000;
/// The shortest interval a watch may sample at.
pub const MIN_WATCH_INTERVAL_MS: u64 = 200;
/// The longest a watch lives before it must be created again.
pub const MAX_WATCH_TTL_MS: u64 = 3_600_000;
/// A watch's lifetime when none is asked for.
pub const DEFAULT_WATCH_TTL_MS: u64 = 300_000;
/// Watches one principal may hold at once.
pub const MAX_WATCHES_PER_PRINCIPAL: usize = 16;
/// Watches the gateway runs at once.
pub const MAX_WATCHES: usize = 256;

pub(super) struct Bus {
    state: std::sync::Mutex<Ring>,
    notify: tokio::sync::Notify,
    /// This process's stream; cursors are only meaningful within it.
    stream: String,
}

struct Ring {
    events: VecDeque<Event>,
    /// The seq of the newest event published, 0 before the first.
    last: u64,
    capacity: usize,
}

impl Bus {
    pub fn new(capacity: usize) -> Self {
        Self {
            state: std::sync::Mutex::new(Ring {
                events: VecDeque::new(),
                last: 0,
                capacity: capacity.max(1),
            }),
            notify: tokio::sync::Notify::new(),
            stream: uuid::Uuid::now_v7().to_string(),
        }
    }

    fn publish(&self, at: Timestamp, body: EventBody) {
        {
            let mut ring = lock(&self.state);
            ring.last += 1;
            let seq = ring.last;
            ring.events.push_back(Event { seq, at, body });
            while ring.events.len() > ring.capacity {
                ring.events.pop_front();
            }
        }
        self.notify.notify_waiters();
    }
}

/// Where to read from and how much: a cursor, a filter, a page size and a wait.
#[derive(Debug, Clone)]
pub struct EventQuery {
    /// The stream the cursor came from; another one (a restart) resets the cursor.
    pub stream: Option<String>,
    /// Return events with a greater `seq`. `None`: only events from now on.
    pub after: Option<u64>,
    pub filter: EventFilter,
    /// At most this many, capped at [`MAX_EVENTS_PER_POLL`].
    pub limit: usize,
    /// Wait this long for an event when there is none, capped at [`MAX_POLL_WAIT_MS`].
    pub wait_ms: u64,
}

impl Default for EventQuery {
    fn default() -> Self {
        Self {
            stream: None,
            after: None,
            filter: EventFilter::default(),
            limit: 50,
            wait_ms: 0,
        }
    }
}

/// What a consumer wants from the stream.
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    /// `*` patterns; empty means every device.
    pub devices: Vec<String>,
    /// Empty means every kind.
    pub kinds: Vec<EventKind>,
}

impl EventFilter {
    fn wants(&self, body: &EventBody) -> bool {
        (self.kinds.is_empty() || self.kinds.contains(&body.kind()))
            && (self.devices.is_empty()
                || self
                    .devices
                    .iter()
                    .any(|p| openmhs_policy::glob_matches(p, body.device())))
    }
}

pub(super) struct WatchEntry {
    watch: Watch,
    credential_id: Option<String>,
    task: tokio::task::AbortHandle,
}

#[derive(Default)]
pub(super) struct Watches {
    by_id: std::sync::Mutex<HashMap<String, WatchEntry>>,
}

impl Engine {
    pub(super) fn publish_observation(
        &self,
        device: &str,
        capability: &str,
        state: &StateValue,
        cause: ObservationCause,
        command_id: Option<&str>,
    ) {
        self.inner.bus.publish(
            self.now(),
            EventBody::Observation {
                device: device.to_owned(),
                capability: capability.to_owned(),
                state: state.clone(),
                cause,
                command_id: command_id.map(str::to_owned),
            },
        );
    }

    pub(super) fn publish_command(&self, r: &Receipt) {
        self.inner.bus.publish(
            self.now(),
            EventBody::Command {
                command_id: r.command_id.clone(),
                principal: r.principal.clone(),
                device: r.request.target.device.clone(),
                capability: r.request.target.capability.clone(),
                state: r.state,
            },
        );
    }

    pub(super) fn publish(&self, body: EventBody) {
        self.inner.bus.publish(self.now(), body);
    }

    /// Whether `caller` may receive a stream of `capability` on `device`: the descriptor
    /// declares `subscribe` on a property, the credential's scope covers it, and a grant
    /// allows it. The same answer for an event and for creating a watch.
    fn subscribe_check(
        &self,
        caller: &Caller,
        device_id: &str,
        capability: &str,
    ) -> Result<Arc<Device>, ErrorObject> {
        // Scope first, so a credential cannot probe which devices exist.
        if !caller.in_scope(device_id, capability, Operation::Subscribe) {
            return Err(scope_denied(
                caller,
                device_id,
                capability,
                Operation::Subscribe,
            ));
        }
        let device = self.device(device_id).ok_or_else(|| {
            ErrorObject::new(
                "command.device.unknown",
                format!("device '{device_id}' is not registered"),
            )
        })?;
        let Some(cap) = device.descriptor.capability(capability) else {
            return Err(ErrorObject::new(
                "command.capability.unknown",
                format!("'{device_id}' has no capability '{capability}'"),
            ));
        };
        if !cap.supports(Operation::Subscribe) || cap.kind != CapabilityKind::Property {
            return Err(ErrorObject::new(
                "command.operation.unsupported",
                format!(
                    "'{capability}' does not declare subscribe; only properties whose descriptor \
                     declares it can be streamed"
                ),
            ));
        }
        let authz =
            self.policy()
                .authorize(caller.principal(), device_id, cap, Operation::Subscribe);
        if authz.decision != Decision::Allow {
            return Err(authz.error.unwrap_or_else(|| {
                ErrorObject::new("policy.denied.no_grant", "subscribe not allowed")
            }));
        }
        Ok(device)
    }

    /// Events after `query.after` that `caller` may see and the filter wants, waiting up
    /// to `query.wait_ms` for one if there are none yet. No cursor starts at the newest
    /// event, so only what happens next is returned. `all_commands` shows every principal's command
    /// events (the operator).
    pub async fn poll_events(
        &self,
        caller: &Caller,
        all_commands: bool,
        query: &EventQuery,
    ) -> EventBatch {
        let (stream, after, filter) = (query.stream.as_deref(), query.after, &query.filter);
        let (limit, wait_ms) = (query.limit, query.wait_ms);
        let bus = &self.inner.bus;
        let reset = stream.is_some_and(|s| s != bus.stream);
        let mut cursor = match (reset, after) {
            (false, Some(a)) => a,
            _ => lock(&bus.state).last,
        };
        let limit = limit.clamp(1, MAX_EVENTS_PER_POLL);
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(wait_ms.min(MAX_POLL_WAIT_MS));
        let mut missed = 0;
        let mut allowed: HashMap<(String, String), bool> = HashMap::new();
        loop {
            // Register for the wake-up before looking, so a publish in between is not lost.
            let notified = bus.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            let candidates: Vec<Event> = {
                let ring = lock(&bus.state);
                let oldest = ring.events.front().map_or(ring.last + 1, |e| e.seq);
                if oldest > cursor + 1 {
                    missed += oldest - cursor - 1;
                    cursor = oldest - 1;
                }
                ring.events
                    .iter()
                    .filter(|e| e.seq > cursor)
                    .cloned()
                    .collect()
            };
            let mut events = Vec::new();
            for e in candidates {
                if events.len() == limit {
                    break;
                }
                cursor = e.seq;
                if filter.wants(&e.body)
                    && self.event_visible(caller, all_commands, &e, &mut allowed)
                {
                    events.push(e);
                }
            }
            if !events.is_empty() || tokio::time::Instant::now() >= deadline {
                return EventBatch {
                    stream: bus.stream.clone(),
                    events,
                    next: cursor,
                    missed,
                    reset,
                };
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                // Timed out: one last look happens at the top of the loop.
            }
        }
    }

    fn event_visible(
        &self,
        caller: &Caller,
        all_commands: bool,
        e: &Event,
        allowed: &mut HashMap<(String, String), bool>,
    ) -> bool {
        match &e.body {
            EventBody::Observation {
                device, capability, ..
            } => *allowed
                .entry((device.clone(), capability.clone()))
                .or_insert_with(|| self.subscribe_check(caller, device, capability).is_ok()),
            EventBody::Availability { device, .. } => caller.in_scope_device(device),
            EventBody::Command { principal, .. } => all_commands || principal == caller.principal(),
        }
    }

    // ------------------------------------------------------------------ watches

    /// Sample `capability` every `interval_ms` for `ttl_ms`, publishing each reading as an
    /// observation event. Authorized as `subscribe` when created and again before every
    /// sample, so a revoked grant stops it. The creation and the end are audited; samples
    /// are not, since they reach only callers allowed to subscribe. A second watch by the
    /// same principal on the same capability replaces the first.
    pub fn create_watch(
        &self,
        caller: &Caller,
        device_id: &str,
        capability: &str,
        interval_ms: u64,
        ttl_ms: Option<u64>,
    ) -> Result<Watch, EngineError> {
        let device = self
            .subscribe_check(caller, device_id, capability)
            .map_err(EngineError::Denied)?;
        let ttl_ms = ttl_ms.unwrap_or(DEFAULT_WATCH_TTL_MS);
        if interval_ms < MIN_WATCH_INTERVAL_MS || ttl_ms == 0 || ttl_ms > MAX_WATCH_TTL_MS {
            return Err(EngineError::InvalidRequest(vec![
                ErrorObject::new(
                    "watch.invalid",
                    format!(
                        "a watch samples at most every {MIN_WATCH_INTERVAL_MS} ms and lives at most \
                         {MAX_WATCH_TTL_MS} ms"
                    ),
                )
                .detail("min_interval_ms", MIN_WATCH_INTERVAL_MS)
                .detail("max_ttl_ms", MAX_WATCH_TTL_MS),
            ]));
        }
        if let Some(e) = self.storage_refusal() {
            return Err(EngineError::Denied(e));
        }
        let principal = caller.principal().to_owned();
        let now = self.now();
        let watch = Watch {
            id: uuid::Uuid::now_v7().to_string(),
            principal: principal.clone(),
            device: device_id.to_owned(),
            capability: capability.to_owned(),
            interval_ms,
            created_at: now,
            expires_at: now.plus_ms(ttl_ms),
        };
        {
            let mut by_id = lock(&self.inner.watches.by_id);
            let replaced: Vec<String> = by_id
                .values()
                .filter(|w| {
                    w.watch.principal == principal
                        && w.watch.device == device_id
                        && w.watch.capability == capability
                })
                .map(|w| w.watch.id.clone())
                .collect();
            for id in &replaced {
                if let Some(old) = by_id.remove(id) {
                    old.task.abort();
                    self.inner.journal.append_audit(
                        now,
                        "watch.ended",
                        device_id,
                        &serde_json::json!({
                            "watch_id": id, "principal": principal,
                            "capability": capability, "reason": format!("replaced by {}", watch.id),
                        }),
                    )?;
                }
            }
            let mine = by_id
                .values()
                .filter(|w| w.watch.principal == principal)
                .count();
            if mine >= MAX_WATCHES_PER_PRINCIPAL || by_id.len() >= MAX_WATCHES {
                return Err(EngineError::Denied(
                    ErrorObject::new(
                        "watch.limit",
                        format!(
                            "too many watches (at most {MAX_WATCHES_PER_PRINCIPAL} per principal and \
                             {MAX_WATCHES} in all); cancel one first"
                        ),
                    )
                    .detail("per_principal", MAX_WATCHES_PER_PRINCIPAL)
                    .detail("total", MAX_WATCHES),
                ));
            }
            self.inner.journal.append_audit(
                now,
                "watch.created",
                device_id,
                &serde_json::json!({
                    "watch_id": watch.id, "principal": principal,
                    "credential_id": caller.credential_id(), "capability": capability,
                    "interval_ms": interval_ms, "expires_at": watch.expires_at,
                }),
            )?;
            let task = tokio::spawn(
                self.clone()
                    .run_watch(caller.clone(), watch.clone(), device),
            );
            by_id.insert(
                watch.id.clone(),
                WatchEntry {
                    watch: watch.clone(),
                    credential_id: caller.credential_id().map(str::to_owned),
                    task: task.abort_handle(),
                },
            );
        }
        Ok(watch)
    }

    async fn run_watch(self, caller: Caller, watch: Watch, device: Arc<Device>) {
        let every = Duration::from_millis(watch.interval_ms);
        let reason = loop {
            tokio::time::sleep(every).await;
            if self.now() >= watch.expires_at {
                break "expired".to_owned();
            }
            if let Err(e) = self.subscribe_check(&caller, &watch.device, &watch.capability) {
                break format!("no longer allowed ({})", e.code);
            }
            // A failed sample is not the end: the device may come back. Its availability is
            // recorded by the read itself.
            let _ = self
                .read_device(&device, &watch.capability, ObservationCause::Watch, None)
                .await;
        };
        self.end_watch(&watch.id, &reason);
    }

    /// Remove a watch and audit why it ended. Only the first end of a watch is recorded.
    fn end_watch(&self, id: &str, reason: &str) {
        let Some(entry) = lock(&self.inner.watches.by_id).remove(id) else {
            return;
        };
        entry.task.abort();
        if let Err(e) = self.inner.journal.append_audit(
            self.now(),
            "watch.ended",
            &entry.watch.device,
            &serde_json::json!({
                "watch_id": id, "principal": entry.watch.principal,
                "capability": entry.watch.capability, "reason": reason,
            }),
        ) {
            warn!(watch = %id, error = %e, "could not record the end of a watch");
        }
    }

    /// Watches `caller` holds, or every watch for the operator (`all`).
    pub fn watches(&self, caller: &Caller, all: bool) -> Vec<Watch> {
        let mut list: Vec<Watch> = lock(&self.inner.watches.by_id)
            .values()
            .filter(|w| all || w.watch.principal == caller.principal())
            .map(|w| w.watch.clone())
            .collect();
        list.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        list
    }

    /// Cancel a watch: its principal's own, or any for the operator (`any`).
    pub fn cancel_watch(&self, caller: &Caller, id: &str, any: bool) -> Result<Watch, EngineError> {
        let watch = lock(&self.inner.watches.by_id)
            .get(id)
            .map(|w| w.watch.clone())
            .filter(|w| any || w.principal == caller.principal())
            .ok_or_else(|| {
                EngineError::Denied(ErrorObject::new(
                    "watch.unknown",
                    format!("no watch '{id}' of yours is running"),
                ))
            })?;
        self.end_watch(id, &format!("cancelled by {}", caller.principal()));
        Ok(watch)
    }

    /// End every watch created with this credential, when it is revoked or replaced.
    pub fn end_watches_of_credential(&self, credential_id: &str) {
        let ids: Vec<String> = lock(&self.inner.watches.by_id)
            .values()
            .filter(|w| w.credential_id.as_deref() == Some(credential_id))
            .map(|w| w.watch.id.clone())
            .collect();
        for id in ids {
            self.end_watch(&id, "credential revoked");
        }
    }

    /// End every watch, at shutdown.
    pub(super) fn end_all_watches(&self) {
        let ids: Vec<String> = lock(&self.inner.watches.by_id).keys().cloned().collect();
        for id in ids {
            self.end_watch(&id, "gateway stopping");
        }
    }
}
