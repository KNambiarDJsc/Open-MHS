//! Twin preflight (ADR-0018, spec/plans.md §8). A plan is dry-run in a separate,
//! in-memory engine whose devices are twins built from the real descriptors and seeded
//! with fresh readings of the real devices. The twin engine has the same policy, the same
//! topology and the same rate and slew history, and the plan runs there as the same
//! caller, so it meets every check a real run would: scopes, policy, leases, units,
//! ranges, rates, slews, preconditions and declared verification.
//!
//! A twin reads back what it was told. It has no physics: it cannot know that a device
//! is slow, stalls, or drifts.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use openmhs_adapter::{
    AdapterCommand, AdapterError, AdapterErrorKind, ExecuteReport, HardwareAdapter, Health,
    Progress,
};
use openmhs_policy::Caller;
use openmhs_types::descriptor::{CapabilityKind, Operation, Verification};
use openmhs_types::plan::{
    DEFAULT_PLAN_LEASE_TTL_MS, LeaseCheck, PlanPreflight, PlanSpec, PlanState, StepPreflight,
    StepStatus, StepVerdict,
};
use openmhs_types::{Clock, CommandState, Descriptor, ErrorObject, Quality, StateValue, Value};

use super::{Engine, LeaseRequest, lock};
use crate::EngineError;

/// The approver the twin uses to wave held steps through, so the steps after them are
/// checked too. It exists only inside the twin engine.
const TWIN_APPROVER: &str = "operator:openmhs-twin";

/// A device that holds the values it was last given.
struct Twin {
    descriptor: Descriptor,
    clock: Arc<dyn Clock>,
    /// Current value per capability, and how old it was when seeded (kept, so a stale
    /// real reading is stale in the twin too).
    values: Mutex<BTreeMap<String, (Value, u64, Quality)>>,
}

impl Twin {
    fn set(&self, capability: &str, value: Value) {
        lock(&self.values).insert(capability.to_owned(), (value, 0, Quality::Good));
    }
}

#[async_trait]
impl HardwareAdapter for Twin {
    fn adapter_id(&self) -> &str {
        &self.descriptor.adapter.id
    }

    async fn describe(&self) -> Result<serde_json::Value, AdapterError> {
        serde_json::to_value(&self.descriptor)
            .map_err(|e| AdapterError::new(AdapterErrorKind::Internal, e.to_string()))
    }

    async fn connect(&self) -> Result<(), AdapterError> {
        Ok(())
    }

    async fn health(&self) -> Health {
        Health::Healthy
    }

    async fn read(&self, capability: &str) -> Result<StateValue, AdapterError> {
        let values = lock(&self.values);
        let Some((value, age_ms, quality)) = values.get(capability) else {
            // No real reading to seed it: fail closed, as the real device would look.
            return Err(AdapterError::new(
                AdapterErrorKind::Disconnected,
                format!("the twin has no reading of '{capability}'"),
            ));
        };
        Ok(StateValue {
            value: value.clone(),
            timestamp: self.clock.now().minus_ms(*age_ms),
            source: openmhs_types::Source::Simulator,
            quality: *quality,
        })
    }

    async fn execute(
        &self,
        command: AdapterCommand,
        _progress: Progress,
    ) -> Result<ExecuteReport, AdapterError> {
        let Some(cap) = self.descriptor.capability(&command.capability) else {
            return Err(AdapterError::new(
                AdapterErrorKind::Unsupported,
                format!("no capability '{}'", command.capability),
            ));
        };
        if cap.kind == CapabilityKind::Property
            && let Some(v) = &command.value
        {
            self.set(&cap.name, v.clone());
        }
        // Make the declared readback come true: the twin does what it is told.
        if let Some(Verification::Readback {
            capability,
            expect,
            expect_param,
            ..
        }) = &cap.verification
        {
            let target = capability.clone().unwrap_or_else(|| cap.name.clone());
            let value = expect
                .clone()
                .or_else(|| {
                    expect_param
                        .as_ref()
                        .and_then(|p| command.params.get(p).cloned())
                })
                .or_else(|| command.value.clone());
            if let Some(v) = value {
                self.set(&target, v);
            }
        }
        Ok(ExecuteReport::Applied {
            result: BTreeMap::new(),
        })
    }

