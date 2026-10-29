//! The command engine.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;

use openmhs_adapter::{
    AdapterCommand, AdapterError, AdapterErrorKind, ExecuteReport, HardwareAdapter, Progress,
    ProgressEvent,
};
use openmhs_policy::{Caller, Expectation, Live, Mutation, Policy, Prepared, PrincipalKind};
use openmhs_types::descriptor::{Capability, Operation};
use openmhs_types::event::ObservationCause;
use openmhs_types::graph::{Graph, check_topology};
use openmhs_types::receipt::{
    AdapterResult, Decision, PolicyRecord, PreflightResult, RuleMatch, VerificationMode,
    VerificationRecord,
};
use openmhs_types::schema::{self, Schema};
use openmhs_types::{
    Clock, CommandRequest, CommandState, Descriptor, ErrorObject, Issue, Receipt, RequestOperation,
    StateValue, Timestamp, Topology, TopologyRelationship,
};
use tracing::{info, warn};

mod approvals;
mod availability;
mod events;
mod leases;
mod plans;
mod twin;

pub use approvals::{DEFAULT_APPROVAL_TTL_MS, MAX_APPROVAL_TTL_MS, fingerprint};
pub use availability::{MAX_CONCURRENT_PROBES, MAX_PROBE_BACKOFF_MS, probe_backoff_ms};
pub use events::{
    DEFAULT_EVENT_CAPACITY, DEFAULT_WATCH_TTL_MS, EventFilter, EventQuery, MAX_EVENTS_PER_POLL,
    MAX_POLL_WAIT_MS, MAX_WATCH_TTL_MS, MAX_WATCHES, MAX_WATCHES_PER_PRINCIPAL,
    MIN_WATCH_INTERVAL_MS,
};
pub use leases::LeaseRequest;

use crate::error::EngineError;
use crate::journal::{AuditVerification, Inserted, Journal, JournalError, sha256_hex};

use CommandState::*;

/// Tunables. Defaults are conservative.
#[derive(Debug, Clone)]
pub struct EngineOptions {
    /// Upper bound for a single adapter read.
    pub read_timeout_ms: u64,
    /// Interval between readback attempts during verification.
    pub verify_poll_ms: u64,
    /// Refuse a non-idempotent write or invoke while an earlier one to the same
    /// capability is `UNKNOWN_OUTCOME` (spec/lifecycle.md §3 rule 3). Only the conformance
    /// suite turns this off: it tests adapters against simulators, and sends again after
    /// the faults it injects on purpose.
    pub hold_unresolved: bool,
    /// Upper bound for one availability probe (`connect` or `health`, ADR-0019).
    pub probe_timeout_ms: u64,
    /// How much older than the moment of dispatch a reading may be stamped and still count
    /// as taken after it. Only for the gap between a device's clock and the gateway's; a
    /// reading older than this cannot show that a command took effect and is not used to
    /// verify or reconcile it (ADR-0020). Zero trusts nothing a device stamps earlier.
    pub readback_skew_ms: u64,
    /// Events kept in memory for `events.poll` (ADR-0023). Older ones are dropped, and a
    /// consumer that had not read them is told how many it missed.
    pub event_capacity: usize,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            read_timeout_ms: 1_000,
            verify_poll_ms: 50,
            hold_unresolved: true,
            probe_timeout_ms: 2_000,
            readback_skew_ms: 0,
            event_capacity: events::DEFAULT_EVENT_CAPACITY,
        }
    }
}

/// A hard limit on the journal and the reserve below it (ADR-0021).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageLimits {
    /// SQLite refuses any write past this size.
    pub max_bytes: u64,
    /// Within this much of `max_bytes`, new physical commands and audited reads are
    /// refused, so that stops, recovery and a person's resolutions still fit.
    pub reserve_bytes: u64,
}

/// What [`Engine::recover`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Commands moved to `UNKNOWN_OUTCOME` because they were in flight.
    pub unknown_outcome: Vec<String>,
    /// Commands denied or expired because they had not been dispatched.
    pub not_executed: Vec<String>,
    /// Commands whose verification was resumed.
    pub verification_resumed: Vec<String>,
    /// Commands left untouched because their device is not registered yet.
    pub skipped: Vec<String>,
    /// Plans that were moving and now wait for a decision.
    pub plans_awaiting_decision: Vec<String>,
}

struct Device {
    descriptor: Descriptor,
    adapter: Arc<dyn HardwareAdapter>,
    /// Serializes state-changing commands on this device, from live checks to verification.
    lock: Arc<tokio::sync::Mutex<()>>,
    /// Dispatched mutations per capability, for rate and slew limits.
    history: Mutex<HashMap<String, Vec<Mutation>>>,
    /// Whether it answers (ADR-0019).
    availability: Mutex<availability::Record>,
    /// A probe's adapter call is still running. A timed-out call is abandoned, not
    /// cancelled, so this keeps a hung device to one outstanding probe.
    probing: Arc<std::sync::atomic::AtomicBool>,
}

struct Inner {
    clock: Arc<dyn Clock>,
    journal: Journal,
    policy: RwLock<Policy>,
    devices: RwLock<HashMap<String, Arc<Device>>>,
    /// The operator's installation-specific relationships (spec/topology.md).
    topology: RwLock<Vec<TopologyRelationship>>,
    state: Mutex<HashMap<(String, String), StateValue>>,
    storage: Mutex<Option<StorageLimits>>,
    options: EngineOptions,
    plans: plans::PlanRuntime,
    /// The state fabric's event stream and watches (ADR-0023). In memory only.
    bus: events::Bus,
    watches: events::Watches,
}

/// Cheap to clone; all clones share one engine.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// How long to keep mutation history; longer than any sensible rate/slew window.
const HISTORY_RETENTION_MS: u64 = 3_600_000;

/// Outcome of validating a request up to (not including) dispatch.
enum Evaluation {
    Denied {
        errors: Vec<ErrorObject>,
        rules: Vec<RuleMatch>,
    },
    /// Allowed or held for approval. The device lock is held so the live checks stay
    /// valid until dispatch; a stop holds none (ADR-0018).
    Ready {
        device: Arc<Device>,
        cap: Box<Capability>,
        decision: Decision,
        rules: Vec<RuleMatch>,
        prepared: Prepared,
        guard: Option<tokio::sync::OwnedMutexGuard<()>>,
    },
}

enum ExecOutcome {
    Finished(Result<Result<ExecuteReport, AdapterError>, tokio::task::JoinError>),
    TimedOut,
}

impl Engine {
    pub fn open(path: &Path, clock: Arc<dyn Clock>, policy: Policy) -> Result<Self, EngineError> {
        Ok(Self::with_journal(
            Journal::open(path)?,
            clock,
            policy,
            EngineOptions::default(),
        ))
    }

    pub fn in_memory(clock: Arc<dyn Clock>, policy: Policy) -> Result<Self, EngineError> {
        Ok(Self::with_journal(
            Journal::open_in_memory()?,
            clock,
            policy,
            EngineOptions::default(),
        ))
    }

