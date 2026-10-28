//! Plans (ADR-0018, spec/plans.md). A plan holds one lease over every device its steps
//! touch and submits its steps one at a time, each through [`Engine::submit`] as the plan's
//! caller, so a step gets exactly the checks, receipt and audit of any other command. The
//! plan only decides what to submit next, and stops for a person whenever the outcome is
//! not definite.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use openmhs_policy::{Caller, CallerRecord, PrincipalKind};
use openmhs_types::plan::{
    DEFAULT_PLAN_LEASE_TTL_MS, DecisionRecord, OnFailure, Plan, PlanDecision, PlanSpec, PlanState,
    PlanStep, StepRecord, StepStatus, execution_order,
};
use openmhs_types::schema::{self, Schema};
use openmhs_types::{
    CommandRequest, CommandState, ErrorObject, Outcome, Receipt, RequestOperation,
};

use super::{Engine, LeaseRequest, lock};
use crate::EngineError;
use crate::journal::JournalError;

/// Per-plan driving locks and pending cancellations. Only one task moves a plan at a time;
/// a cancellation asks the driver to stop at the next step boundary.
#[derive(Default)]
pub(super) struct PlanRuntime {
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    cancelling: Mutex<HashSet<String>>,
}

fn refused(code: &str, message: impl Into<String>) -> EngineError {
    EngineError::Denied(ErrorObject::new(code, message))
}

/// How a step's (or compensation's) command ended, from the plan's point of view.
enum Settled {
    Done,
    Held,
    Failed(ErrorObject),
    Uncertain(ErrorObject),
}

fn settle(r: &Receipt) -> Settled {
    let why = || {
        r.policy
            .errors
            .first()
            .cloned()
            .or_else(|| r.adapter_result.as_ref().and_then(|a| a.error.clone()))
            .unwrap_or_else(|| {
                ErrorObject::new(
                    "plan.step.failed",
                    format!("command {} ended {:?}", r.command_id, r.state),
                )
            })
            .detail("command_id", r.command_id.clone())
    };
    match r.state.outcome() {
        Outcome::Succeeded | Outcome::Unverified => Settled::Done,
        Outcome::Pending if r.state == CommandState::AwaitingApproval => Settled::Held,
        Outcome::Failed | Outcome::Denied | Outcome::NotExecuted => Settled::Failed(why()),
        // Anything still in flight or unresolved is treated as possibly applied.
        Outcome::Uncertain | Outcome::Pending => Settled::Uncertain(
            ErrorObject::new(
                "plan.step.uncertain",
                format!(
                    "command {} is {:?}: it may or may not have taken effect; reconcile it, then decide",
                    r.command_id, r.state
                ),
            )
            .detail("command_id", r.command_id.clone()),
        ),
    }
}

/// An engine error that ends a step, as opposed to one the plan cannot continue past.
fn step_error(e: EngineError) -> Result<ErrorObject, EngineError> {
    match e {
        EngineError::Journal(_) => Err(e),
        EngineError::Denied(e) => Ok(e),
        EngineError::InvalidRequest(mut v) if !v.is_empty() => Ok(v.remove(0)),
        other => Ok(ErrorObject::new("plan.step.failed", other.to_string())),
    }
}

fn parse_request(
    value: &serde_json::Value,
    step: &str,
    what: &str,
) -> Result<CommandRequest, ErrorObject> {
    let bad = |msg: String| {
        ErrorObject::new("plan.step.invalid_request", msg)
            .detail("step", step.to_owned())
            .detail("field", what.to_owned())
    };
    let issues = schema::validate(Schema::Command, value);
    if let Some(first) = issues.first() {
        return Err(bad(format!("step '{step}' {what}: {first}")));
    }
    let req: CommandRequest = serde_json::from_value(value.clone())
        .map_err(|e| bad(format!("step '{step}' {what}: {e}")))?;
    if req.operation == RequestOperation::Read {
        return Err(bad(format!(
            "step '{step}' {what} is a read; plan steps change devices (use preconditions to read)"
        )));
    }
    if req.idempotency_key.is_some() {
        return Err(bad(format!(
            "step '{step}' {what} carries an idempotency_key; the plan derives one per attempt"
        )));
    }
    Ok(req)
}

fn with_key(value: &serde_json::Value, key: &str) -> serde_json::Value {
    let mut v = value.clone();
    v["idempotency_key"] = serde_json::Value::String(key.to_owned());
    v
}

fn step_key(plan: &str, step: &str, attempt: usize) -> String {
    format!("plan:{plan}:{step}:{attempt}")
}

