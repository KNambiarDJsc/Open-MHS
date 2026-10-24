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
mod leases;

pub use approvals::{DEFAULT_APPROVAL_TTL_MS, MAX_APPROVAL_TTL_MS, fingerprint};
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
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            read_timeout_ms: 1_000,
            verify_poll_ms: 50,
        }
    }
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
}

struct Device {
    descriptor: Descriptor,
    adapter: Arc<dyn HardwareAdapter>,
    /// Serializes state-changing commands on this device, from live checks to verification.
    lock: Arc<tokio::sync::Mutex<()>>,
    /// Dispatched mutations per capability, for rate and slew limits.
    history: Mutex<HashMap<String, Vec<Mutation>>>,
}

struct Inner {
    clock: Arc<dyn Clock>,
    journal: Journal,
    policy: RwLock<Policy>,
    devices: RwLock<HashMap<String, Arc<Device>>>,
    /// The operator's installation-specific relationships (spec/topology.md).
    topology: RwLock<Vec<TopologyRelationship>>,
    state: Mutex<HashMap<(String, String), StateValue>>,
    options: EngineOptions,
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
                options,
            }),
        }
    }

    fn now(&self) -> Timestamp {
        self.inner.clock.now()
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

    /// Register a device. Its descriptor comes from the adapter and must validate.
    /// Registration grants no permissions (spec/authorization.md §1).
    pub async fn register(
        &self,
        adapter: Arc<dyn HardwareAdapter>,
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
        adapter.connect().await.map_err(EngineError::Adapter)?;
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
                }),
            );
        }
        self.inner.journal.append_audit(
            self.now(),
            "device.registered",
            &id,
            &serde_json::json!({
                "adapter": descriptor.adapter,
                "manufacturer": descriptor.device.manufacturer,
                "model": descriptor.device.model,
                "capabilities": descriptor.capabilities.len(),
            }),
        )?;
        info!(device = %id, "device registered");
        Ok(descriptor)
    }

    /// Ask every adapter to shut down. Best effort: failures are logged.
    pub async fn shutdown_adapters(&self) {
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
        let result = self.read_device(&device, capability).await;
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
        }
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
        Ok(r)
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

        // The stop lane: a stop does not wait for the command it may be meant to stop.
        // It has no preconditions or rate limit to protect (descriptor rule 14).
        let guard = if cap.stop {
            None
        } else {
            Some(device.lock.clone().lock_owned().await)
        };
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
            match self.read_device(device, &name).await {
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
                Some(ProgressEvent::Accepted) = rx.recv(), if !accepted => {
                    accepted = true;
                    self.transition(id, Accepted, None, |r| {
                        r.adapter_result.get_or_insert_with(AdapterResult::default).acknowledged = Some(true);
                    })?;
                }
                () = &mut sleep => {
                    handle.abort();
                    break ExecOutcome::TimedOut;
                }
            }
        };
        if !accepted && matches!(rx.try_recv(), Ok(ProgressEvent::Accepted)) {
            accepted = true;
            self.transition(id, Accepted, None, |r| {
                r.adapter_result
                    .get_or_insert_with(AdapterResult::default)
                    .acknowledged = Some(true);
            })?;
        }

        let with_error = |e: ErrorObject| {
            move |r: &mut Receipt| {
                r.adapter_result
                    .get_or_insert_with(AdapterResult::default)
                    .error = Some(e);
            }
        };

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
        let mut last: Option<StateValue> = None;
        loop {
            if let Ok(v) = self.read_device(device, &capability).await
                && v.is_usable(self.now(), None)
            {
                if openmhs_policy::readback_matches(&expected, &v.value, tolerance) {
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
                }
                last = Some(v);
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
                Some("no usable readback within the verification timeout".into()),
                |r| {
                    r.verification = Some(VerificationRecord {
                        mode: VerificationMode::Readback,
                        capability: Some(capability),
                        expected: Some(expected),
                        observed: None,
                        matched: None,
                        reason: Some("readback source unavailable or not GOOD quality".into()),
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
                "no readback is declared for this capability; an operator must resolve it".into(),
            ));
        };
        let device = self
            .device(&receipt.request.target.device)
            .ok_or_else(|| EngineError::NotReconcilable("device is not registered".into()))?;
        let _guard = device.lock.lock().await;
        let observed = self
            .read_device(&device, &capability)
            .await
            .ok()
            .filter(|v| v.is_usable(self.now(), None))
            .ok_or_else(|| {
                EngineError::NotReconcilable(format!("no usable reading of '{capability}'"))
            })?;
        let matched = openmhs_policy::readback_matches(&expected, &observed.value, tolerance);
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
        self.inner.journal.append_audit(
            self.now(),
            "gateway.recovered",
            "engine",
            &serde_json::json!({
                "unknown_outcome": report.unknown_outcome,
                "not_executed": report.not_executed,
                "verification_resumed": report.verification_resumed,
                "skipped": report.skipped,
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
