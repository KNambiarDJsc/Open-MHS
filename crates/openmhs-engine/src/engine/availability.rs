//! Device availability (ADR-0019).
//!
//! The engine keeps, per device, whether it answered the last time anything reached it:
//! a probe (`connect` until the first success, `health` after), or a read or command that
//! got through or found the device disconnected. A device that does not answer at
//! registration is registered `OFFLINE` instead of stopping the gateway.
//!
//! Commands to an `OFFLINE` device are probed once more and then refused before any
//! traffic (`device.unavailable`). Stops never are: status is bookkeeping, and bookkeeping
//! must not stand between an operator and a stop. Status never makes anything more
//! permissive: every other check still runs, and still fails closed on its own readings.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use openmhs_adapter::{AdapterError, AdapterErrorKind, HardwareAdapter, Health};
use openmhs_types::availability::LeaseHold;
use openmhs_types::{Availability, DeviceStatus, ErrorObject, Timestamp};
use tracing::{info, warn};

use super::{Device, Engine, lock};
use crate::error::EngineError;

/// How many availability probes run at once.
pub const MAX_CONCURRENT_PROBES: usize = 16;

/// The longest a periodic sweep waits between probes of a device that keeps not answering.
pub const MAX_PROBE_BACKOFF_MS: u64 = 300_000;