fn undo_key(plan: &str, step: &str, attempt: usize) -> String {
    format!("plan:{plan}:{step}:undo:{attempt}")
}

/// Every device a plan's steps and compensations act on.
pub(super) fn plan_devices(spec: &PlanSpec) -> Vec<String> {
    let mut out = BTreeSet::new();
    for s in &spec.steps {
        for v in std::iter::once(&s.request).chain(s.compensate.as_ref()) {
            if let Some(d) = v["target"]["device"].as_str() {
                out.insert(d.to_owned());
            }
        }
    }
    out.into_iter().collect()
}

impl Engine {
    /// Check a plan's structure, its requests and its preconditions' targets, all at once.
    pub(super) fn check_plan(&self, spec: &PlanSpec) -> Result<Vec<String>, Vec<ErrorObject>> {
        let order = execution_order(spec)?;
        let mut errors = Vec::new();
        for s in &spec.steps {
            if let Err(e) = parse_request(&s.request, &s.id, "request") {
                errors.push(e);
            }
            if let Some(c) = &s.compensate
                && let Err(e) = parse_request(c, &s.id, "compensate")
            {
                errors.push(e);
            }
            for p in &s.preconditions {
                let readable = self
                    .descriptor(&p.device)
                    .and_then(|d| d.capability(&p.capability).cloned())
                    .is_some_and(|c| c.value.is_some());
                if !readable {
                    errors.push(
                        ErrorObject::new(
                            "plan.precondition.unknown",
                            format!(
                                "step '{}' precondition reads '{}' on '{}', which is not a readable property of a registered device",
                                s.id, p.capability, p.device
                            ),
                        )
                        .detail("step", s.id.clone()),
                    );
                }
            }
        }
        if errors.is_empty() {
            Ok(order)
        } else {
            Err(errors)
        }
    }

    /// Submit a plan: check it, lease its devices, and run it until it ends or waits for
    /// an approval or a decision. Invalid plans are refused whole (`InvalidRequest`); a
    /// lease that cannot be taken refuses the plan with the lease's code.
    pub async fn submit_plan(&self, caller: &Caller, spec: PlanSpec) -> Result<Plan, EngineError> {
        let order = self
            .check_plan(&spec)
            .map_err(EngineError::InvalidRequest)?;
        let id = uuid::Uuid::now_v7().to_string();
        let lease = self.acquire_lease(
            caller,
            LeaseRequest {
                devices: plan_devices(&spec),
                resources: vec![],
                ttl_ms: spec.lease_ttl_ms.unwrap_or(DEFAULT_PLAN_LEASE_TTL_MS),
                reason: Some(format!("plan {id}")),
                plan_id: Some(id.clone()),
            },
        )?;
        let now = self.now();
        let plan = Plan {
            id: id.clone(),
            principal: caller.principal().to_owned(),
            credential_id: caller.credential_id().map(str::to_owned),
            state: PlanState::Running,
            steps: order
                .iter()
                .map(|s| StepRecord {
                    id: s.clone(),
                    status: StepStatus::Pending,
                    commands: vec![],
                    compensations: vec![],
                    error: None,
                })
                .collect(),
            order,
            spec,
            current: None,
            lease_id: Some(lease.id.clone()),
            compensating: false,
            reason: None,
            decisions: vec![],
            created_at: now,
            updated_at: now,
        };
        let caller_json = serde_json::to_string(&caller.record()).map_err(JournalError::from)?;
        if let Err(e) = self.inner.journal.insert_plan(&plan, &caller_json) {
            let _ = self.inner.journal.release_lease(
                &lease.id,
                caller.principal(),
                now,
                Some("plan not recorded"),
            );
            return Err(e.into());
        }
        self.drive(&id, None).await
    }

    /// A plan, if the caller may see it: an agent sees only its own (others answer
    /// `plan.unknown`, as commands do).
    pub fn plan(&self, caller: &Caller, id: &str) -> Result<Plan, EngineError> {
        let unknown = || refused("plan.unknown", format!("unknown plan {id}"));
        let plan = self.inner.journal.plan(id)?.ok_or_else(unknown)?;
        if caller.kind() == PrincipalKind::Agent && plan.principal != caller.principal() {
            return Err(unknown());
        }
        Ok(plan)
    }

    /// Recent plans the caller may see, newest first.
    pub fn plans(&self, caller: &Caller, limit: usize) -> Result<Vec<Plan>, EngineError> {
        let mut plans = self.inner.journal.plans(limit)?;
        if caller.kind() == PrincipalKind::Agent {
            plans.retain(|p| p.principal == caller.principal());
        }
        Ok(plans)
    }