    async fn shutdown(&self) -> Result<(), AdapterError> {
        Ok(())
    }
}

fn verdict(status: StepStatus, held: bool) -> StepVerdict {
    match status {
        StepStatus::Succeeded
        | StepStatus::Compensating
        | StepStatus::Compensated
        | StepStatus::CompensationFailed
        | StepStatus::CompensationUncertain
            if held =>
        {
            StepVerdict::NeedsApproval
        }
        StepStatus::Succeeded
        | StepStatus::Compensating
        | StepStatus::Compensated
        | StepStatus::CompensationFailed
        | StepStatus::CompensationUncertain => StepVerdict::Allowed,
        StepStatus::Failed | StepStatus::Uncertain => StepVerdict::Refused,
        StepStatus::Pending | StepStatus::Running | StepStatus::WaitingApproval => {
            StepVerdict::NotReached
        }
    }
}

impl Engine {
    /// Dry-run a plan against twins of its devices (ADR-0018). Only reads reach real
    /// devices. Audited. Everything the twin engine logs is inside a `twin` span, so its
    /// registrations and command transitions are never mistaken for real ones.
    pub async fn preflight_plan(
        &self,
        caller: &Caller,
        spec: PlanSpec,
    ) -> Result<PlanPreflight, EngineError> {
        use tracing::Instrument;
        self.preflight_in_twin(caller, spec)
            .instrument(tracing::info_span!("twin"))
            .await
    }