/// How long a periodic sweep leaves a device alone after its `failures`-th failed probe in a
/// row: the base interval doubled for each failure, capped at [`MAX_PROBE_BACKOFF_MS`], plus
/// up to a quarter more, derived from the device id so that devices that went down together
/// do not all come back to be probed on the same tick. No failures: the base interval.
pub fn probe_backoff_ms(base_ms: u64, failures: u32, device: &str) -> u64 {
    if failures == 0 {
        return base_ms;
    }
    let wait = base_ms
        .saturating_mul(1u64 << failures.min(6))
        .min(MAX_PROBE_BACKOFF_MS.max(base_ms));
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in device.bytes().chain(failures.to_le_bytes()) {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    wait + hash % (wait / 4 + 1)
}

/// What the engine last saw of a device.
#[derive(Debug, Clone)]
pub(super) struct Record {
    pub availability: Availability,
    pub reason: Option<String>,
    pub since: Timestamp,
    pub checked_at: Timestamp,
    /// Whether the adapter's `connect` has succeeded. Until it has, probes call `connect`.
    pub connected: bool,
    /// Probes and calls in a row that found the device not answering; zero while it is not
    /// `OFFLINE`. Periodic sweeps back off by it ([`probe_backoff_ms`]).
    pub failures: u32,
}

impl Record {
    pub fn online(at: Timestamp) -> Self {
        Self {
            availability: Availability::Online,
            reason: None,
            since: at,
            checked_at: at,
            connected: true,
            failures: 0,
        }
    }

    pub fn offline(at: Timestamp, reason: String) -> Self {
        Self {
            availability: Availability::Offline,
            reason: Some(reason),
            since: at,
            checked_at: at,
            connected: false,
            failures: 1,
        }
    }
}

/// Marks a device's probe as outstanding until the adapter call ends, however it ends.
struct InFlight(Arc<AtomicBool>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// How an adapter's answer to a probe maps onto availability.
fn classify(health: Health) -> (Availability, Option<String>) {
    match health {
        Health::Healthy => (Availability::Online, None),
        Health::Degraded(why) => (Availability::Degraded, Some(why)),
        Health::Disconnected(why) => (Availability::Offline, Some(why)),
    }
}

impl Engine {
    /// The status of one device, or `None` if it is not registered.
    pub fn device_status(&self, device: &str) -> Result<Option<DeviceStatus>, EngineError> {
        match self.device(device) {
            Some(d) => Ok(Some(self.status_of(&d)?)),
            None => Ok(None),
        }
    }

    /// The status of every registered device, sorted by id.
    pub fn device_statuses(&self) -> Result<Vec<DeviceStatus>, EngineError> {
        let mut devices = self.registered();
        devices.sort_by(|a, b| a.descriptor.device.id.cmp(&b.descriptor.device.id));
        devices.iter().map(|d| self.status_of(d)).collect()
    }

    fn status_of(&self, device: &Device) -> Result<DeviceStatus, EngineError> {
        let id = &device.descriptor.device.id;
        let record = lock(&device.availability).clone();
        let lease = self
            .inner
            .journal
            .active_leases(self.now())?
            .into_iter()
            .find(|l| l.claim.devices.contains(id))
            .map(|l| LeaseHold {
                id: l.id,
                holder: l.holder,
                expires_at: l.expires_at,
            });
        Ok(DeviceStatus {
            device: id.clone(),
            availability: record.availability,
            reason: record.reason,
            since: record.since,
            checked_at: record.checked_at,
            // Held by any state-changing command, from its live checks to verification.
            busy: device.lock.try_lock().is_err(),
            lease,
        })
    }

    /// Probe every device not checked in the last `min_age_ms`, and not busy: a running
    /// command is already talking to it, and a probe must not get in its way. Returns how
    /// many were probed.
    ///
    /// Probes run concurrently, at most [`MAX_CONCURRENT_PROBES`] at a time, so a sweep of
    /// many switched-off devices takes about `ceil(n / limit)` probe timeouts rather than
    /// `n`, and the number of tasks stays bounded however many devices there are.
    pub async fn refresh_availability(&self, min_age_ms: u64) -> Result<usize, EngineError> {
        self.refresh_inner(min_age_ms, false).await
    }

    /// The periodic sweep: as [`Engine::refresh_availability`] with `base_ms`, except that a
    /// device that keeps not answering is probed less and less often
    /// ([`probe_backoff_ms`]), so a large fleet with many switched-off devices does not
    /// spend every interval timing out on the same ones. A device that answers, or that a
    /// command reaches, is back on the base interval at once. Commands to an `OFFLINE`
    /// device still probe it on demand.
    pub async fn sweep_availability(&self, base_ms: u64) -> Result<usize, EngineError> {
        self.refresh_inner(base_ms, true).await
    }

    async fn refresh_inner(&self, min_age_ms: u64, backoff: bool) -> Result<usize, EngineError> {
        let now = self.now();
        let due: Vec<Arc<Device>> = self
            .registered()
            .into_iter()
            .filter(|d| {
                let wait = if backoff {
                    let failures = lock(&d.availability).failures;
                    probe_backoff_ms(min_age_ms, failures, &d.descriptor.device.id)
                } else {
                    min_age_ms
                };
                let wait = i64::try_from(wait).unwrap_or(i64::MAX);
                lock(&d.availability).checked_at.ms_until(now) >= wait
                    && d.lock.try_lock().is_ok()
                    && !d.probing.load(Ordering::SeqCst)
            })
            .collect();
        let probed = due.len();
        let limit = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_PROBES));
        let mut probes = tokio::task::JoinSet::new();
        for device in due {
            let (engine, limit) = (self.clone(), limit.clone());
            probes.spawn(async move {
                let _permit = limit.acquire_owned().await;
                engine.probe(&device).await
            });
        }
        let mut failed = None;
        while let Some(done) = probes.join_next().await {
            match done {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    failed.get_or_insert(e);
                }
                Err(join) => warn!(error = %join, "availability probe task failed"),
            }
        }
        match failed {
            Some(e) => Err(e),
            None => Ok(probed),
        }
    }

    /// `connect` for a device being registered, bounded by the probe timeout. The call is
    /// abandoned, not cancelled, if it outlives it, and `probing` stays set until it ends so
    /// that the device is not probed again meanwhile.
    pub(super) async fn connect_bounded(
        &self,
        adapter: Arc<dyn HardwareAdapter>,
        probing: Arc<AtomicBool>,
    ) -> Result<(), AdapterError> {
        probing.store(true, Ordering::SeqCst);
        let in_flight = InFlight(probing);
        let task = tokio::spawn(async move {
            let _in_flight = in_flight;
            adapter.connect().await
        });
        let limit = self.inner.options.probe_timeout_ms;
        match tokio::time::timeout(Duration::from_millis(limit), task).await {
            Ok(Ok(result)) => result,
            Ok(Err(join)) => Err(AdapterError::new(
                AdapterErrorKind::Internal,
                format!("connect failed: {join}"),
            )),
            Err(_) => Err(AdapterError::new(
                AdapterErrorKind::Timeout,
                format!("no answer to connect within {limit} ms"),
            )),
        }
    }

    /// Ask the adapter whether the device answers, bounded by the probe timeout, and record
    /// what it said. `connect` until it has succeeded once, then `health`; both are free of
    /// side effects for a conforming adapter (`L0.connect`, `L0.health`).
    ///
    /// At most one probe per device is outstanding. A call that outlives the timeout is
    /// abandoned, not cancelled (cancelling could cut an exchange in half), and until it
    /// returns the device is not probed again: its recorded availability stands.
    pub(super) async fn probe(&self, device: &Arc<Device>) -> Result<Availability, EngineError> {
        if device.probing.swap(true, Ordering::SeqCst) {
            return Ok(lock(&device.availability).availability);
        }
        let connected = lock(&device.availability).connected;
        let adapter: Arc<dyn HardwareAdapter> = device.adapter.clone();
        let in_flight = InFlight(device.probing.clone());
        let task = tokio::spawn(async move {
            let _in_flight = in_flight;
            if connected {
                (adapter.health().await, true)
            } else {
                match adapter.connect().await {
                    Ok(()) => (Health::Healthy, true),
                    Err(e) => (Health::Disconnected(e.message), false),
                }
            }
        });
        let limit = self.inner.options.probe_timeout_ms;
        let (health, connected) =
            match tokio::time::timeout(Duration::from_millis(limit), task).await {
                Ok(Ok(answer)) => answer,
                Ok(Err(join)) => (
                    Health::Disconnected(format!("health check failed: {join}")),
                    connected,
                ),
                Err(_) => (
                    Health::Disconnected(format!("no answer to a health check within {limit} ms")),
                    connected,
                ),
            };
        let (to, reason) = classify(health);
        lock(&device.availability).connected = connected;
        self.set_availability(device, to, reason)?;
        Ok(to)
    }

    /// Learn from a call that reached the adapter anyway: a reading or a report means the
    /// device answers; a disconnect means it does not. Other failures say nothing about
    /// reachability and change nothing. Never fails the call it observes.
    pub(super) fn observe(&self, device: &Device, result: Result<(), &AdapterError>) {
        let change = match result {
            Ok(()) => {
                let mut r = lock(&device.availability);
                r.connected = true;
                r.failures = 0;
                if r.availability == Availability::Offline {
                    Some((Availability::Online, None))
                } else {
                    r.checked_at = self.now();
                    None
                }
            }
            Err(e) if e.kind == AdapterErrorKind::Disconnected => {
                Some((Availability::Offline, Some(e.message.clone())))
            }
            Err(_) => None,
        };
        if let Some((to, reason)) = change
            && let Err(e) = self.set_availability(device, to, reason)
        {
            warn!(device = %device.descriptor.device.id, error = %e, "could not record availability");
        }
    }

    /// Record an availability. A change is audited (`device.availability`) and logged; the
    /// same availability again only refreshes the reason and the check time.
    fn set_availability(
        &self,
        device: &Device,
        to: Availability,
        reason: Option<String>,
    ) -> Result<(), EngineError> {
        let now = self.now();
        let from = {
            let mut r = lock(&device.availability);
            r.checked_at = now;
            r.failures = if to == Availability::Offline {
                r.failures.saturating_add(1)
            } else {
                0
            };
            let from = r.availability;
            r.reason = reason.clone();
            if from == to {
                return Ok(());
            }
            r.availability = to;
            r.since = now;
            from
        };
        let id = &device.descriptor.device.id;
        match to {
            Availability::Online => info!(device = %id, from = from.as_str(), "device online"),
            _ => {
                warn!(device = %id, from = from.as_str(), to = to.as_str(), reason = reason.as_deref().unwrap_or(""), "device availability changed")
            }
        }
        self.inner.journal.append_audit(
            now,
            "device.availability",
            id,
            &serde_json::json!({ "from": from, "to": to, "reason": reason }),
        )?;
        Ok(())
    }

    /// The refusal for a command to a device that does not answer. An `OFFLINE` device is
    /// probed once more first, so a device that has just come back is not refused on old
    /// news. The message is the gateway's own: the adapter's reason is device-written text
    /// and is shown, quoted, by status instead.
    pub(super) async fn unavailable_refusal(&self, device: &Arc<Device>) -> Option<ErrorObject> {
        if lock(&device.availability).availability != Availability::Offline {
            return None;
        }
        let id = &device.descriptor.device.id;
        match self.probe(device).await {
            Ok(Availability::Offline) => {
                let since = lock(&device.availability).since;
                Some(
                    ErrorObject::new(
                        "device.unavailable",
                        format!(
                            "'{id}' is not answering, so nothing was sent; it has been offline \
                             since {}. Try again once devices.status shows it ONLINE, or ask \
                             the operator",
                            since.to_rfc3339()
                        ),
                    )
                    .detail("since", since.to_rfc3339()),
                )
            }
            Ok(_) => None,
            // Fail closed: if the probe cannot even be recorded, nothing runs.
            Err(e) => Some(ErrorObject::new(
                "gateway.journal",
                format!("could not record the availability of '{id}': {e}"),
            )),
        }
    }
}