    pub(super) fn load_plan(&self, id: &str) -> Result<Plan, EngineError> {
        self.inner
            .journal
            .plan(id)?
            .ok_or_else(|| refused("plan.unknown", format!("unknown plan {id}")))
    }

    fn plan_lock(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        lock(&self.inner.plans.locks)
            .entry(id.to_owned())
            .or_default()
            .clone()
    }

    fn save_plan(
        &self,
        plan: &mut Plan,
        kind: &str,
        detail: serde_json::Value,
    ) -> Result<(), EngineError> {
        plan.updated_at = self.now();
        let mut detail = detail;
        detail["state"] = serde_json::json!(plan.state);
        Ok(self.inner.journal.update_plan(plan, kind, &detail)?)
    }

    /// End the plan in `state` and free its devices.
    fn finish(
        &self,
        plan: &mut Plan,
        state: PlanState,
        reason: Option<String>,
    ) -> Result<(), EngineError> {
        if let Some(lease) = &plan.lease_id {
            match self.inner.journal.release_lease(
                lease,
                &plan.principal,
                self.now(),
                Some(&format!("plan {} ended {}", plan.id, state.name())),
            ) {
                Ok(_) | Err(JournalError::Refused(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        plan.state = state;
        plan.reason = reason;
        let kind = format!("plan.{}", state.name().to_lowercase());
        let detail = serde_json::json!({ "reason": plan.reason });
        self.save_plan(plan, &kind, detail)
    }

    fn wait_for_decision(&self, plan: &mut Plan, reason: String) -> Result<(), EngineError> {
        plan.state = PlanState::AwaitingDecision;
        plan.reason = Some(reason);
        let detail = serde_json::json!({ "step": plan.current, "reason": plan.reason });
        self.save_plan(plan, "plan.awaiting_decision", detail)
    }

    fn caller_of(&self, id: &str) -> Result<Caller, EngineError> {
        let json = self
            .inner
            .journal
            .plan_caller(id)?
            .ok_or_else(|| refused("plan.unknown", format!("unknown plan {id}")))?;
        let record: CallerRecord = serde_json::from_str(&json).map_err(JournalError::from)?;
        Ok(Caller::from_record(&record))
    }

    /// Run the plan under its lock until it ends or waits. `held` is a command of the
    /// plan that just left `AWAITING_APPROVAL`, to be taken into account first.
    async fn drive(&self, id: &str, held: Option<&str>) -> Result<Plan, EngineError> {
        let gate = self.plan_lock(id);
        let _guard = gate.lock().await;
        let caller = self.caller_of(id)?;
        let mut plan = self.load_plan(id)?;
        if let Some(command_id) = held {
            if plan.state != PlanState::WaitingApproval {
                return Ok(plan);
            }
            let Some(receipt) = self.inner.journal.receipt(command_id)? else {
                return Ok(plan);
            };
            if receipt.state == CommandState::AwaitingApproval {
                return Ok(plan);
            }
            let step = plan.current.clone().unwrap_or_default();
            plan.state = if plan.compensating {
                PlanState::Compensating
            } else {
                PlanState::Running
            };
            self.apply(&mut plan, &step, &receipt)?;
        }
        loop {
            if lock(&self.inner.plans.cancelling).contains(id) {
                return Ok(plan);
            }
            match plan.state {
                PlanState::Running => self.next_step(&caller, &mut plan).await?,
                PlanState::Compensating => self.next_compensation(&caller, &mut plan).await?,
                _ => return Ok(plan),
            }
        }
    }

    /// Keep the plan's lease alive for the next step, taking a new one if it ran out.
    fn keep_lease(&self, caller: &Caller, plan: &mut Plan) -> Result<(), ErrorObject> {
        let now = self.now();
        let ttl = plan.spec.lease_ttl_ms.unwrap_or(DEFAULT_PLAN_LEASE_TTL_MS);
        if let Some(lease) = &plan.lease_id
            && self
                .inner
                .journal
                .renew_lease(lease, &plan.principal, now.plus_ms(ttl), now)
                .is_ok()
        {
            return Ok(());
        }
        let lease = self
            .acquire_lease(
                caller,
                LeaseRequest {
                    devices: plan_devices(&plan.spec),
                    resources: vec![],
                    ttl_ms: ttl,
                    reason: Some(format!("plan {}", plan.id)),
                    plan_id: Some(plan.id.clone()),
                },
            )
            .map_err(|e| match step_error(e) {
                Ok(e) | Err(EngineError::Denied(e)) => e,
                Err(other) => ErrorObject::new("gateway.journal", other.to_string()),
            })?;
        plan.lease_id = Some(lease.id);
        Ok(())
    }

    async fn check_preconditions(
        &self,
        caller: &Caller,
        step: &PlanStep,
    ) -> Result<(), ErrorObject> {
        for p in &step.preconditions {
            let failed = |msg: String| {
                ErrorObject::new("plan.precondition.failed", msg)
                    .detail("step", step.id.clone())
                    .detail("device", p.device.clone())
                    .detail("capability", p.capability.clone())
            };
            let spec = self
                .descriptor(&p.device)
                .and_then(|d| d.capability(&p.capability).and_then(|c| c.value.clone()))
                .ok_or_else(|| {
                    failed(format!(
                        "'{}' on '{}' is no longer readable",
                        p.capability, p.device
                    ))
                })?;
            let reading = self
                .read(caller, &p.device, &p.capability)
                .await
                .map_err(|e| {
                    failed(format!(
                        "could not read '{}' on '{}': {e}",
                        p.capability, p.device
                    ))
                })?;
            if !reading.is_usable(self.now(), p.max_age_ms) {
                return Err(failed(format!(
                    "the reading of '{}' on '{}' is stale or of bad quality",
                    p.capability, p.device
                )));
            }
            match openmhs_policy::condition_holds(&spec, &reading.value, p.op, &p.value) {
                Ok(true) => {}
                Ok(false) => {
                    return Err(failed(p.description.clone().unwrap_or_else(|| {
                        format!(
                            "'{}' on '{}' does not satisfy the step's precondition",
                            p.capability, p.device
                        )
                    }))
                    .detail(
                        "observed",
                        serde_json::to_value(&reading.value).unwrap_or_default(),
                    ));
                }
                Err(msg) => return Err(failed(msg)),
            }
        }
        Ok(())
    }

    /// Submit a request for a step (or its compensation) and record the command.
    async fn submit_for(
        &self,
        caller: &Caller,
        plan: &mut Plan,
        step: &str,
        request: &serde_json::Value,
        undo: bool,
    ) -> Result<Result<Receipt, ErrorObject>, EngineError> {
        let attempt = plan.step(step).map_or(0, |s| {
            if undo {
                s.compensations.len()
            } else {
                s.commands.len()
            }
        }) + 1;
        let key = if undo {
            undo_key(&plan.id, step, attempt)
        } else {
            step_key(&plan.id, step, attempt)
        };
        let receipt = match self.submit(caller, &with_key(request, &key)).await {
            Ok(r) => r,
            Err(e) => return step_error(e).map(Err),
        };
        self.inner
            .journal
            .link_plan_command(&receipt.command_id, &plan.id, step)?;
        if let Some(s) = plan.step_mut(step) {
            if undo {
                s.compensations.push(receipt.command_id.clone());
            } else {
                s.commands.push(receipt.command_id.clone());
            }
        }
        Ok(Ok(receipt))
    }

    async fn next_step(&self, caller: &Caller, plan: &mut Plan) -> Result<(), EngineError> {
        let next = plan
            .order
            .iter()
            .find(|s| {
                plan.step(s)
                    .is_some_and(|r| r.status == StepStatus::Pending)
            })
            .cloned();
        let Some(step_id) = next else {
            return self.finish(plan, PlanState::Succeeded, None);
        };
        plan.current = Some(step_id.clone());
        if let Err(e) = self.keep_lease(caller, plan) {
            let why = format!("could not keep the plan's lease: {}", e.message);
            return self.wait_for_decision(plan, why);
        }
        let Some(step) = plan.spec_step(&step_id).cloned() else {
            return self.finish(
                plan,
                PlanState::Halted,
                Some(format!("step '{step_id}' is missing")),
            );
        };
        if let Err(e) = self.check_preconditions(caller, &step).await {
            return self.fail(plan, &step_id, e);
        }
        if let Some(s) = plan.step_mut(&step_id) {
            s.status = StepStatus::Running;
        }
        self.save_plan(
            plan,
            "plan.step.started",
            serde_json::json!({ "step": step_id }),
        )?;
        match self
            .submit_for(caller, plan, &step_id, &step.request, false)
            .await?
        {
            Ok(receipt) => self.apply(plan, &step_id, &receipt),
            Err(e) => self.fail(plan, &step_id, e),
        }
    }

    async fn next_compensation(&self, caller: &Caller, plan: &mut Plan) -> Result<(), EngineError> {
        // Newest completed step first. Steps without a compensation are left as they are.
        let next = plan
            .order
            .iter()
            .rev()
            .find(|s| {
                plan.step(s)
                    .is_some_and(|r| r.status == StepStatus::Succeeded)
                    && plan.spec_step(s).is_some_and(|p| p.compensate.is_some())
            })
            .cloned();
        let Some(step_id) = next else {
            return self.finish(plan, PlanState::Compensated, plan.reason.clone());
        };
        plan.current = Some(step_id.clone());
        if let Err(e) = self.keep_lease(caller, plan) {
            let why = format!("could not keep the plan's lease: {}", e.message);
            return self.wait_for_decision(plan, why);
        }
        let Some(request) = plan.spec_step(&step_id).and_then(|s| s.compensate.clone()) else {
            return Ok(());
        };
        if let Some(s) = plan.step_mut(&step_id) {
            s.status = StepStatus::Compensating;
        }
        self.save_plan(
            plan,
            "plan.step.compensating",
            serde_json::json!({ "step": step_id }),
        )?;
        match self
            .submit_for(caller, plan, &step_id, &request, true)
            .await?
        {
            Ok(receipt) => self.apply(plan, &step_id, &receipt),
            Err(e) => self.fail(plan, &step_id, e),
        }
    }

    /// Take a step's (or compensation's) settled command into the plan.
    fn apply(&self, plan: &mut Plan, step: &str, receipt: &Receipt) -> Result<(), EngineError> {
        let undo = plan.compensating;
        match settle(receipt) {
            Settled::Done => {
                if let Some(s) = plan.step_mut(step) {
                    s.status = if undo {
                        StepStatus::Compensated
                    } else {
                        StepStatus::Succeeded
                    };
                    s.error = None;
                }
                let kind = if undo {
                    "plan.step.compensated"
                } else {
                    "plan.step.succeeded"
                };
                self.save_plan(
                    plan,
                    kind,
                    serde_json::json!({ "step": step, "command_id": receipt.command_id }),
                )
            }
            Settled::Held => {
                if !undo && let Some(s) = plan.step_mut(step) {
                    s.status = StepStatus::WaitingApproval;
                }
                plan.state = PlanState::WaitingApproval;
                plan.reason = Some(format!(
                    "step '{step}' (command {}) is waiting for approval",
                    receipt.command_id
                ));
                self.save_plan(
                    plan,
                    "plan.waiting_approval",
                    serde_json::json!({ "step": step, "command_id": receipt.command_id }),
                )
            }
            Settled::Uncertain(e) => {
                if let Some(s) = plan.step_mut(step) {
                    s.status = if undo {
                        StepStatus::CompensationUncertain
                    } else {
                        StepStatus::Uncertain
                    };
                    s.error = Some(e.clone());
                }
                // Never guessed: no retry, no compensation, whatever on_failure says.
                self.wait_for_decision(plan, format!("step '{step}': {}", e.message))
            }
            Settled::Failed(e) => self.fail(plan, step, e),
        }
    }

    /// A step (or compensation) ended definitely without success.
    fn fail(&self, plan: &mut Plan, step: &str, e: ErrorObject) -> Result<(), EngineError> {
        let undo = plan.compensating;
        if let Some(s) = plan.step_mut(step) {
            s.status = if undo {
                StepStatus::CompensationFailed
            } else {
                StepStatus::Failed
            };
            s.error = Some(e.clone());
        }
        let why = format!("step '{step}' failed: {} ({})", e.message, e.code);
        if undo {
            return self.wait_for_decision(plan, format!("compensation of {why}"));
        }
        self.save_plan(
            plan,
            "plan.step.failed",
            serde_json::json!({ "step": step, "error": e }),
        )?;
        match plan
            .spec_step(step)
            .map_or(OnFailure::Halt, |s| s.on_failure)
        {
            OnFailure::Halt => self.finish(plan, PlanState::Halted, Some(why)),
            OnFailure::Compensate => {
                plan.compensating = true;
                plan.state = PlanState::Compensating;
                plan.reason = Some(why);
                self.save_plan(
                    plan,
                    "plan.compensating",
                    serde_json::json!({ "step": step }),
                )
            }
            OnFailure::Ask => self.wait_for_decision(plan, why),
        }
    }

    /// Called when a command leaves `AWAITING_APPROVAL` (approved, denied, cancelled or
    /// expired): the plan it belongs to, if any, carries on.
    pub(super) async fn plan_command_settled(&self, command_id: &str) -> Result<(), EngineError> {
        if let Some((plan_id, _)) = self.inner.journal.plan_of_command(command_id)? {
            self.drive(&plan_id, Some(command_id)).await?;
        }
        Ok(())
    }

    /// The same, from code that cannot wait: the plan carries on in a task of its own.
    pub(super) fn plan_command_settled_later(&self, command_id: &str) {
        let Ok(Some(_)) = self.inner.journal.plan_of_command(command_id) else {
            return;
        };
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let engine = self.clone();
        let command_id = command_id.to_owned();
        rt.spawn(async move {
            if let Err(e) = engine.plan_command_settled(&command_id).await {
                tracing::warn!(command = %command_id, error = %e, "could not continue plan");
            }
        });
    }

    /// An approver's decision on a plan that is waiting for one (ADR-0018).
    pub async fn decide_plan(
        &self,
        caller: &Caller,
        id: &str,
        decision: PlanDecision,
        reason: Option<String>,
    ) -> Result<Plan, EngineError> {
        if caller.kind() == PrincipalKind::Agent || !self.policy().is_approver(caller.principal()) {
            return Err(refused(
                "plan.decision.not_approver",
                "only a human approver decides how a plan continues",
            ));
        }
        let mut plan = {
            let gate = self.plan_lock(id);
            let _guard = gate.lock().await;
            let mut plan = self.plan(caller, id)?;
            let abortable = matches!(
                plan.state,
                PlanState::AwaitingDecision | PlanState::WaitingApproval
            );
            if plan.state != PlanState::AwaitingDecision
                && !(decision == PlanDecision::Abort && abortable)
            {
                return Err(refused(
                    "plan.decision.not_awaiting",
                    format!(
                        "plan {id} is {}, not waiting for a decision",
                        plan.state.name()
                    ),
                ));
            }
            self.take_decision(caller, &mut plan, decision)?;
            plan.decisions.push(DecisionRecord {
                decision,
                by: caller.principal().to_owned(),
                at: self.now(),
                reason: reason.clone(),
            });
            // Every decision is audited as one; an abort then ends the plan.
            let detail = serde_json::json!({ "decision": decision, "by": caller.principal(), "reason": reason });
            if decision == PlanDecision::Abort {
                self.save_plan(&mut plan, "plan.decision", detail)?;
                self.cancel_held(&plan, caller.principal())?;
                self.finish(
                    &mut plan,
                    PlanState::Aborted,
                    Some(reason.unwrap_or_else(|| format!("aborted by {}", caller.principal()))),
                )?;
            } else {
                plan.reason = None;
                self.save_plan(&mut plan, "plan.decision", detail)?;
            }
            plan
        };
        if !plan.state.is_terminal() {
            plan = self.drive(id, None).await?;
        }
        Ok(plan)
    }

    /// Check a decision against the step in question and move the plan accordingly.
    fn take_decision(
        &self,
        _caller: &Caller,
        plan: &mut Plan,
        decision: PlanDecision,
    ) -> Result<(), EngineError> {
        let undo = plan.compensating;
        let step = plan.current.clone();
        let record = step.as_deref().and_then(|s| plan.step(s)).cloned();
        let last = record.as_ref().and_then(|r| {
            if undo {
                r.compensations.last()
            } else {
                r.commands.last()
            }
        });
        let outcome = match last {
            Some(c) => self.inner.journal.receipt(c)?.map(|r| r.state.outcome()),
            None => None,
        };
        let uncertain = matches!(outcome, Some(Outcome::Uncertain | Outcome::Pending));
        let succeeded = matches!(outcome, Some(Outcome::Succeeded | Outcome::Unverified));
        let status = record.as_ref().map(|r| r.status);
        let set = |plan: &mut Plan, status: StepStatus| {
            if let Some(s) = step.as_deref().and_then(|s| plan.step_mut(s)) {
                s.status = status;
                s.error = None;
            }
        };
        let not_allowed = |why: &str| refused("plan.decision.not_allowed", why.to_owned());
        let still_uncertain = || {
            refused(
                "plan.decision.uncertain",
                "the step's command may or may not have taken effect; reconcile it first",
            )
        };
        let running = if undo {
            PlanState::Compensating
        } else {
            PlanState::Running
        };
        match decision {
            PlanDecision::Abort => {}
            PlanDecision::Resume => {
                match status {
                    // Interrupted before anything was sent, or a lease that can be retaken.
                    None
                    | Some(StepStatus::Pending | StepStatus::Succeeded | StepStatus::Compensated) =>
                        {}
                    Some(_) if succeeded => {
                        set(
                            plan,
                            if undo {
                                StepStatus::Compensated
                            } else {
                                StepStatus::Succeeded
                            },
                        );
                    }
                    Some(_) if uncertain => return Err(still_uncertain()),
                    Some(_) => {
                        return Err(not_allowed(
                            "the step did not succeed; retry it, compensate, or abort",
                        ));
                    }
                }
                plan.state = running;
            }
            PlanDecision::Retry => {
                if uncertain {
                    return Err(still_uncertain());
                }
                if succeeded {
                    return Err(not_allowed("the step's command succeeded; resume instead"));
                }
                // A failed compensation goes back to "completed" so it is undone again.
                set(
                    plan,
                    if undo {
                        StepStatus::Succeeded
                    } else {
                        StepStatus::Pending
                    },
                );
                plan.state = running;
            }
            PlanDecision::Compensate => {
                if undo {
                    return Err(not_allowed(
                        "the plan is already compensating; retry or abort",
                    ));
                }
                if uncertain {
                    return Err(still_uncertain());
                }
                if succeeded {
                    set(plan, StepStatus::Succeeded);
                }
                plan.compensating = true;
                plan.state = PlanState::Compensating;
            }
        }
        Ok(())
    }

    /// Withdraw a step command the plan is waiting on, if any.
    fn cancel_held(&self, plan: &Plan, by: &str) -> Result<(), EngineError> {
        let Some(step) = plan.current.as_deref().and_then(|s| plan.step(s)) else {
            return Ok(());
        };
        let last = if plan.compensating {
            step.compensations.last()
        } else {
            step.commands.last()
        };
        if let Some(c) = last
            && let Some(r) = self.inner.journal.receipt(c)?
            && r.state == CommandState::AwaitingApproval
        {
            let r = self.inner.journal.transition(
                c,
                CommandState::Cancelled,
                self.now(),
                Some(format!("plan {} ended by {by}", plan.id)),
                |_| {},
            )?;
            self.publish_command(&r);
        }
        Ok(())
    }

    /// Cancel a plan: its submitter or an approver. A running plan stops at the next step
    /// boundary; the step in progress finishes.
    pub async fn cancel_plan(
        &self,
        caller: &Caller,
        id: &str,
        reason: Option<String>,
    ) -> Result<Plan, EngineError> {
        let plan = self.plan(caller, id)?;
        let approver =
            caller.kind() != PrincipalKind::Agent && self.policy().is_approver(caller.principal());
        if plan.principal != caller.principal() && !approver {
            return Err(refused(
                "plan.cancel.not_allowed",
                "only the plan's submitter or an approver can cancel it",
            ));
        }
        let reason = reason.unwrap_or_else(|| format!("cancelled by {}", caller.principal()));
        self.end_plan(id, caller.principal(), reason).await
    }

    async fn end_plan(&self, id: &str, by: &str, reason: String) -> Result<Plan, EngineError> {
        lock(&self.inner.plans.cancelling).insert(id.to_owned());
        let result = async {
            let gate = self.plan_lock(id);
            let _guard = gate.lock().await;
            let mut plan = self.load_plan(id)?;
            if plan.state.is_terminal() {
                return Ok(plan);
            }
            self.cancel_held(&plan, by)?;
            self.finish(&mut plan, PlanState::Cancelled, Some(reason))?;
            Ok(plan)
        }
        .await;
        lock(&self.inner.plans.cancelling).remove(id);
        result
    }

    /// Cancel the open plans submitted with a credential, when it is revoked or rotated.
    pub async fn cancel_credential_plans(
        &self,
        credential_id: &str,
        by: &str,
    ) -> Result<Vec<String>, EngineError> {
        let mut cancelled = Vec::new();
        for p in self.inner.journal.open_plans()? {
            if p.credential_id.as_deref() == Some(credential_id) {
                self.end_plan(&p.id, by, format!("credential {credential_id} was revoked"))
                    .await?;
                cancelled.push(p.id);
            }
        }
        Ok(cancelled)
    }

    /// After a restart: a plan that was moving comes back waiting for a decision, with its
    /// interrupted step settled from what the journal knows (ADR-0018).
    pub(super) fn recover_plans(&self) -> Result<Vec<String>, EngineError> {
        let mut recovered = Vec::new();
        for mut plan in self.inner.journal.open_plans()? {
            let held_settled =
                plan.state == PlanState::WaitingApproval && !self.still_held(&plan)?;
            if !held_settled && !matches!(plan.state, PlanState::Running | PlanState::Compensating)
            {
                continue;
            }
            if let Some(step) = plan.current.clone() {
                self.settle_interrupted(&mut plan, &step)?;
            }
            plan.state = PlanState::AwaitingDecision;
            plan.reason = Some("the gateway restarted while the plan was running".into());
            let detail = serde_json::json!({ "step": plan.current });
            self.save_plan(&mut plan, "plan.recovered", detail)?;
            recovered.push(plan.id);
        }
        Ok(recovered)
    }

    /// Whether the command the plan waits on is still waiting for approval.
    fn still_held(&self, plan: &Plan) -> Result<bool, EngineError> {
        let Some(step) = plan.current.as_deref().and_then(|s| plan.step(s)) else {
            return Ok(false);
        };
        let last = if plan.compensating {
            step.compensations.last()
        } else {
            step.commands.last()
        };
        Ok(match last {
            Some(c) => self
                .inner
                .journal
                .receipt(c)?
                .is_some_and(|r| r.state == CommandState::AwaitingApproval),
            None => false,
        })
    }

    /// Bring an interrupted step's status in line with its command, which may have been
    /// created without the plan recording it.
    fn settle_interrupted(&self, plan: &mut Plan, step: &str) -> Result<(), EngineError> {
        let undo = plan.compensating;
        let Some(record) = plan.step(step).cloned() else {
            return Ok(());
        };
        let in_progress = if undo {
            record.status == StepStatus::Compensating
        } else {
            matches!(
                record.status,
                StepStatus::Running | StepStatus::WaitingApproval
            )
        };
        if !in_progress {
            return Ok(());
        }
        let request = plan.spec_step(step).and_then(|s| {
            if undo {
                s.compensate.clone()
            } else {
                Some(s.request.clone())
            }
        });
        let device = request
            .as_ref()
            .and_then(|r| r["target"]["device"].as_str().map(str::to_owned))
            .unwrap_or_default();
        let attempt = if undo {
            record.compensations.len()
        } else {
            record.commands.len()
        } + 1;
        let key = if undo {
            undo_key(&plan.id, step, attempt)
        } else {
            step_key(&plan.id, step, attempt)
        };
        let receipt = match self
            .inner
            .journal
            .command_by_key(&plan.principal, &device, &key)?
        {
            Some(r) => {
                self.inner
                    .journal
                    .link_plan_command(&r.command_id, &plan.id, step)?;
                if let Some(s) = plan.step_mut(step) {
                    if undo {
                        s.compensations.push(r.command_id.clone());
                    } else {
                        s.commands.push(r.command_id.clone());
                    }
                }
                Some(r)
            }
            None => record
                .commands
                .last()
                .filter(|_| !undo)
                .or(record.compensations.last().filter(|_| undo))
                .and_then(|c| self.inner.journal.receipt(c).ok().flatten()),
        };
        let status = match receipt.as_ref().map(settle) {
            // Never submitted: nothing happened.
            None => {
                if undo {
                    StepStatus::Succeeded
                } else {
                    StepStatus::Pending
                }
            }
            Some(Settled::Done) => {
                if undo {
                    StepStatus::Compensated
                } else {
                    StepStatus::Succeeded
                }
            }
            Some(Settled::Held) => {
                if undo {
                    StepStatus::Compensating
                } else {
                    StepStatus::WaitingApproval
                }
            }
            Some(Settled::Failed(_)) => {
                if undo {
                    StepStatus::CompensationFailed
                } else {
                    StepStatus::Failed
                }
            }
            Some(Settled::Uncertain(_)) => {
                if undo {
                    StepStatus::CompensationUncertain
                } else {
                    StepStatus::Uncertain
                }
            }
        };
        if let Some(s) = plan.step_mut(step) {
            s.status = status;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_are_checked_and_keys_are_the_plans() {
        let ok = json!({ "target": { "device": "d", "capability": "c" }, "operation": "invoke", "params": {} });
        assert!(parse_request(&ok, "s", "request").is_ok());
        let read = json!({ "target": { "device": "d", "capability": "c" }, "operation": "read" });
        assert_eq!(
            parse_request(&read, "s", "request").unwrap_err().code,
            "plan.step.invalid_request"
        );
        let keyed = with_key(&ok, "mine");
        assert!(parse_request(&keyed, "s", "request").is_err());
        assert_eq!(with_key(&ok, "k")["idempotency_key"], "k");
    }
}