    async fn preflight_in_twin(
        &self,
        caller: &Caller,
        spec: PlanSpec,
    ) -> Result<PlanPreflight, EngineError> {
        let order = self
            .check_plan(&spec)
            .map_err(EngineError::InvalidRequest)?;
        let devices = super::plans::plan_devices(&spec);

        // 1. The lease, against the real gateway's live leases.
        let request = LeaseRequest {
            devices: devices.clone(),
            resources: vec![],
            ttl_ms: spec.lease_ttl_ms.unwrap_or(DEFAULT_PLAN_LEASE_TTL_MS),
            reason: None,
            plan_id: None,
        };
        let lease = match self.lease_would_conflict(caller, request) {
            Ok((l, None)) => LeaseCheck {
                available: true,
                claim: l.claim,
                error: None,
            },
            Ok((l, Some(other))) => LeaseCheck {
                available: false,
                claim: l.claim,
                error: Some(ErrorObject::new(
                    "lease.conflict",
                    format!(
                        "held by {} under lease {} until {}",
                        other.holder, other.id, other.expires_at
                    ),
                )),
            },
            Err(EngineError::Denied(e)) => LeaseCheck {
                available: false,
                claim: openmhs_types::graph::Claim::default(),
                error: Some(e),
            },
            Err(e) => return Err(e),
        };

        // 2. The twin: every registered device, seeded with fresh readings of the ones
        // the plan reads or changes, under the same policy and topology.
        let mut policy = self.policy();
        policy.approvers.push(TWIN_APPROVER.into());
        let twin = Engine::in_memory(self.inner.clock.clone(), policy)?;
        let mut touched: BTreeSet<String> = devices.iter().cloned().collect();
        for s in &spec.steps {
            touched.extend(s.preconditions.iter().map(|p| p.device.clone()));
        }
        let mut reads = 0usize;
        for descriptor in self.devices() {
            let id = descriptor.device.id.clone();
            let adapter = Arc::new(Twin {
                descriptor: descriptor.clone(),
                clock: self.inner.clock.clone(),
                values: Mutex::new(BTreeMap::new()),
            });
            if touched.contains(&id)
                && let Some(real) = self.device(&id)
            {
                for cap in &descriptor.capabilities {
                    if cap.value.is_none() || !cap.supports(Operation::Read) {
                        continue;
                    }
                    if let Ok(v) = self.read_device(&real, &cap.name).await {
                        let age = u64::try_from(v.timestamp.ms_until(self.now())).unwrap_or(0);
                        lock(&adapter.values).insert(cap.name.clone(), (v.value, age, v.quality));
                        reads += 1;
                    }
                }
            }
            twin.register(adapter).await?;
            if let (Some(real), Some(copy)) = (self.device(&id), twin.device(&id)) {
                *lock(&copy.history) = lock(&real.history).clone();
            }
        }
        *twin
            .inner
            .topology
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = self
            .inner
            .topology
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        // 3. Run it, approving anything policy holds so the rest is checked too.
        let approver = Caller::in_process(TWIN_APPROVER);
        let mut held: BTreeSet<(String, bool)> = BTreeSet::new();
        let (outcome, mut steps): (PlanState, Vec<StepPreflight>) =
            match twin.submit_plan(caller, spec.clone()).await {
                Ok(mut plan) => {
                    let mut rounds = 0;
                    while plan.state == PlanState::WaitingApproval && rounds <= 2 * order.len() {
                        rounds += 1;
                        let Some(step) = plan.current.clone() else {
                            break;
                        };
                        let Some(record) = plan.step(&step) else {
                            break;
                        };
                        let undo = plan.compensating;
                        let command = if undo {
                            record.compensations.last()
                        } else {
                            record.commands.last()
                        };
                        let Some(command) = command.cloned() else {
                            break;
                        };
                        held.insert((step, undo));
                        if twin
                            .receipt(&command)?
                            .is_some_and(|r| r.state == CommandState::AwaitingApproval)
                        {
                            twin.approve(&approver, &command).await?;
                        }
                        plan = twin.load_plan(&plan.id)?;
                    }
                    let steps = order
                        .iter()
                        .map(|id| {
                            let r = plan.step(id);
                            let status = r.map_or(StepStatus::Pending, |r| r.status);
                            let compensation =
                                r.filter(|r| !r.compensations.is_empty())
                                    .map(|r| match r.status {
                                        StepStatus::Compensated
                                            if held.contains(&(id.clone(), true)) =>
                                        {
                                            StepVerdict::NeedsApproval
                                        }
                                        StepStatus::Compensated => StepVerdict::Allowed,
                                        _ => StepVerdict::Refused,
                                    });
                            StepPreflight {
                                id: id.clone(),
                                verdict: verdict(status, held.contains(&(id.clone(), false))),
                                errors: r.and_then(|r| r.error.clone()).into_iter().collect(),
                                compensation,
                            }
                        })
                        .collect();
                    (plan.state, steps)
                }
                // The twin could not even lease: report it on every step.
                Err(EngineError::Denied(e)) => {
                    let steps = order
                        .iter()
                        .map(|id| StepPreflight {
                            id: id.clone(),
                            verdict: StepVerdict::NotReached,
                            errors: vec![],
                            compensation: None,
                        })
                        .collect::<Vec<_>>();
                    let mut lease = lease.clone();
                    lease.available = false;
                    lease.error.get_or_insert(e);
                    return self.preflight_result(
                        caller,
                        PlanState::Halted,
                        lease,
                        order,
                        steps,
                        reads,
                    );
                }
                Err(e) => return Err(e),
            };
        // A step the twin never reached cannot carry the error of a later one.
        for s in &mut steps {
            if s.verdict == StepVerdict::NotReached {
                s.errors.clear();
            }
        }
        self.preflight_result(caller, outcome, lease, order, steps, reads)
    }

    fn preflight_result(
        &self,
        caller: &Caller,
        outcome: PlanState,
        lease: LeaseCheck,
        order: Vec<String>,
        steps: Vec<StepPreflight>,
        reads: usize,
    ) -> Result<PlanPreflight, EngineError> {
        let ok = lease.available
            && outcome == PlanState::Succeeded
            && steps
                .iter()
                .all(|s| matches!(s.verdict, StepVerdict::Allowed | StepVerdict::NeedsApproval));
        let result = PlanPreflight {
            ok,
            lease,
            outcome,
            order,
            steps,
            checked_at: self.now(),
        };
        self.inner.journal.append_audit(
            result.checked_at,
            "plan.preflight",
            caller.principal(),
            &serde_json::json!({
                "credential_id": caller.credential_id(),
                "ok": result.ok,
                "outcome": result.outcome,
                "lease_available": result.lease.available,
                "steps": result.steps.iter().map(|s| (s.id.clone(), s.verdict)).collect::<Vec<_>>(),
                "device_reads": reads,
            }),
        )?;
        Ok(result)
    }
}