    pub fn with_journal(
        journal: Journal,
        clock: Arc<dyn Clock>,
        policy: Policy,
        options: EngineOptions,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                clock,
                journal,
                policy: RwLock::new(policy),
                devices: RwLock::new(HashMap::new()),
                topology: RwLock::new(Vec::new()),
                state: Mutex::new(HashMap::new()),
                storage: Mutex::new(None),
                bus: events::Bus::new(options.event_capacity),
                watches: events::Watches::default(),
                options,
                plans: plans::PlanRuntime::default(),
            }),
        }
    }

    fn now(&self) -> Timestamp {
        self.inner.clock.now()
    }

    /// Limit the journal's size and keep a reserve below the limit for critical writes
    /// (ADR-0021). The reserve must be smaller than the limit, and the journal must
    /// already fit in it.
    pub fn set_storage_limits(&self, limits: StorageLimits) -> Result<(), EngineError> {
        if limits.reserve_bytes >= limits.max_bytes {
            return Err(EngineError::InvalidRequest(vec![ErrorObject::new(
                "storage.limits",
                "the reserve must be smaller than the journal's size limit",
            )]));
        }
        let used = self.inner.journal.size_bytes()?;
        if used > limits.max_bytes {
            // SQLite would quietly keep the larger size as the limit.
            return Err(EngineError::InvalidRequest(vec![ErrorObject::new(
                "storage.limits",
                format!(
                    "the journal is already {used} bytes, above the limit of {} bytes",
                    limits.max_bytes
                ),
            )]));
        }
        self.inner.journal.set_max_bytes(limits.max_bytes)?;
        *lock(&self.inner.storage) = Some(limits);
        Ok(())
    }

    /// `Some` once the journal is within its reserve of its limit: the refusal for work
    /// that is not critical. If the size cannot be read, nothing non-critical runs.
    fn storage_refusal(&self) -> Option<ErrorObject> {
        let limits = (*lock(&self.inner.storage))?;
        match self.inner.journal.size_bytes() {
            Ok(used) if used.saturating_add(limits.reserve_bytes) < limits.max_bytes => None,
            Ok(used) => Some(
                ErrorObject::new(
                    "storage.exhausted",
                    format!(
                        "the journal is within its critical reserve of its size limit \
                         ({used} of {} bytes used), so nothing was sent; new commands and \
                         reads are refused until an operator frees space or raises the \
                         limit. Stops are still accepted",
                        limits.max_bytes
                    ),
                )
                .detail("used_bytes", used)
                .detail("limit_bytes", limits.max_bytes)
                .detail("reserve_bytes", limits.reserve_bytes),
            ),
            Err(e) => Some(ErrorObject::new(
                "gateway.journal",
                format!("could not read the journal's size: {e}"),
            )),
        }
    }

    pub fn journal(&self) -> &Journal {
        &self.inner.journal
    }

    pub fn policy(&self) -> Policy {
        self.inner
            .policy
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_policy(&self, policy: Policy) -> Result<(), EngineError> {
        let grants = policy.grants.len();
        *self
            .inner
            .policy
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = policy;
        self.inner.journal.append_audit(
            self.now(),
            "policy.updated",
            "policy",
            &serde_json::json!({ "grants": grants }),
        )?;
        Ok(())
    }

    fn device(&self, id: &str) -> Option<Arc<Device>> {
        self.inner
            .devices
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    fn registered(&self) -> Vec<Arc<Device>> {
        self.inner
            .devices
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    /// Registered device descriptors, sorted by id.
    pub fn devices(&self) -> Vec<Descriptor> {
        let devices = self
            .inner
            .devices
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut out: Vec<Descriptor> = devices.values().map(|d| d.descriptor.clone()).collect();
        out.sort_by(|a, b| a.device.id.cmp(&b.device.id));
        out
    }

    pub fn descriptor(&self, device: &str) -> Option<Descriptor> {
        self.device(device).map(|d| d.descriptor.clone())
    }

    /// The capability graph of what is registered now, with the operator's topology.
    pub fn graph(&self) -> Graph {
        let topology = self
            .inner
            .topology
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        Graph::build(&self.devices(), &topology)
    }

    /// Install the operator's topology. Refused, whole, if any relationship names a device
    /// that is not registered or breaks a relationship rule: register devices first.
    pub fn set_topology(&self, topology: Topology) -> Result<(), EngineError> {
        let issues = check_topology(&self.devices(), &topology.relationships);
        if !issues.is_empty() {
            return Err(EngineError::InvalidRequest(
                issues.into_iter().map(Into::into).collect(),
            ));
        }
        let count = topology.relationships.len();
        *self
            .inner
            .topology
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = topology.relationships;
        self.inner.journal.append_audit(
            self.now(),
            "topology.set",
            "topology",
            &serde_json::json!({ "relationships": count }),
        )?;
        Ok(())
    }

    /// Last reading the engine observed, without contacting the device.
    pub fn last_known(&self, device: &str, capability: &str) -> Option<StateValue> {
        lock(&self.inner.state)
            .get(&(device.to_owned(), capability.to_owned()))
            .cloned()
    }

    pub fn receipt(&self, command_id: &str) -> Result<Option<Receipt>, EngineError> {
        Ok(self.inner.journal.receipt(command_id)?)
    }

    /// Most recent commands first; see [`Journal::list_commands`].
    pub fn commands(
        &self,
        principal: Option<&str>,
        states: &[CommandState],
        limit: usize,
    ) -> Result<Vec<Receipt>, EngineError> {
        Ok(self.inner.journal.list_commands(principal, states, limit)?)
    }

    pub fn verify_audit(&self) -> Result<AuditVerification, EngineError> {
        Ok(self.inner.journal.verify_audit()?)
    }

    /// Register a device. Its descriptor comes from the adapter and must validate, and the
    /// device must answer `connect`. Registration grants no permissions
    /// (spec/authorization.md §1).
    pub async fn register(
        &self,
        adapter: Arc<dyn HardwareAdapter>,
    ) -> Result<Descriptor, EngineError> {
        self.register_inner(adapter, false).await
    }

    /// Register a device that may be switched off: as [`Engine::register`], but if
    /// `connect` fails the device is registered `OFFLINE` and comes online when it answers
    /// (ADR-0019). A descriptor that does not validate is still refused.
    pub async fn register_allow_offline(
        &self,
        adapter: Arc<dyn HardwareAdapter>,
    ) -> Result<Descriptor, EngineError> {
        self.register_inner(adapter, true).await
    }

    async fn register_inner(
        &self,
        adapter: Arc<dyn HardwareAdapter>,
        allow_offline: bool,
    ) -> Result<Descriptor, EngineError> {
        let json = adapter.describe().await.map_err(EngineError::Adapter)?;
        let descriptor =
            openmhs_types::validate_descriptor(&json).map_err(EngineError::InvalidDescriptor)?;
        if descriptor.adapter.id != adapter.adapter_id() {
            return Err(EngineError::InvalidDescriptor(vec![Issue::new(
                "registry.adapter_mismatch",
                "/adapter/id",
                format!(
                    "descriptor names adapter '{}' but it is served by '{}'",
                    descriptor.adapter.id,
                    adapter.adapter_id()
                ),
            )]));
        }
        let probing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let connected = if allow_offline {
            // A device that drops packets instead of refusing them must not hold the
            // start: bound the connect, and keep one outstanding until it ends.
            self.connect_bounded(adapter.clone(), probing.clone()).await
        } else {
            adapter.connect().await
        };
        let availability = match connected {
            Ok(()) => availability::Record::online(self.now()),
            Err(e) if allow_offline => {
                warn!(device = %descriptor.device.id, error = %e, "device does not answer; registered OFFLINE");
                availability::Record::offline(self.now(), e.message)
            }
            Err(e) => return Err(EngineError::Adapter(e)),
        };
        let id = descriptor.device.id.clone();
        {
            let mut devices = self
                .inner
                .devices
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if devices.contains_key(&id) {
                return Err(EngineError::DuplicateDevice(id));
            }
            devices.insert(
                id.clone(),
                Arc::new(Device {
                    descriptor: descriptor.clone(),
                    adapter,
                    lock: Arc::new(tokio::sync::Mutex::new(())),
                    history: Mutex::new(HashMap::new()),
                    availability: Mutex::new(availability.clone()),
                    probing,
                }),
            );
        }
        self.inner.journal.append_audit(
            self.now(),
            "device.registered",
            &id,
            &serde_json::json!({
                "adapter": descriptor.adapter,
                "descriptor_sha256": descriptor_sha256(&descriptor),
                "manufacturer": descriptor.device.manufacturer,
                "model": descriptor.device.model,
                "capabilities": descriptor.capabilities.len(),
                "availability": availability.availability,
                "reason": availability.reason,
            }),
        )?;
        info!(device = %id, "device registered");
        Ok(descriptor)
    }

    /// Ask every adapter to shut down. Best effort: failures are logged.
    pub async fn shutdown_adapters(&self) {
        self.end_all_watches();
        let devices: Vec<Arc<Device>> = self
            .inner
            .devices
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for d in devices {
            if let Err(e) = d.adapter.shutdown().await {
                warn!(device = %d.descriptor.device.id, error = %e, "adapter shutdown failed");
            }
        }
    }

    // ---------------------------------------------------------------- reads

    /// Authorized, audited read straight from the device. The caller's credential must
    /// scope the read and a grant must allow it.
    pub async fn read(
        &self,
        caller: &Caller,
        device_id: &str,
        capability: &str,
    ) -> Result<StateValue, EngineError> {
        let now = self.now();
        let principal = caller.principal();
        let audit_denied = |e: &ErrorObject| {
            self.inner.journal.append_audit(
                now,
                "read.denied",
                device_id,
                &serde_json::json!({
                    "principal": principal, "credential_id": caller.credential_id(), "capability": capability, "error": e,
                }),
            )
        };
        // Before any lookup, so a credential cannot probe which devices exist.
        if !caller.in_scope(device_id, capability, Operation::Read) {
            let e = scope_denied(caller, device_id, capability, Operation::Read);
            audit_denied(&e)?;
            return Err(EngineError::Denied(e));
        }
        let Some(device) = self.device(device_id) else {
            let e = ErrorObject::new(
                "command.device.unknown",
                format!("device '{device_id}' is not registered"),
            );
            audit_denied(&e)?;
            return Err(EngineError::Denied(e));
        };
        let Some(cap) = device.descriptor.capability(capability) else {
            let e = ErrorObject::new(
                "command.capability.unknown",
                format!("'{device_id}' has no capability '{capability}'"),
            );
            audit_denied(&e)?;
            return Err(EngineError::Denied(e));
        };
        let authz = self
            .policy()
            .authorize(principal, device_id, cap, Operation::Read);
        if authz.decision != Decision::Allow {
            let e = authz
                .error
                .unwrap_or_else(|| ErrorObject::new("policy.denied.no_grant", "read not allowed"));
            audit_denied(&e)?;
            return Err(EngineError::Denied(e));
        }
        if let Err(e) = openmhs_policy::check_read(cap) {
            audit_denied(&e)?;
            return Err(EngineError::Denied(e));
        }
        // An audited read writes a record; once only the reserve is left, it must not.
        if let Some(e) = self.storage_refusal() {
            return Err(EngineError::Denied(e));
        }
        let result = self
            .read_device(&device, capability, ObservationCause::Read, None)
            .await;
        self.inner.journal.append_audit(
            self.now(),
            "read",
            device_id,
            &match &result {
                Ok(v) => serde_json::json!({ "principal": principal, "credential_id": caller.credential_id(), "capability": capability, "state": v }),
                Err(e) => serde_json::json!({ "principal": principal, "credential_id": caller.credential_id(), "capability": capability, "error": e.kind.code(), "message": e.message }),
            },
        )?;
        result.map_err(EngineError::Adapter)
    }

    /// Read through the adapter with a timeout, isolating adapter panics, and cache the result.
    async fn read_device(
        &self,
        device: &Arc<Device>,
        capability: &str,
        cause: ObservationCause,
        command_id: Option<&str>,
    ) -> Result<StateValue, AdapterError> {
        let adapter = device.adapter.clone();
        let cap = capability.to_owned();
        let task = tokio::spawn(async move { adapter.read(&cap).await });
        let timeout = Duration::from_millis(self.inner.options.read_timeout_ms);
        let result = match tokio::time::timeout(timeout, task).await {
            Err(_) => Err(AdapterError::new(
                AdapterErrorKind::Timeout,
                "read timed out",
            )),
            Ok(Err(join)) => Err(AdapterError::new(
                AdapterErrorKind::Internal,
                format!("adapter task failed: {join}"),
            )),
            Ok(Ok(r)) => r,
        };
        if let Ok(v) = &result {
            lock(&self.inner.state).insert(
                (device.descriptor.device.id.clone(), capability.to_owned()),
                v.clone(),
            );
            self.publish_observation(
                &device.descriptor.device.id,
                capability,
                v,
                cause,
                command_id,
            );
        }
        self.observe(device, result.as_ref().map(|_| ()));
        result
    }

    // ---------------------------------------------------------------- commands

    /// Submit a write or invoke. Runs the command to a terminal state, `UNKNOWN_OUTCOME`,
    /// or `AWAITING_APPROVAL`, and returns its receipt.
    pub async fn submit(
        &self,
        caller: &Caller,
        request: &serde_json::Value,
    ) -> Result<Receipt, EngineError> {
        self.submit_inner(caller, request, None).await
    }

    /// Submit a command that needs approval together with an approval that was requested
    /// and approved ahead of it ([`Engine::request_approval`]). The approval is spent only
    /// if it was given for exactly this command, by this requester, and has not expired;
    /// otherwise the command is denied and the approval is left as it was.
    pub async fn submit_approved(
        &self,
        caller: &Caller,
        request: &serde_json::Value,
        approval_id: &str,
    ) -> Result<Receipt, EngineError> {
        self.submit_inner(caller, request, Some(approval_id)).await
    }

    async fn submit_inner(
        &self,
        caller: &Caller,
        request: &serde_json::Value,
        approval_id: Option<&str>,
    ) -> Result<Receipt, EngineError> {
        let now = self.now();
        let principal = caller.principal();
        let issues = schema::validate(Schema::Command, request);
        if !issues.is_empty() {
            let errors: Vec<ErrorObject> = issues.into_iter().map(Into::into).collect();
            self.inner.journal.append_audit(
                now,
                "request.rejected",
                principal,
                &serde_json::json!({ "errors": errors, "credential_id": caller.credential_id() }),
            )?;
            return Err(EngineError::InvalidRequest(errors));
        }
        let req: CommandRequest = serde_json::from_value(request.clone()).map_err(|e| {
            EngineError::InvalidRequest(vec![ErrorObject::new("command.schema", e.to_string())])
        })?;
        if req.operation == RequestOperation::Read {
            return Err(EngineError::NotACommand);
        }

        let request_hash =
            sha256_hex(&serde_json::to_string(&req).map_err(crate::journal::JournalError::from)?);
        let receipt = Receipt::created(
            uuid::Uuid::now_v7().to_string(),
            principal.to_owned(),
            req,
            now,
        );
        let receipt =
            match self
                .inner
                .journal
                .insert_command(&receipt, &request_hash, &origin(caller))?
            {
                Inserted::New(r) => r,
                Inserted::Existing {
                    receipt,
                    request_hash: existing,
                } => {
                    return if existing == request_hash {
                        Ok(receipt)
                    } else {
                        Err(EngineError::IdempotencyConflict(receipt.command_id))
                    };
                }
            };
        self.run(caller, receipt, approval_id).await
    }

    fn transition(
        &self,
        id: &str,
        to: CommandState,
        detail: Option<String>,
        update: impl FnOnce(&mut Receipt),
    ) -> Result<Receipt, EngineError> {
        let r = self
            .inner
            .journal
            .transition(id, to, self.now(), detail, update)?;
        info!(command = %id, state = ?to, "transition");
        self.publish_command(&r);
        Ok(r)
    }

    /// When the command was handed to the adapter (its `DISPATCHED` transition).
    fn dispatched_at(&self, id: &str) -> Result<Option<Timestamp>, EngineError> {
        Ok(self.receipt(id)?.and_then(|r| {
            r.transitions
                .iter()
                .find(|t| t.state == Dispatched)
                .map(|t| t.at)
        }))
    }

    /// Whether a reading was measured at or after `since`, allowing the configured clock
    /// gap. With no `since` (a command that was never dispatched) there is nothing to be
    /// older than.
    fn measured_since(&self, reading: &StateValue, since: Option<Timestamp>) -> bool {
        match since {
            Some(at) => {
                reading
                    .timestamp
                    .plus_ms(self.inner.options.readback_skew_ms)
                    >= at
            }
            None => true,
        }
    }

    fn deny(
        &self,
        id: &str,
        errors: Vec<ErrorObject>,
        rules: Vec<RuleMatch>,
    ) -> Result<Receipt, EngineError> {
        let detail = errors.first().map(|e| e.message.clone());
        self.transition(id, Denied, detail, |r| {
            r.policy = PolicyRecord {
                decision: Decision::Deny,
                rules,
                errors,
                approved_by: None,
            };
        })
    }

    /// Validation order of spec/lifecycle.md §3a, shared by submission and preflight so
    /// the two can never disagree.
    async fn evaluate(&self, caller: &Caller, req: &CommandRequest) -> Evaluation {
        let principal = caller.principal();
        let denied = |e: ErrorObject| Evaluation::Denied {
            errors: vec![e],
            rules: vec![],
        };
        // The credential's ceiling comes first and before any lookup, so it can neither
        // be widened by a grant nor used to probe for devices it cannot reach.
        let operation: Operation = req.operation.into();
        if !caller.in_scope(&req.target.device, &req.target.capability, operation) {
            return denied(scope_denied(
                caller,
                &req.target.device,
                &req.target.capability,
                operation,
            ));
        }
        let Some(device) = self.device(&req.target.device) else {
            return denied(ErrorObject::new(
                "command.device.unknown",
                format!("device '{}' is not registered", req.target.device),
            ));
        };
        let Some(cap) = device
            .descriptor
            .capability(&req.target.capability)
            .cloned()
        else {
            return denied(ErrorObject::new(
                "command.capability.unknown",
                format!(
                    "'{}' has no capability '{}'",
                    req.target.device, req.target.capability
                ),
            ));
        };

        // Someone else's lease keeps everyone but its holder off the device (ADR-0018).
        // If the leases cannot be read, nothing runs. Stops are never leased away.
        if !cap.stop
            && let Some(e) = self.lease_refusal(principal, &req.target.device)
        {
            return denied(e);
        }

        let authz =
            self.policy()
                .authorize(principal, &req.target.device, &cap, req.operation.into());
        if authz.decision == Decision::Deny {
            let e = authz
                .error
                .unwrap_or_else(|| ErrorObject::new("policy.denied.no_grant", "not allowed"));
            return Evaluation::Denied {
                errors: vec![e],
                rules: authz.rules,
            };
        }

        // Stateless checks first: an invalid request never causes device traffic.
        if let Err(errors) = openmhs_policy::check_command(&device.descriptor, &cap, req, None) {
            return Evaluation::Denied {
                errors,
                rules: authz.rules,
            };
        }

        // Within the critical reserve of the journal's limit, only critical work runs
        // (ADR-0021). Never a stop.
        if !cap.stop
            && let Some(e) = self.storage_refusal()
        {
            return Evaluation::Denied {
                errors: vec![e],
                rules: authz.rules,
            };
        }

        // A device that does not answer is refused before anything is sent (ADR-0019).
        // Never a stop: bookkeeping does not stand between an operator and a stop.
        if !cap.stop
            && let Some(e) = self.unavailable_refusal(&device).await
        {
            return Evaluation::Denied {
                errors: vec![e],
                rules: authz.rules,
            };
        }

        // The stop lane: a stop does not wait for the command it may be meant to stop.
        // It has no preconditions or rate limit to protect (descriptor rule 14).
        let guard = if cap.stop {
            None
        } else {
            Some(device.lock.clone().lock_owned().await)
        };
        // No blind retries (spec/lifecycle.md §3 rule 3). Checked under the lock, so a
        // command that has just ended uncertain is seen.
        if self.inner.options.hold_unresolved
            && !cap.stop
            && cap.idempotent != Some(true)
            && let Some(e) = self.unresolved_refusal(&req.target.device, &req.target.capability)
        {
            return Evaluation::Denied {
                errors: vec![e],
                rules: authz.rules,
            };
        }
        match self.check_live(&device, &cap, req).await {
            Ok(prepared) => Evaluation::Ready {
                device,
                cap: Box::new(cap),
                decision: authz.decision,
                rules: authz.rules,
                prepared,
                guard,
            },
            Err(errors) => Evaluation::Denied {
                errors,
                rules: authz.rules,
            },
        }
    }

    async fn run(
        &self,
        caller: &Caller,
        receipt: Receipt,
        approval_id: Option<&str>,
    ) -> Result<Receipt, EngineError> {
        let id = receipt.command_id.clone();
        self.transition(&id, Validating, None, |_| {})?;
        // `_guard` keeps the device locked from the live checks through verification.
        let (device, cap, decision, rules, prepared, _guard) =
            match self.evaluate(caller, &receipt.request).await {
                Evaluation::Denied { errors, rules } => return self.deny(&id, errors, rules),
                Evaluation::Ready {
                    device,
                    cap,
                    decision,
                    rules,
                    prepared,
                    guard,
                } => (device, *cap, decision, rules, prepared, guard),
            };

        if decision == Decision::RequireApproval {
            if let Some(approval_id) = approval_id {
                // Spend the approval requested ahead of time, and move on, in one step.
                let held = rules.clone();
                let spent = self.inner.journal.consume_approval(
                    approval_id,
                    &id,
                    self.now(),
                    |r| fingerprint(&r.principal, &r.request),
                    Approved,
                    Some(format!("approval {approval_id} spent")),
                    |r, a| {
                        r.policy = PolicyRecord {
                            decision: Decision::RequireApproval,
                            rules: held,
                            errors: vec![],
                            approved_by: a.decided_by.clone(),
                        };
                    },
                );
                return match spent {
                    Ok(_) => self.dispatch(&device, &cap, &id, prepared).await,
                    Err(JournalError::Refused(e)) => self.deny(&id, vec![e], rules),
                    Err(e) => Err(e.into()),
                };
            }
            self.open_approval_for(caller.credential_id(), &receipt)?;
            return self.transition(
                &id,
                AwaitingApproval,
                Some("waiting for an approver".into()),
                |r| {
                    r.policy = PolicyRecord {
                        decision: Decision::RequireApproval,
                        rules,
                        errors: vec![],
                        approved_by: None,
                    };
                },
            );
        }
        self.transition(&id, Approved, None, |r| {
            r.policy = PolicyRecord {
                decision: Decision::Allow,
                rules,
                errors: vec![],
                approved_by: None,
            };
        })?;
        self.dispatch(&device, &cap, &id, prepared).await
    }

    /// Run every check a submission would run, against fresh readings, without creating
    /// a command or contacting the device beyond those reads. Audited.
    pub async fn preflight(
        &self,
        caller: &Caller,
        request: &serde_json::Value,
    ) -> Result<PreflightResult, EngineError> {
        let principal = caller.principal();
        let issues = schema::validate(Schema::Command, request);
        if !issues.is_empty() {
            return Err(EngineError::InvalidRequest(
                issues.into_iter().map(Into::into).collect(),
            ));
        }
        let req: CommandRequest = serde_json::from_value(request.clone()).map_err(|e| {
            EngineError::InvalidRequest(vec![ErrorObject::new("command.schema", e.to_string())])
        })?;
        if req.operation == RequestOperation::Read {
            return Err(EngineError::NotACommand);
        }
        let (decision, rules, errors) = match self.evaluate(caller, &req).await {
            Evaluation::Denied { errors, rules } => (Decision::Deny, rules, errors),
            Evaluation::Ready {
                decision, rules, ..
            } => (decision, rules, vec![]),
        };
        let result = PreflightResult {
            decision,
            rules,
            errors,
            checked_at: self.now(),
        };
        // A check refused for want of space does not write its own audit record: that
        // would spend the reserve it is refused to protect.
        if result.errors.iter().any(|e| e.code == "storage.exhausted") {
            return Ok(result);
        }
        self.inner.journal.append_audit(
            result.checked_at,
            "command.preflight",
            &req.target.device,
            &serde_json::json!({
                "principal": principal,
                "credential_id": caller.credential_id(),
                "request": req,
                "decision": result.decision,
                "errors": result.errors,
            }),
        )?;
        Ok(result)
    }

    /// The refusal for a non-idempotent write while an earlier one to the same capability
    /// may or may not have taken effect: sending it again could do it twice. If the
    /// journal cannot be read, nothing runs.
    fn unresolved_refusal(&self, device: &str, capability: &str) -> Option<ErrorObject> {
        match self.inner.journal.unresolved_commands(device, capability) {
            Ok(ids) => ids.first().map(|first| {
                ErrorObject::new(
                    "command.outcome_unresolved",
                    format!(
                        "command {first} on '{device}' '{capability}' may or may not have taken \
                         effect; reconcile it, or have an operator resolve it, before sending \
                         this again"
                    ),
                )
                .detail("unresolved", ids.clone())
            }),
            Err(e) => Some(ErrorObject::new(
                "gateway.journal",
                format!("could not check for unresolved commands: {e}"),
            )),
        }
    }

    /// Stateful checks against fresh readings taken now. Only readings taken during this
    /// check are used, so a disconnected device fails closed.
    async fn check_live(
        &self,
        device: &Arc<Device>,
        cap: &Capability,
        req: &CommandRequest,
    ) -> Result<Prepared, Vec<ErrorObject>> {
        let mut fresh: HashMap<String, StateValue> = HashMap::new();
        for name in openmhs_policy::state_needed(cap) {
            match self
                .read_device(device, &name, ObservationCause::Check, None)
                .await
            {
                Ok(v) => {
                    fresh.insert(name, v);
                }
                Err(e) => {
                    warn!(capability = %name, error = %e, "state read failed; treating as missing")
                }
            }
        }
        let history = lock(&device.history)
            .get(&cap.name)
            .cloned()
            .unwrap_or_default();
        let state = |name: &str| fresh.get(name).cloned();
        let live = Live {
            now: self.now(),
            state: &state,
            history: &history,
        };
        openmhs_policy::check_command(&device.descriptor, cap, req, Some(&live))
    }

    async fn dispatch(
        &self,
        device: &Arc<Device>,
        cap: &Capability,
        id: &str,
        prepared: Prepared,
    ) -> Result<Receipt, EngineError> {
        let receipt = self
            .inner
            .journal
            .receipt(id)?
            .ok_or_else(|| EngineError::UnknownCommand(id.into()))?;
        let now = self.now();
        let deadline = receipt.deadline_at.unwrap_or(now);
        if now >= deadline {
            return self.transition(
                id,
                Expired,
                Some("deadline passed before dispatch".into()),
                |_| {},
            );
        }

        self.inner
            .journal
            .set_expectation(id, &prepared.expectation)?;
        {
            let mut history = lock(&device.history);
            let entries = history.entry(cap.name.clone()).or_default();
            entries.retain(|m| m.at > now.minus_ms(HISTORY_RETENTION_MS));
            entries.push(Mutation {
                at: now,
                delta: prepared.delta,
            });
        }
        // The intent is durable before the adapter is called (spec/lifecycle.md §4).
        self.transition(id, Dispatched, Some("intent journaled".into()), |_| {})?;

        let command = AdapterCommand {
            command_id: id.to_owned(),
            capability: cap.name.clone(),
            operation: prepared.operation,
            value: prepared.value.clone(),
            params: prepared.params.clone(),
            idempotent: cap.idempotent.unwrap_or(false),
            deadline,
        };
        let remaining = u64::try_from(now.ms_until(deadline)).unwrap_or(0);
        let timeout_ms = cap
            .timing
            .map_or(remaining, |t| t.timeout_ms.min(remaining));

        let (progress, mut rx) = Progress::channel();
        let adapter = device.adapter.clone();
        let mut handle = tokio::spawn(async move { adapter.execute(command, progress).await });
        let sleep = tokio::time::sleep(Duration::from_millis(timeout_ms));
        tokio::pin!(sleep);
        let mut accepted = false;

        let outcome = loop {
            tokio::select! {
                biased;
                res = &mut handle => break ExecOutcome::Finished(res),
                Some(ProgressEvent::Accepted { external_ref }) = rx.recv(), if !accepted => {
                    accepted = true;
                    self.transition(id, Accepted, accepted_detail(external_ref.as_deref()), |r| {
                        let ar = r.adapter_result.get_or_insert_with(AdapterResult::default);
                        ar.acknowledged = Some(true);
                        ar.external_ref = external_ref;
                    })?;
                }
                () = &mut sleep => {
                    handle.abort();
                    break ExecOutcome::TimedOut;
                }
            }
        };
        if !accepted && let Ok(ProgressEvent::Accepted { external_ref }) = rx.try_recv() {
            accepted = true;
            self.transition(id, Accepted, accepted_detail(external_ref.as_deref()), |r| {
                let ar = r.adapter_result.get_or_insert_with(AdapterResult::default);
                ar.acknowledged = Some(true);
                ar.external_ref = external_ref;
            })?;
        }

        let with_error = |e: ErrorObject| {
            move |r: &mut Receipt| {
                r.adapter_result
                    .get_or_insert_with(AdapterResult::default)
                    .error = Some(e);
            }
        };
        match &outcome {
            ExecOutcome::Finished(Ok(Ok(_))) => self.observe(device, Ok(())),
            ExecOutcome::Finished(Ok(Err(err))) => self.observe(device, Err(err)),
            _ => {}
        }

        match outcome {
            ExecOutcome::TimedOut => {
                let e = ErrorObject::new(
                    "adapter.timeout",
                    format!("no result within {timeout_ms} ms"),
                );
                self.transition(
                    id,
                    TimedOut,
                    Some(format!("no result within {timeout_ms} ms")),
                    with_error(e),
                )?;
                self.transition(
                    id,
                    UnknownOutcome,
                    Some(
                        "may or may not have taken effect; not retried; reconcile by readback"
                            .into(),
                    ),
                    |_| {},
                )
            }
            ExecOutcome::Finished(Err(join)) => {
                let e =
                    ErrorObject::new("adapter.internal", format!("adapter task failed: {join}"));
                self.transition(
                    id,
                    UnknownOutcome,
                    Some("adapter crashed during execution; not retried".into()),
                    with_error(e),
                )
            }
            ExecOutcome::Finished(Ok(Err(err))) => {
                let e = ErrorObject::new(err.kind.code(), err.message.clone());
                if err.kind == AdapterErrorKind::Timeout {
                    self.transition(id, TimedOut, Some(err.message.clone()), with_error(e))?;
                    self.transition(
                        id,
                        UnknownOutcome,
                        Some("may or may not have taken effect; not retried".into()),
                        |_| {},
                    )
                } else if err.kind.outcome_uncertain() {
                    self.transition(
                        id,
                        UnknownOutcome,
                        Some(format!("{err}; not retried")),
                        with_error(e),
                    )
                } else {
                    self.transition(id, Failed, Some(err.to_string()), with_error(e))
                }
            }
            ExecOutcome::Finished(Ok(Ok(ExecuteReport::Rejected { reason }))) => {
                let e = ErrorObject::new("adapter.rejected", reason.clone());
                let to = if accepted { Failed } else { Rejected };
                self.transition(
                    id,
                    to,
                    Some(format!("device rejected the command: {reason}")),
                    with_error(e),
                )
            }
            ExecOutcome::Finished(Ok(Ok(ExecuteReport::Applied { result }))) => {
                if !accepted {
                    self.transition(
                        id,
                        Accepted,
                        Some("implied by the applied report".into()),
                        |r| {
                            r.adapter_result
                                .get_or_insert_with(AdapterResult::default)
                                .acknowledged = Some(true);
                        },
                    )?;
                }
                self.transition(id, Applied, Some("device reports applied".into()), |r| {
                    let ar = r.adapter_result.get_or_insert_with(AdapterResult::default);
                    ar.reported_applied = Some(true);
                    if !result.is_empty() {
                        ar.result = Some(result);
                    }
                })?;
                self.verify(device, id, &prepared.expectation).await
            }
        }
    }

    /// Readback verification (spec/descriptor.md §5).
    async fn verify(
        &self,
        device: &Arc<Device>,
        id: &str,
        expectation: &Expectation,
    ) -> Result<Receipt, EngineError> {
        let (capability, expected, tolerance, settle_ms, timeout_ms) = match expectation {
            Expectation::None { reason } => {
                let reason = reason.clone();
                return self.transition(
                    id,
                    Unverifiable,
                    Some(format!("no readback declared: {reason}")),
                    |r| {
                        r.verification = Some(VerificationRecord {
                            mode: VerificationMode::None,
                            capability: None,
                            expected: None,
                            observed: None,
                            matched: None,
                            reason: Some(reason),
                        });
                    },
                );
            }
            Expectation::Readback {
                capability,
                expected,
                tolerance,
                settle_ms,
                timeout_ms,
            } => (
                capability.clone(),
                expected.clone(),
                *tolerance,
                *settle_ms,
                *timeout_ms,
            ),
        };

        tokio::time::sleep(Duration::from_millis(settle_ms)).await;
        let started = self.now();
        let since = self.dispatched_at(id)?;
        let mut last: Option<StateValue> = None;
        let mut too_old = false;
        loop {
            if let Ok(v) = self
                .read_device(device, &capability, ObservationCause::Verify, Some(id))
                .await
                && v.is_usable(self.now(), None)
            {
                if !self.measured_since(&v, since) {
                    // Measured before the command was sent: it cannot show its effect,
                    // whether it matches or not.
                    too_old = true;
                } else if openmhs_policy::readback_matches(&expected, &v.value, tolerance) {
                    let detail = format!("readback {} matches", describe_value(&v.value));
                    return self.transition(id, Verified, Some(detail), |r| {
                        r.verification = Some(VerificationRecord {
                            mode: VerificationMode::Readback,
                            capability: Some(capability),
                            expected: Some(expected),
                            observed: Some(v),
                            matched: Some(true),
                            reason: None,
                        });
                    });
                } else {
                    last = Some(v);
                }
            }
            let elapsed = u64::try_from(started.ms_until(self.now())).unwrap_or(0);
            if elapsed >= timeout_ms {
                break;
            }
            let wait = self
                .inner
                .options
                .verify_poll_ms
                .min(timeout_ms - elapsed)
                .max(1);
            tokio::time::sleep(Duration::from_millis(wait)).await;
        }

        match last {
            Some(observed) => {
                let detail = format!(
                    "readback {} does not match expected {}",
                    describe_value(&observed.value),
                    describe_value(&expected)
                );
                self.transition(id, VerificationFailed, Some(detail), |r| {
                    r.verification = Some(VerificationRecord {
                        mode: VerificationMode::Readback,
                        capability: Some(capability),
                        expected: Some(expected),
                        observed: Some(observed),
                        matched: Some(false),
                        reason: None,
                    });
                })
            }
            None => self.transition(
                id,
                Unverifiable,
                Some(if too_old {
                    "readings were older than the command; none can show its effect".into()
                } else {
                    "no usable readback within the verification timeout".into()
                }),
                |r| {
                    r.verification = Some(VerificationRecord {
                        mode: VerificationMode::Readback,
                        capability: Some(capability),
                        expected: Some(expected),
                        observed: None,
                        matched: None,
                        reason: Some(if too_old {
                            "every reading was older than the command, so none can show its effect"
                                .into()
                        } else {
                            "readback source unavailable or not GOOD quality".into()
                        }),
                    });
                },
            ),
        }
    }

    // ---------------------------------------------------------------- approvals

    /// Approve a command waiting in `AWAITING_APPROVAL`. Authorization and live checks
    /// are re-run, because policy and device state may have changed while it waited.
    pub async fn approve(
        &self,
        approver_caller: &Caller,
        command_id: &str,
    ) -> Result<Receipt, EngineError> {
        let receipt = self.approve_command(approver_caller, command_id).await?;
        // A plan held on this command carries on before the call returns (ADR-0018).
        if receipt.state != AwaitingApproval {
            self.plan_command_settled(command_id).await?;
        }
        Ok(receipt)
    }

    async fn approve_command(
        &self,
        approver_caller: &Caller,
        command_id: &str,
    ) -> Result<Receipt, EngineError> {
        let approver = approver_caller.principal();
        if approver_caller.kind() == PrincipalKind::Agent {
            return Err(EngineError::Denied(ErrorObject::new(
                "policy.approval.not_human",
                "an agent credential cannot approve commands",
            )));
        }
        if !self.policy().is_approver(approver) {
            return Err(EngineError::Denied(ErrorObject::new(
                "policy.approval.not_approver",
                format!("'{approver}' is not an approver"),
            )));
        }
        let receipt = self
            .receipt(command_id)?
            .ok_or_else(|| EngineError::UnknownCommand(command_id.into()))?;
        // An approver's credential must reach what it approves.
        let target = &receipt.request.target;
        let operation: Operation = receipt.request.operation.into();
        if !approver_caller.in_scope(&target.device, &target.capability, operation) {
            return Err(EngineError::Denied(scope_denied(
                approver_caller,
                &target.device,
                &target.capability,
                operation,
            )));
        }
        if receipt.state != AwaitingApproval {
            return Err(EngineError::InvalidState {
                id: command_id.into(),
                state: receipt.state,
                action: "approve",
            });
        }
        if receipt.principal == approver {
            return Err(EngineError::Denied(ErrorObject::new(
                "policy.approval.self",
                "a principal cannot approve its own command",
            )));
        }
        if receipt.deadline_at.is_some_and(|d| self.now() >= d) {
            let expired = self.transition(
                command_id,
                Expired,
                Some("deadline passed while awaiting approval".into()),
                |_| {},
            );
            self.sweep_approvals()?;
            return expired;
        }

        let req = receipt.request.clone();
        let device = self.device(&req.target.device);
        let cap = device
            .as_ref()
            .and_then(|d| d.descriptor.capability(&req.target.capability).cloned());
        let (Some(device), Some(cap)) = (device, cap) else {
            let e = ErrorObject::new(
                "command.device.unknown",
                "device or capability no longer registered",
            );
            return self.transition(command_id, Denied, Some(e.message.clone()), |r| {
                r.policy.errors = vec![e]
            });
        };
        let authz = self.policy().authorize(
            &receipt.principal,
            &req.target.device,
            &cap,
            req.operation.into(),
        );
        if authz.decision == Decision::Deny {
            let e = authz
                .error
                .unwrap_or_else(|| ErrorObject::new("policy.denied.no_grant", "no longer allowed"));
            return self.transition(command_id, Denied, Some(e.message.clone()), |r| {
                r.policy = PolicyRecord {
                    decision: Decision::Deny,
                    rules: authz.rules,
                    errors: vec![e],
                    approved_by: None,
                };
            });
        }
        // A lease someone else took while the command waited stops it too (not a stop).
        if !cap.stop
            && let Some(e) = self.lease_refusal(&receipt.principal, &req.target.device)
        {
            return self.transition(command_id, Denied, Some(e.message.clone()), |r| {
                r.policy.decision = Decision::Deny;
                r.policy.errors = vec![e];
            });
        }

        if !cap.stop
            && let Some(e) = self.storage_refusal()
        {
            return self.transition(command_id, Denied, Some(e.message.clone()), |r| {
                r.policy.decision = Decision::Deny;
                r.policy.errors = vec![e];
            });
        }
        if !cap.stop
            && let Some(e) = self.unavailable_refusal(&device).await
        {
            return self.transition(command_id, Denied, Some(e.message.clone()), |r| {
                r.policy.decision = Decision::Deny;
                r.policy.errors = vec![e];
            });
        }

        let _guard = if cap.stop {
            None
        } else {
            Some(device.lock.lock().await)
        };
        let prepared = match self.check_live(&device, &cap, &req).await {
            Ok(p) => p,
            Err(errors) => {
                let detail = errors.first().map(|e| e.message.clone());
                return self.transition(command_id, Denied, detail, |r| {
                    r.policy.decision = Decision::Deny;
                    r.policy.errors = errors;
                });
            }
        };
        // The approval record for this command (commands held before M8 get theirs now).
        let approval = match self.inner.journal.approval_for_command(command_id)? {
            Some(a) => a,
            None => self.open_approval_for(None, &receipt)?,
        };
        if approval.status == openmhs_types::ApprovalStatus::Pending {
            self.inner.journal.decide_approval(
                &approval.id,
                openmhs_types::ApprovalStatus::Approved,
                approver,
                self.now(),
                None,
            )?;
        }
        // Spend it and move the command on in one transaction. The fingerprint is taken
        // from the command as stored at this moment, so a command changed after it was
        // held no longer matches what was approved.
        let spent = self.inner.journal.consume_approval(
            &approval.id,
            command_id,
            self.now(),
            |r| fingerprint(&r.principal, &r.request),
            Approved,
            Some(format!("approved by {approver}")),
            |r, a| r.policy.approved_by = a.decided_by.clone(),
        );
        match spent {
            Ok(_) => self.dispatch(&device, &cap, command_id, prepared).await,
            Err(JournalError::Refused(e)) => {
                let detail = Some(e.message.clone());
                if e.code == "approval.mismatch" {
                    let _ = self.inner.journal.decide_approval(
                        &approval.id,
                        openmhs_types::ApprovalStatus::Revoked,
                        approver,
                        self.now(),
                        Some("the command no longer matches what was approved".into()),
                    );
                }
                self.transition(command_id, Denied, detail, |r| {
                    r.policy.decision = Decision::Deny;
                    r.policy.errors = vec![e];
                })
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Cancel a command that has not been dispatched. Allowed for the requester and approvers.
    pub fn cancel(&self, caller: &Caller, command_id: &str) -> Result<Receipt, EngineError> {
        let principal = caller.principal();
        let receipt = self
            .receipt(command_id)?
            .ok_or_else(|| EngineError::UnknownCommand(command_id.into()))?;
        if receipt.principal != principal && !self.policy().is_approver(principal) {
            return Err(EngineError::Denied(ErrorObject::new(
                "policy.cancel.not_allowed",
                "only the requester or an approver can cancel",
            )));
        }
        if !receipt.state.can_transition_to(Cancelled) {
            return Err(EngineError::InvalidState {
                id: command_id.into(),
                state: receipt.state,
                action: "cancel",
            });
        }
        let cancelled = self.transition(
            command_id,
            Cancelled,
            Some(format!("cancelled by {principal}")),
            |_| {},
        )?;
        self.plan_command_settled_later(command_id);
        if let Some(a) = self.inner.journal.approval_for_command(command_id)?
            && !a.status.is_final()
        {
            self.inner.journal.decide_approval(
                &a.id,
                openmhs_types::ApprovalStatus::Revoked,
                principal,
                self.now(),
                Some("the command was cancelled".into()),
            )?;
        }
        Ok(cancelled)
    }

    // ---------------------------------------------------------------- reconciliation and recovery

    /// Resolve an `UNKNOWN_OUTCOME` command by reading the device (spec/lifecycle.md §5).
    pub async fn reconcile(&self, command_id: &str) -> Result<Receipt, EngineError> {
        let receipt = self
            .receipt(command_id)?
            .ok_or_else(|| EngineError::UnknownCommand(command_id.into()))?;
        if receipt.state != UnknownOutcome {
            return Err(EngineError::InvalidState {
                id: command_id.into(),
                state: receipt.state,
                action: "reconcile",
            });
        }
        let Some(Expectation::Readback {
            capability,
            expected,
            tolerance,
            ..
        }) = self.inner.journal.expectation(command_id)?
        else {
            return Err(EngineError::NotReconcilable(
                "no readback is declared for this capability; an operator can check the device and resolve it (commands.resolve)".into(),
            ));
        };
        let device = self
            .device(&receipt.request.target.device)
            .ok_or_else(|| EngineError::NotReconcilable("device is not registered".into()))?;
        let _guard = device.lock.lock().await;
        let observed = self
            .read_device(
                &device,
                &capability,
                ObservationCause::Reconcile,
                Some(command_id),
            )
            .await
            .ok()
            .filter(|v| v.is_usable(self.now(), None))
            .ok_or_else(|| {
                EngineError::NotReconcilable(format!("no usable reading of '{capability}'"))
            })?;
        // A reading from before the command was sent says nothing about it. One from before
        // the gateway gave up waiting cannot rule out a device that was merely slow, so it
        // cannot support "not applied", the conclusion that invites a resend.
        let matched = openmhs_policy::readback_matches(&expected, &observed.value, tolerance);
        let since = self.dispatched_at(command_id)?;
        let gave_up = receipt
            .transitions
            .iter()
            .rev()
            .find(|t| t.state == UnknownOutcome)
            .map(|t| t.at);
        let floor = if matched { since } else { gave_up.or(since) };
        if !self.measured_since(&observed, floor) {
            return Err(EngineError::NotReconcilable(format!(
                "the only reading of '{capability}' available was measured at {}, older than the \
                 command ({}); it cannot show whether the command took effect. Try again when \
                 the device reports a current value, or have an operator resolve it",
                observed.timestamp.to_rfc3339(),
                if matched { "sent" } else { "given up on" },
            )));
        }
        let to = if matched {
            ReconciledApplied
        } else {
            ReconciledNotApplied
        };
        let detail = format!("reconciled by readback {}", describe_value(&observed.value));
        self.transition(command_id, to, Some(detail), |r| {
            r.verification = Some(VerificationRecord {
                mode: VerificationMode::Readback,
                capability: Some(capability),
                expected: Some(expected),
                observed: Some(observed),
                matched: Some(matched),
                reason: None,
            });
        })
    }

    /// Settle an `UNKNOWN_OUTCOME` command on a person's word, with the reason recorded
    /// (spec/lifecycle.md §5): for a capability without readback, or a device that cannot
    /// be read. The device is not contacted, and the command is never re-sent.
    pub fn resolve(
        &self,
        caller: &Caller,
        command_id: &str,
        applied: bool,
        reason: &str,
    ) -> Result<Receipt, EngineError> {
        let principal = caller.principal();
        if caller.kind() == PrincipalKind::Agent {
            return Err(EngineError::Denied(ErrorObject::new(
                "policy.resolve.not_human",
                "an agent credential cannot resolve an unknown outcome; a person must check the device",
            )));
        }
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(EngineError::InvalidRequest(vec![
                ErrorObject::new(
                    "command.resolve.reason",
                    "say how the outcome was established, e.g. 'checked the display: 5.00 V'",
                )
                .at("/reason"),
            ]));
        }
        let receipt = self
            .receipt(command_id)?
            .ok_or_else(|| EngineError::UnknownCommand(command_id.into()))?;
        let target = &receipt.request.target;
        let operation: Operation = receipt.request.operation.into();
        if !caller.in_scope(&target.device, &target.capability, operation) {
            return Err(EngineError::Denied(scope_denied(
                caller,
                &target.device,
                &target.capability,
                operation,
            )));
        }
        if receipt.state != UnknownOutcome {
            return Err(EngineError::InvalidState {
                id: command_id.into(),
                state: receipt.state,
                action: "resolve",
            });
        }
        let to = if applied {
            ReconciledApplied
        } else {
            ReconciledNotApplied
        };
        self.transition(
            command_id,
            to,
            Some(format!("resolved by {principal}: {reason}")),
            |_| {},
        )
    }

    /// Bring unfinished commands to a safe state after a restart (spec/lifecycle.md §4).
    /// Call after registering devices.
    pub async fn recover(&self) -> Result<RecoveryReport, EngineError> {
        let mut report = RecoveryReport::default();
        let pending = self.inner.journal.commands_in_states(&[
            Created,
            Validating,
            Approved,
            AwaitingApproval,
            Dispatched,
            Accepted,
            TimedOut,
            Applied,
        ])?;
        for r in pending {
            let id = r.command_id.clone();
            match r.state {
                Created | Validating => {
                    if r.state == Created {
                        self.transition(&id, Validating, None, |_| {})?;
                    }
                    let e = ErrorObject::new(
                        "gateway.restarted",
                        "gateway restarted before validation finished; resubmit",
                    );
                    self.deny(&id, vec![e], vec![])?;
                    report.not_executed.push(id);
                }
                Approved => {
                    self.transition(
                        &id,
                        Expired,
                        Some("gateway restarted before dispatch; never sent".into()),
                        |_| {},
                    )?;
                    report.not_executed.push(id);
                }
                AwaitingApproval => {
                    if r.deadline_at.is_some_and(|d| self.now() >= d) {
                        self.transition(
                            &id,
                            Expired,
                            Some("deadline passed while awaiting approval".into()),
                            |_| {},
                        )?;
                        report.not_executed.push(id);
                    }
                }
                Dispatched | Accepted | TimedOut => {
                    self.transition(
                        &id,
                        UnknownOutcome,
                        Some("gateway restarted during execution; not retried".into()),
                        |_| {},
                    )?;
                    report.unknown_outcome.push(id);
                }
                Applied => {
                    let device = self.device(&r.request.target.device);
                    let expectation = self.inner.journal.expectation(&id)?;
                    match (device, expectation) {
                        (Some(device), Some(expectation)) => {
                            let _guard = device.lock.lock().await;
                            self.verify(&device, &id, &expectation).await?;
                            report.verification_resumed.push(id);
                        }
                        _ => report.skipped.push(id),
                    }
                }
                _ => {}
            }
        }
        self.sweep_approvals()?;
        self.inner.journal.expire_leases(self.now())?;
        report.plans_awaiting_decision = self.recover_plans()?;
        self.inner.journal.append_audit(
            self.now(),
            "gateway.recovered",
            "engine",
            &serde_json::json!({
                "unknown_outcome": report.unknown_outcome,
                "not_executed": report.not_executed,
                "verification_resumed": report.verification_resumed,
                "skipped": report.skipped,
                "plans_awaiting_decision": report.plans_awaiting_decision,
            }),
        )?;
        Ok(report)
    }
}

/// The denial for a call outside its credential's scopes. Says what was refused, never
/// what other scopes or devices exist.
fn scope_denied(
    caller: &Caller,
    device: &str,
    capability: &str,
    operation: Operation,
) -> ErrorObject {
    ErrorObject::new(
        "auth.scope.denied",
        format!(
            "credential {} of '{}' has no scope for {operation:?} on {device} / {capability}",
            caller.credential_id().unwrap_or("(in-process)"),
            caller.principal(),
        ),
    )
}

/// How the caller proved who they are, for the `command.created` audit record.
fn origin(caller: &Caller) -> serde_json::Value {
    serde_json::json!({
        "credential_id": caller.credential_id(),
        "auth_method": caller.method(),
    })
}

fn describe_value(v: &openmhs_types::Value) -> String {
    match v {
        openmhs_types::Value::Quantity(q) => format!("{} {}", q.value, q.unit),
        openmhs_types::Value::Integer(i) => i.to_string(),
        openmhs_types::Value::Bool(b) => b.to_string(),
        openmhs_types::Value::Text(s) => format!("'{s}'"),
    }
}

// The gateway shares one engine across tasks.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Engine>();
};

/// The detail recorded (and so hash-chained) when the device acknowledges a command: the
/// device's own reference for the work, so the audit trail links this command to the device's
/// record of it. `None` when the adapter gave no reference.
fn accepted_detail(external_ref: Option<&str>) -> Option<String> {
    external_ref.map(|r| format!("device reference {r}"))
}

/// SHA-256 of the validated descriptor in canonical form (sorted keys, no whitespace): which
/// capability contract was in force when a device was registered.
fn descriptor_sha256(descriptor: &Descriptor) -> String {
    let value = serde_json::to_value(descriptor).unwrap_or(serde_json::Value::Null);
    sha256_hex(&openmhs_types::canonical_json(&value))
}
