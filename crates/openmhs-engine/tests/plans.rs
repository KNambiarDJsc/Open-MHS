//! M9: plans (ADR-0018, spec/plans.md). A workcell plan over the virtual lab: set the
//! supply, enable it, home the stage, move it, switch the supply off. Faults are injected
//! at every step, definite and uncertain, and the plan must do exactly what its failure
//! rule says: halt, compensate, or wait for a person. Never guess.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use common::*;
use openmhs_adapter::{
    AdapterCommand, AdapterError, AdapterErrorKind, ExecuteReport, HardwareAdapter, Health,
    Progress,
};
use openmhs_engine::{Engine, EngineError};
use openmhs_policy::{AuthMethod, Caller, Effect, Grant, Policy, Scope};
use openmhs_types::descriptor::Operation;
use openmhs_types::plan::{OnFailure, Plan, PlanDecision, PlanSpec, PlanState, StepStatus};
use openmhs_types::{CommandState, StateValue, Topology, Value};
use serde_json::{Value as Json, json};

const A: &str = "agent:a";
const B: &str = "agent:b";
const AXIS: &str = "sim.axis-01";
const STEPS: [&str; 5] = ["set-voltage", "output-on", "home", "move", "output-off"];

// ------------------------------------------------------------------ fault injection

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// The device refuses: a definite failure, nothing applied.
    Reject,
    /// The link drops mid-command: it may or may not have applied.
    Uncertain,
}

/// Fails chosen executes across every device (1-based), so a fault can be placed at any
/// step of the plan.
#[derive(Default)]
struct Script {
    executes: AtomicU64,
    fail_at: std::sync::Mutex<Vec<(u64, Fault)>>,
}

impl Script {
    fn fail_at(&self, n: u64, fault: Fault) {
        self.fail_at.lock().unwrap().push((n, fault));
    }

    fn clear(&self) {
        self.fail_at.lock().unwrap().clear();
    }

    fn executes(&self) -> u64 {
        self.executes.load(Ordering::SeqCst)
    }
}

struct Scripted {
    inner: Arc<dyn HardwareAdapter>,
    script: Arc<Script>,
}

#[async_trait]
impl HardwareAdapter for Scripted {
    fn adapter_id(&self) -> &str {
        self.inner.adapter_id()
    }
    async fn describe(&self) -> Result<Json, AdapterError> {
        self.inner.describe().await
    }
    async fn connect(&self) -> Result<(), AdapterError> {
        self.inner.connect().await
    }
    async fn health(&self) -> Health {
        self.inner.health().await
    }
    async fn read(&self, capability: &str) -> Result<StateValue, AdapterError> {
        self.inner.read(capability).await
    }
    async fn execute(
        &self,
        command: AdapterCommand,
        progress: Progress,
    ) -> Result<ExecuteReport, AdapterError> {
        let n = self.script.executes.fetch_add(1, Ordering::SeqCst) + 1;
        let fault = self
            .script
            .fail_at
            .lock()
            .unwrap()
            .iter()
            .find(|(at, _)| *at == n)
            .map(|(_, f)| *f);
        match fault {
            Some(Fault::Reject) => Ok(ExecuteReport::Rejected {
                reason: "scripted refusal".into(),
            }),
            Some(Fault::Uncertain) => Err(AdapterError::new(
                AdapterErrorKind::Disconnected,
                "scripted link loss mid-command",
            )),
            None => self.inner.execute(command, progress).await,
        }
    }
    async fn shutdown(&self) -> Result<(), AdapterError> {
        self.inner.shutdown().await
    }
}

// ------------------------------------------------------------------ the workcell

fn everything(principal: &str) -> Grant {
    Grant {
        id: format!("{principal}.all"),
        principal: principal.into(),
        device: "*".into(),
        capability: "*".into(),
        operations: vec![Operation::Read, Operation::Write, Operation::Invoke],
        effect: Effect::Allow,
        max_side_effect: None,
        reason: None,
    }
}

fn policy() -> Policy {
    Policy {
        grants: vec![everything(A), everything(B)],
        approvers: vec![OPERATOR.into()],
    }
}

async fn register_lab(engine: &Engine, script: &Arc<Script>) {
    for sim in openmhs_sim::virtual_lab(clock(), 7) {
        engine
            .register(Arc::new(Scripted {
                inner: sim.adapter,
                script: script.clone(),
            }))
            .await
            .unwrap();
    }
    let topology = Topology::parse(include_str!(
        "../../../spec/examples/topology/virtual-workcell.json"
    ))
    .unwrap();
    engine.set_topology(topology).unwrap();
}

async fn lab(policy: Policy) -> (Engine, Arc<Script>) {
    let engine = Engine::in_memory(clock(), policy).unwrap();
    let script = Arc::new(Script::default());
    register_lab(&engine, &script).await;
    (engine, script)
}

fn psu_write(capability: &str, value: Json) -> Json {
    json!({ "target": { "device": PSU, "capability": capability }, "operation": "write", "value": value })
}

fn axis(capability: &str, params: Json) -> Json {
    json!({ "target": { "device": AXIS, "capability": capability }, "operation": "invoke", "params": params })
}

fn move_to(mm: f64) -> Json {
    axis(
        "axis.move_absolute",
        json!({ "position": { "value": mm, "unit": "mm" }, "speed": { "value": 50, "unit": "mm/s" } }),
    )
}

/// The workcell plan. Every step follows `on_failure`; all but homing and the final
/// switch-off can be undone.
fn workcell(on_failure: OnFailure) -> PlanSpec {
    serde_json::from_value(json!({
        "description": "power the stage and move it to the work position",
        "steps": [
            {
                "id": "set-voltage",
                "request": psu_write(SETPOINT, volts(0.8)),
                "on_failure": on_failure,
                "compensate": psu_write(SETPOINT, volts(0.0)),
            },
            {
                "id": "output-on",
                "after": ["set-voltage"],
                "request": psu_write(ENABLED, json!(true)),
                "on_failure": on_failure,
                "compensate": psu_write(ENABLED, json!(false)),
            },
            {
                "id": "home",
                "after": ["output-on"],
                "request": axis("axis.home", json!({})),
                "on_failure": on_failure,
            },
            {
                "id": "move",
                "after": ["home"],
                "preconditions": [
                    { "device": AXIS, "capability": "axis.homed", "op": "eq", "value": true }
                ],
                "request": move_to(50.0),
                "on_failure": on_failure,
                "compensate": move_to(0.0),
            },
            {
                "id": "output-off",
                "after": ["move"],
                "request": psu_write(ENABLED, json!(false)),
                "on_failure": on_failure,
            },
        ]
    }))
    .unwrap()
}

fn statuses(p: &Plan) -> Vec<StepStatus> {
    STEPS.iter().map(|s| p.step(s).unwrap().status).collect()
}

fn denied_code(e: EngineError) -> String {
    match e {
        EngineError::Denied(e) => e.code,
        EngineError::InvalidRequest(v) => v[0].code.clone(),
        other => format!("{other:?}"),
    }
}

fn invalid_codes(e: EngineError) -> Vec<String> {
    match e {
        EngineError::InvalidRequest(v) => v.into_iter().map(|e| e.code).collect(),
        other => panic!("expected an invalid plan, got {other:?}"),
    }
}

async fn read(engine: &Engine, device: &str, capability: &str) -> Value {
    engine
        .read(&Caller::in_process(A), device, capability)
        .await
        .unwrap()
        .value
}

fn number(v: &Value) -> f64 {
    match v {
        Value::Quantity(q) => q.value,
        other => panic!("not a quantity: {other:?}"),
    }
}

fn no_live_leases(engine: &Engine) -> bool {
    engine.leases(true, 10).unwrap().is_empty()
}

fn audit_kinds(engine: &Engine) -> Vec<String> {
    engine
        .journal()
        .audit_records()
        .unwrap()
        .into_iter()
        .map(|r| r.kind)
        .collect()
}

/// Let a plan carried on in a task of its own (after a denial or cancellation) settle.
async fn settled(engine: &Engine, id: &str) -> Plan {
    let operator = Caller::in_process(OPERATOR);
    for _ in 0..200 {
        let p = engine.plan(&operator, id).unwrap();
        if p.state != PlanState::WaitingApproval {
            return p;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("plan {id} never left WAITING_APPROVAL");
}

// ------------------------------------------------------------------ the happy path

#[tokio::test(start_paused = true)]
async fn a_plan_runs_its_steps_in_order_as_ordinary_commands() {
    let (engine, script) = lab(policy()).await;
    let a = Caller::in_process(A);
    let plan = engine
        .submit_plan(&a, workcell(OnFailure::Halt))
        .await
        .unwrap();

    assert_eq!(plan.state, PlanState::Succeeded, "{plan:#?}");
    assert_eq!(plan.order, STEPS);
    assert!(statuses(&plan).iter().all(|s| *s == StepStatus::Succeeded));
    assert_eq!(script.executes(), 5);
    // Each step is a command with its own receipt, submitted as the plan's caller with a
    // key the plan derived.
    for step in STEPS {
        let id = plan.step(step).unwrap().last_command().unwrap();
        let r = engine.receipt(id).unwrap().unwrap();
        assert_eq!(r.principal, A);
        assert_eq!(r.state, CommandState::Verified, "{step}: {r:?}");
        assert_eq!(
            r.idempotency_key.as_deref(),
            Some(format!("plan:{}:{step}:1", plan.id).as_str())
        );
    }
    assert!((number(&read(&engine, AXIS, "axis.position").await) - 50.0).abs() < 0.1);
    assert_eq!(read(&engine, PSU, ENABLED).await, Value::Bool(false));
    // The lease is gone with the plan; the record is in the audit chain.
    assert!(no_live_leases(&engine));
    let kinds = audit_kinds(&engine);
    for kind in [
        "plan.submitted",
        "plan.step.started",
        "plan.step.succeeded",
        "plan.succeeded",
    ] {
        assert!(kinds.iter().any(|k| k == kind), "no {kind}");
    }
    assert!(engine.verify_audit().unwrap().first_invalid.is_none());
}

/// While the plan runs, its lease keeps other callers off every device it touches,
/// including the sensor that shares the stage's bench.
#[tokio::test(start_paused = true)]
async fn a_plan_leases_what_it_touches() {
    let (engine, _script) = lab(policy()).await;
    let mut spec = workcell(OnFailure::Ask);
    spec.steps[0].request = psu_write(SETPOINT, volts(99.0)); // out of range: fails at once
    let plan = engine
        .submit_plan(&Caller::in_process(A), spec)
        .await
        .unwrap();
    assert_eq!(plan.state, PlanState::AwaitingDecision);
    let lease = engine.lease(plan.lease_id.as_deref().unwrap()).unwrap();
    assert_eq!(lease.holder, A);
    assert_eq!(lease.plan_id.as_deref(), Some(plan.id.as_str()));
    for d in [PSU, AXIS, "sim.thermo-01"] {
        assert!(lease.claim.devices.contains(d), "{d}");
    }
    let b = Caller::in_process(B);
    let refused = engine
        .submit(&b, &psu_write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(codes(&refused), ["lease.held"]);
    // A second plan over the same devices cannot start.
    let e = engine
        .submit_plan(&b, workcell(OnFailure::Halt))
        .await
        .unwrap_err();
    assert_eq!(denied_code(e), "lease.conflict");
}

// ------------------------------------------------------------------ refused plans

#[tokio::test(start_paused = true)]
async fn malformed_plans_are_refused_whole_and_take_nothing() {
    let (engine, script) = lab(policy()).await;
    let a = Caller::in_process(A);
    let bad = |v: Json| serde_json::from_value::<PlanSpec>(v).unwrap();
    let step = |id: &str, after: &[&str], request: Json| json!({ "id": id, "after": after, "request": request });
    let on = psu_write(ENABLED, json!(true));

    let cycle =
        bad(json!({ "steps": [step("a", &["b"], on.clone()), step("b", &["a"], on.clone())] }));
    assert_eq!(
        invalid_codes(engine.submit_plan(&a, cycle).await.unwrap_err()),
        ["plan.cycle"]
    );

    let dangling = bad(json!({ "steps": [step("a", &["nope"], on.clone())] }));
    assert_eq!(
        invalid_codes(engine.submit_plan(&a, dangling).await.unwrap_err()),
        ["plan.step.unknown_dependency"]
    );

    let read = json!({ "target": { "device": PSU, "capability": ENABLED }, "operation": "read" });
    let mut keyed = on.clone();
    keyed["idempotency_key"] = json!("mine");
    let mut garbage = on.clone();
    garbage["operation"] = json!("teleport");
    let several = bad(json!({ "steps": [
        step("read", &[], read),
        step("keyed", &[], keyed),
        step("garbage", &[], garbage),
        { "id": "watch", "request": on,
          "preconditions": [{ "device": PSU, "capability": "power.nonexistent", "op": "eq", "value": true }] },
    ] }));
    let c = invalid_codes(engine.submit_plan(&a, several).await.unwrap_err());
    assert_eq!(
        c,
        [
            "plan.step.invalid_request",
            "plan.step.invalid_request",
            "plan.step.invalid_request",
            "plan.precondition.unknown"
        ]
    );
    assert_eq!(
        invalid_codes(
            engine
                .submit_plan(&a, bad(json!({ "steps": [] })))
                .await
                .unwrap_err()
        ),
        ["plan.empty"]
    );
    assert_eq!(script.executes(), 0);
    assert!(
        engine.leases(false, 10).unwrap().is_empty(),
        "no lease was taken"
    );
    assert!(
        engine
            .plans(&Caller::in_process(OPERATOR), 10)
            .unwrap()
            .is_empty()
    );
}

// ------------------------------------------------------------------ faults at every step

/// `halt`: a refusal at step k ends the plan there. Earlier steps stay done, later ones
/// never run, the devices are released.
#[tokio::test(start_paused = true)]
async fn halt_at_every_step() {
    for k in 1..=STEPS.len() {
        let (engine, script) = lab(policy()).await;
        script.fail_at(k as u64, Fault::Reject);
        let plan = engine
            .submit_plan(&Caller::in_process(A), workcell(OnFailure::Halt))
            .await
            .unwrap();
        assert_eq!(plan.state, PlanState::Halted, "k={k}: {plan:#?}");
        let s = statuses(&plan);
        for (i, status) in s.iter().enumerate() {
            let expected = match (i + 1).cmp(&k) {
                std::cmp::Ordering::Less => StepStatus::Succeeded,
                std::cmp::Ordering::Equal => StepStatus::Failed,
                std::cmp::Ordering::Greater => StepStatus::Pending,
            };
            assert_eq!(*status, expected, "k={k} step {}", STEPS[i]);
        }
        assert_eq!(
            script.executes(),
            k as u64,
            "k={k}: nothing ran after the failure"
        );
        assert_eq!(plan.current.as_deref(), Some(STEPS[k - 1]));
        assert!(plan.reason.as_deref().unwrap().contains(STEPS[k - 1]));
        assert!(no_live_leases(&engine), "k={k}");
    }
}

/// `compensate`: a refusal at step k undoes the completed steps that can be undone,
/// newest first, and leaves the devices as they were before the plan.
#[tokio::test(start_paused = true)]
async fn compensate_at_every_step() {
    for k in 1..=STEPS.len() {
        let (engine, script) = lab(policy()).await;
        script.fail_at(k as u64, Fault::Reject);
        let plan = engine
            .submit_plan(&Caller::in_process(A), workcell(OnFailure::Compensate))
            .await
            .unwrap();
        assert_eq!(plan.state, PlanState::Compensated, "k={k}: {plan:#?}");
        let mut undone = Vec::new();
        for (i, step) in STEPS.iter().enumerate() {
            let r = plan.step(step).unwrap();
            let can_undo = plan.spec_step(step).unwrap().compensate.is_some();
            let expected = if i + 1 < k && can_undo {
                StepStatus::Compensated
            } else if i + 1 < k {
                StepStatus::Succeeded
            } else if i + 1 == k {
                StepStatus::Failed
            } else {
                StepStatus::Pending
            };
            assert_eq!(r.status, expected, "k={k} step {step}");
            if let Some(c) = r.compensations.last() {
                undone.push((engine.receipt(c).unwrap().unwrap().created_at, *step));
            }
        }
        // Newest first.
        undone.sort();
        let order: Vec<&str> = undone.iter().map(|(_, s)| *s).collect();
        let mut expected: Vec<&str> = ["set-voltage", "output-on", "move"]
            .into_iter()
            .filter(|s| STEPS.iter().position(|x| x == s).unwrap() + 1 < k)
            .collect();
        expected.reverse();
        assert_eq!(order, expected, "k={k}");
        assert_eq!(
            read(&engine, PSU, ENABLED).await,
            Value::Bool(false),
            "k={k}"
        );
        assert!(
            number(&read(&engine, PSU, SETPOINT).await).abs() < 1e-9,
            "k={k}"
        );
        if k > 4 {
            assert!(number(&read(&engine, AXIS, "axis.position").await).abs() < 0.1);
        }
        assert!(no_live_leases(&engine), "k={k}");
    }
}

/// An outcome that may or may not have happened is never guessed: whatever `on_failure`
/// says, the plan waits for a person, keeps its lease, and runs nothing else.
#[tokio::test(start_paused = true)]
async fn uncertainty_at_every_step_waits_for_a_decision() {
    for on_failure in [OnFailure::Halt, OnFailure::Compensate, OnFailure::Ask] {
        for k in 1..=STEPS.len() {
            let (engine, script) = lab(policy()).await;
            script.fail_at(k as u64, Fault::Uncertain);
            let plan = engine
                .submit_plan(&Caller::in_process(A), workcell(on_failure))
                .await
                .unwrap();
            assert_eq!(plan.state, PlanState::AwaitingDecision, "k={k}: {plan:#?}");
            let step = plan.step(STEPS[k - 1]).unwrap();
            assert_eq!(step.status, StepStatus::Uncertain, "k={k}");
            assert_eq!(step.error.as_ref().unwrap().code, "plan.step.uncertain");
            let r = engine
                .receipt(step.last_command().unwrap())
                .unwrap()
                .unwrap();
            assert_eq!(r.state, CommandState::UnknownOutcome);
            assert_eq!(script.executes(), k as u64, "k={k}: nothing else ran");
            assert!(!no_live_leases(&engine), "k={k}: devices stay leased");
            assert!(
                statuses(&plan)
                    .iter()
                    .all(|s| *s != StepStatus::Compensated)
            );
        }
    }
}

// ------------------------------------------------------------------ decisions

#[tokio::test(start_paused = true)]
async fn ask_waits_and_an_approver_decides() {
    let (engine, script) = lab(policy()).await;
    script.fail_at(3, Fault::Reject);
    let a = Caller::in_process(A);
    let plan = engine
        .submit_plan(&a, workcell(OnFailure::Ask))
        .await
        .unwrap();
    assert_eq!(plan.state, PlanState::AwaitingDecision);
    assert_eq!(plan.current.as_deref(), Some("home"));

    // Agents do not decide, not even the plan's own.
    let e = engine
        .decide_plan(&a, &plan.id, PlanDecision::Retry, None)
        .await
        .unwrap_err();
    assert_eq!(denied_code(e), "plan.decision.not_approver");
    let operator = Caller::in_process(OPERATOR);
    // Resuming past a failed step is refused: it did not happen.
    let e = engine
        .decide_plan(&operator, &plan.id, PlanDecision::Resume, None)
        .await
        .unwrap_err();
    assert_eq!(denied_code(e), "plan.decision.not_allowed");

    script.clear();
    let done = engine
        .decide_plan(
            &operator,
            &plan.id,
            PlanDecision::Retry,
            Some("cable reseated".into()),
        )
        .await
        .unwrap();
    assert_eq!(done.state, PlanState::Succeeded, "{done:#?}");
    assert_eq!(
        done.step("home").unwrap().commands.len(),
        2,
        "a second attempt, new key"
    );
    assert_eq!(done.decisions.len(), 1);
    assert_eq!(done.decisions[0].by, OPERATOR);
    // Deciding again is refused: the plan is not waiting any more.
    let e = engine
        .decide_plan(&operator, &plan.id, PlanDecision::Abort, None)
        .await
        .unwrap_err();
    assert_eq!(denied_code(e), "plan.decision.not_awaiting");
}

/// The uncertain step must be reconciled before anyone retries or compensates it; once
/// the device shows it applied, the plan resumes from there.
#[tokio::test(start_paused = true)]
async fn uncertainty_is_reconciled_before_the_plan_moves() {
    let (engine, script) = lab(policy()).await;
    // Step 2 (output on) loses its link. The real device did not apply it (the scripted
    // fault stops short of the device), so reconciliation finds the output still off.
    script.fail_at(2, Fault::Uncertain);
    let plan = engine
        .submit_plan(&Caller::in_process(A), workcell(OnFailure::Compensate))
        .await
        .unwrap();
    let operator = Caller::in_process(OPERATOR);
    for d in [
        PlanDecision::Retry,
        PlanDecision::Compensate,
        PlanDecision::Resume,
    ] {
        let e = engine
            .decide_plan(&operator, &plan.id, d, None)
            .await
            .unwrap_err();
        assert_eq!(denied_code(e), "plan.decision.uncertain", "{d:?}");
    }
    let command = plan
        .step("output-on")
        .unwrap()
        .last_command()
        .unwrap()
        .to_owned();
    let reconciled = engine.reconcile(&command).await.unwrap();
    assert_eq!(reconciled.state, CommandState::ReconciledNotApplied);
    // Not applied: resuming would skip a step that never happened.
    let e = engine
        .decide_plan(&operator, &plan.id, PlanDecision::Resume, None)
        .await
        .unwrap_err();
    assert_eq!(denied_code(e), "plan.decision.not_allowed");
    script.clear();
    let done = engine
        .decide_plan(&operator, &plan.id, PlanDecision::Retry, None)
        .await
        .unwrap();
    assert_eq!(done.state, PlanState::Succeeded, "{done:#?}");
}

#[tokio::test(start_paused = true)]
async fn an_approver_can_choose_to_compensate_or_abort() {
    let (engine, script) = lab(policy()).await;
    script.fail_at(4, Fault::Reject);
    let plan = engine
        .submit_plan(&Caller::in_process(A), workcell(OnFailure::Ask))
        .await
        .unwrap();
    let operator = Caller::in_process(OPERATOR);
    let undone = engine
        .decide_plan(&operator, &plan.id, PlanDecision::Compensate, None)
        .await
        .unwrap();
    assert_eq!(undone.state, PlanState::Compensated, "{undone:#?}");
    assert_eq!(read(&engine, PSU, ENABLED).await, Value::Bool(false));

    let (engine, script) = lab(policy()).await;
    script.fail_at(2, Fault::Uncertain);
    let plan = engine
        .submit_plan(&Caller::in_process(A), workcell(OnFailure::Ask))
        .await
        .unwrap();
    // Abort is always possible, even with the outcome unknown; it touches nothing.
    let aborted = engine
        .decide_plan(
            &operator,
            &plan.id,
            PlanDecision::Abort,
            Some("will check by hand".into()),
        )
        .await
        .unwrap();
    assert_eq!(aborted.state, PlanState::Aborted);
    assert_eq!(
        aborted.step("output-on").unwrap().status,
        StepStatus::Uncertain
    );
    assert_eq!(script.executes(), 2);
    assert!(no_live_leases(&engine));
}

/// A compensation that fails is not retried by itself either.
#[tokio::test(start_paused = true)]
async fn a_failed_compensation_waits_for_a_decision() {
    let (engine, script) = lab(policy()).await;
    // Step 3 fails; the first compensation (output off, the 4th execute) fails too.
    script.fail_at(3, Fault::Reject);
    script.fail_at(4, Fault::Reject);
    let plan = engine
        .submit_plan(&Caller::in_process(A), workcell(OnFailure::Compensate))
        .await
        .unwrap();
    assert_eq!(plan.state, PlanState::AwaitingDecision, "{plan:#?}");
    assert_eq!(
        plan.step("output-on").unwrap().status,
        StepStatus::CompensationFailed
    );
    assert_eq!(
        plan.step("set-voltage").unwrap().status,
        StepStatus::Succeeded,
        "not reached"
    );

    script.clear();
    let operator = Caller::in_process(OPERATOR);
    let done = engine
        .decide_plan(&operator, &plan.id, PlanDecision::Retry, None)
        .await
        .unwrap();
    assert_eq!(done.state, PlanState::Compensated, "{done:#?}");
    assert_eq!(done.step("output-on").unwrap().compensations.len(), 2);
}

// ------------------------------------------------------------------ the single-command path

/// A plan step gets exactly the checks a lone command gets: policy, scopes, units,
/// ranges, slew. There is no way around them by putting a command in a plan.
#[tokio::test(start_paused = true)]
async fn no_step_bypasses_the_single_command_policy_path() {
    let mut p = policy();
    p.grants.insert(
        0,
        Grant {
            id: "a.no-output".into(),
            principal: A.into(),
            device: PSU.into(),
            capability: ENABLED.into(),
            operations: vec![Operation::Write],
            effect: Effect::Deny,
            max_side_effect: None,
            reason: Some("output switching is for operators".into()),
        },
    );
    let (engine, script) = lab(p).await;
    let plan = engine
        .submit_plan(&Caller::in_process(A), workcell(OnFailure::Halt))
        .await
        .unwrap();
    assert_eq!(plan.state, PlanState::Halted);
    let step = plan.step("output-on").unwrap();
    assert!(
        step.error.as_ref().unwrap().code.starts_with("policy."),
        "{step:?}"
    );
    let r = engine
        .receipt(step.last_command().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(r.state, CommandState::Denied);
    assert_eq!(script.executes(), 1, "only the first step reached a device");

    // A credential's scopes bound plan steps as they bound commands.
    let (engine, script) = lab(policy()).await;
    let narrow = Caller::authenticated(
        A,
        "tok_narrow",
        AuthMethod::Credential,
        vec![Scope {
            device: PSU.into(),
            capability: "power.output.voltage.*".into(),
            operations: vec![Operation::Read, Operation::Write],
        }],
    );
    let mut spec = workcell(OnFailure::Halt);
    spec.steps.truncate(2);
    let plan = engine.submit_plan(&narrow, spec).await.unwrap();
    assert_eq!(plan.state, PlanState::Halted);
    assert_eq!(
        plan.step("output-on").unwrap().error.as_ref().unwrap().code,
        "auth.scope.denied"
    );
    assert_eq!(script.executes(), 1);

    // Validation too: an over-fast voltage change is refused by the slew limit.
    let (engine, _script) = lab(policy()).await;
    let mut spec = workcell(OnFailure::Halt);
    spec.steps[0].request = psu_write(SETPOINT, volts(9.0));
    let plan = engine
        .submit_plan(&Caller::in_process(A), spec)
        .await
        .unwrap();
    assert_eq!(plan.state, PlanState::Halted);
    let code = &plan
        .step("set-voltage")
        .unwrap()
        .error
        .as_ref()
        .unwrap()
        .code;
    assert!(code.starts_with("command."), "{code}");
}

#[tokio::test(start_paused = true)]
async fn a_step_precondition_that_does_not_hold_stops_the_step_before_it_is_sent() {
    let (engine, script) = lab(policy()).await;
    let mut spec = workcell(OnFailure::Halt);
    // Drop homing: the move's precondition (axis homed) cannot hold.
    spec.steps.remove(2);
    spec.steps[2].after = vec!["output-on".into()];
    spec.steps[3].after = vec!["move".into()];
    let plan = engine
        .submit_plan(&Caller::in_process(A), spec)
        .await
        .unwrap();
    assert_eq!(plan.state, PlanState::Halted);
    let mv = plan.step("move").unwrap();
    assert_eq!(mv.error.as_ref().unwrap().code, "plan.precondition.failed");
    assert!(mv.commands.is_empty(), "never submitted");
    assert_eq!(script.executes(), 2);
}

// ------------------------------------------------------------------ approvals

#[tokio::test(start_paused = true)]
async fn a_step_held_for_approval_pauses_the_plan_until_an_approver_acts() {
    let mut p = policy();
    p.grants.insert(
        0,
        Grant {
            id: "a.moves.ask".into(),
            principal: A.into(),
            device: AXIS.into(),
            capability: "axis.move_absolute".into(),
            operations: vec![Operation::Invoke],
            effect: Effect::RequireApproval,
            max_side_effect: None,
            reason: None,
        },
    );
    let (engine, _script) = lab(p.clone()).await;
    let a = Caller::in_process(A);
    let plan = engine
        .submit_plan(&a, workcell(OnFailure::Halt))
        .await
        .unwrap();
    assert_eq!(plan.state, PlanState::WaitingApproval, "{plan:#?}");
    assert_eq!(
        plan.step("move").unwrap().status,
        StepStatus::WaitingApproval
    );
    let held = plan
        .step("move")
        .unwrap()
        .last_command()
        .unwrap()
        .to_owned();
    // Approving the command runs it, and the plan carries on before the call returns.
    let operator = Caller::in_process(OPERATOR);
    let r = engine.approve(&operator, &held).await.unwrap();
    assert_eq!(r.state, CommandState::Verified);
    let plan = engine.plan(&a, &plan.id).unwrap();
    assert_eq!(plan.state, PlanState::Succeeded, "{plan:#?}");

    // Denied: the step failed, and the plan follows its rule.
    let (engine, _script) = lab(p).await;
    let plan = engine
        .submit_plan(&a, workcell(OnFailure::Halt))
        .await
        .unwrap();
    let held = plan
        .step("move")
        .unwrap()
        .last_command()
        .unwrap()
        .to_owned();
    let approval = engine
        .journal()
        .approval_for_command(&held)
        .unwrap()
        .unwrap();
    engine
        .deny_approval(&operator, &approval.id, Some("not today".into()))
        .unwrap();
    let plan = settled(&engine, &plan.id).await;
    assert_eq!(plan.state, PlanState::Halted, "{plan:#?}");
    assert_eq!(
        plan.step("move").unwrap().error.as_ref().unwrap().code,
        "approval.denied"
    );
    assert!(no_live_leases(&engine));
}

// ------------------------------------------------------------------ cancellation

#[tokio::test(start_paused = true)]
async fn plans_are_private_to_their_agent_and_cancellable_by_it() {
    let (engine, script) = lab(policy()).await;
    script.fail_at(2, Fault::Reject);
    let a = Caller::in_process(A);
    let plan = engine
        .submit_plan(&a, workcell(OnFailure::Ask))
        .await
        .unwrap();
    let b = Caller::in_process(B);
    assert_eq!(
        denied_code(engine.plan(&b, &plan.id).unwrap_err()),
        "plan.unknown"
    );
    assert!(engine.plans(&b, 10).unwrap().is_empty());
    assert_eq!(
        denied_code(engine.cancel_plan(&b, &plan.id, None).await.unwrap_err()),
        "plan.unknown"
    );
    let cancelled = engine.cancel_plan(&a, &plan.id, None).await.unwrap();
    assert_eq!(cancelled.state, PlanState::Cancelled);
    assert!(no_live_leases(&engine));
}

#[tokio::test(start_paused = true)]
async fn revoking_the_credential_cancels_its_plans() {
    let (engine, script) = lab(policy()).await;
    script.fail_at(2, Fault::Reject);
    let caller = Caller::authenticated(A, "tok_a", AuthMethod::Credential, vec![Scope::any()]);
    let plan = engine
        .submit_plan(&caller, workcell(OnFailure::Ask))
        .await
        .unwrap();
    let cancelled = engine
        .cancel_credential_plans("tok_a", OPERATOR)
        .await
        .unwrap();
    assert_eq!(cancelled, std::slice::from_ref(&plan.id));
    let plan = engine
        .plan(&Caller::in_process(OPERATOR), &plan.id)
        .unwrap();
    assert_eq!(plan.state, PlanState::Cancelled);
    assert!(no_live_leases(&engine));
}

// ------------------------------------------------------------------ crash

/// The gateway dies with the stage moving. After restart the move is UNKNOWN_OUTCOME
/// and the plan waits for a decision; nothing is retried by itself.
#[tokio::test(start_paused = true)]
async fn a_crash_mid_plan_comes_back_awaiting_a_decision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.db");
    let plan_id = {
        let engine = Engine::open(&path, clock(), policy()).unwrap();
        let script = Arc::new(Script::default());
        register_lab(&engine, &script).await;
        let task = {
            let engine = engine.clone();
            tokio::spawn(async move {
                engine
                    .submit_plan(&Caller::in_process(A), workcell(OnFailure::Compensate))
                    .await
            })
        };
        // Wait until the fourth command (the move) is on its way to the stage.
        while script.executes() < 4 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        task.abort();
        let _ = task.await;
        let plan = engine
            .plans(&Caller::in_process(OPERATOR), 1)
            .unwrap()
            .remove(0);
        assert_eq!(plan.state, PlanState::Running);
        plan.id
    };

    let engine = Engine::open(&path, clock(), policy()).unwrap();
    let script = Arc::new(Script::default());
    register_lab(&engine, &script).await;
    let report = engine.recover().await.unwrap();
    assert_eq!(
        report.plans_awaiting_decision,
        std::slice::from_ref(&plan_id)
    );
    let operator = Caller::in_process(OPERATOR);
    let plan = engine.plan(&operator, &plan_id).unwrap();
    assert_eq!(plan.state, PlanState::AwaitingDecision);
    assert!(plan.reason.as_deref().unwrap().contains("restarted"));
    let mv = plan.step("move").unwrap();
    assert_eq!(mv.status, StepStatus::Uncertain);
    let command = mv.last_command().unwrap().to_owned();
    assert_eq!(
        engine.receipt(&command).unwrap().unwrap().state,
        CommandState::UnknownOutcome
    );
    assert_eq!(
        script.executes(),
        0,
        "nothing ran by itself after the restart"
    );
    let e = engine
        .decide_plan(&operator, &plan_id, PlanDecision::Retry, None)
        .await
        .unwrap_err();
    assert_eq!(denied_code(e), "plan.decision.uncertain");
    // The devices stay leased to the plan's principal until someone decides.
    let refused = engine
        .submit(&Caller::in_process(B), &psu_write(ENABLED, json!(false)))
        .await
        .unwrap();
    assert_eq!(codes(&refused), ["lease.held"]);
    let aborted = engine
        .decide_plan(&operator, &plan_id, PlanDecision::Abort, None)
        .await
        .unwrap();
    assert_eq!(aborted.state, PlanState::Aborted);
    assert!(no_live_leases(&engine));
}

// ------------------------------------------------------------------ twin preflight

use openmhs_types::plan::StepVerdict;

fn verdicts(p: &openmhs_types::plan::PlanPreflight) -> Vec<StepVerdict> {
    p.steps.iter().map(|s| s.verdict).collect()
}

/// The dry run touches nothing real but reads: no command, no lease, no plan.
#[tokio::test(start_paused = true)]
async fn preflight_dry_runs_the_plan_and_touches_nothing() {
    let (engine, script) = lab(policy()).await;
    let a = Caller::in_process(A);
    let report = engine
        .preflight_plan(&a, workcell(OnFailure::Halt))
        .await
        .unwrap();
    assert!(report.ok, "{report:#?}");
    assert_eq!(report.outcome, PlanState::Succeeded);
    assert!(verdicts(&report).iter().all(|v| *v == StepVerdict::Allowed));
    assert!(report.lease.available);
    assert!(report.lease.claim.devices.contains("sim.thermo-01"));

    assert_eq!(script.executes(), 0, "nothing was sent to a real device");
    assert_eq!(read(&engine, AXIS, "axis.homed").await, Value::Bool(false));
    assert!(engine.commands(None, &[], 10).unwrap().is_empty());
    assert!(engine.leases(false, 10).unwrap().is_empty());
    assert!(
        engine
            .plans(&Caller::in_process(OPERATOR), 10)
            .unwrap()
            .is_empty()
    );
    assert!(audit_kinds(&engine).iter().any(|k| k == "plan.preflight"));
}

/// The twin carries state from step to step, so a step that depends on an earlier one is
/// judged against what the earlier one would have done.
#[tokio::test(start_paused = true)]
async fn preflight_finds_the_step_that_would_fail() {
    let (engine, script) = lab(policy()).await;
    let mut spec = workcell(OnFailure::Halt);
    spec.steps.remove(2); // no homing: the move's precondition cannot hold
    spec.steps[2].after = vec!["output-on".into()];
    spec.steps[3].after = vec!["move".into()];
    let report = engine
        .preflight_plan(&Caller::in_process(A), spec)
        .await
        .unwrap();
    assert!(!report.ok);
    assert_eq!(report.outcome, PlanState::Halted);
    assert_eq!(
        verdicts(&report),
        [
            StepVerdict::Allowed,
            StepVerdict::Allowed,
            StepVerdict::Refused,
            StepVerdict::NotReached
        ]
    );
    assert_eq!(report.steps[2].errors[0].code, "plan.precondition.failed");
    assert_eq!(script.executes(), 0);
}

/// A step policy holds for approval is reported, then assumed approved so the steps after
/// it are checked as well.
#[tokio::test(start_paused = true)]
async fn preflight_looks_past_approvals() {
    let mut p = policy();
    p.grants.insert(
        0,
        Grant {
            id: "a.moves.ask".into(),
            principal: A.into(),
            device: AXIS.into(),
            capability: "axis.move_absolute".into(),
            operations: vec![Operation::Invoke],
            effect: Effect::RequireApproval,
            max_side_effect: None,
            reason: None,
        },
    );
    let (engine, _script) = lab(p).await;
    let mut spec = workcell(OnFailure::Halt);
    spec.steps[4].request = psu_write(SETPOINT, volts(99.0)); // out of range, after the move
    let report = engine
        .preflight_plan(&Caller::in_process(A), spec)
        .await
        .unwrap();
    assert_eq!(
        report.steps[3].verdict,
        StepVerdict::NeedsApproval,
        "{report:#?}"
    );
    assert_eq!(report.steps[4].verdict, StepVerdict::Refused);
    assert!(report.steps[4].errors[0].code.starts_with("command."));
    assert!(!report.ok);
    // No approval request was left behind on the real gateway.
    assert!(
        engine
            .approvals(&Caller::in_process(OPERATOR), &[], 10)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(start_paused = true)]
async fn preflight_reports_a_busy_lease_and_policy_refusals() {
    let (engine, _script) = lab(policy()).await;
    engine
        .acquire_lease(
            &Caller::in_process(B),
            openmhs_engine::LeaseRequest {
                devices: vec![PSU.into()],
                ttl_ms: 60_000,
                ..Default::default()
            },
        )
        .unwrap();
    let report = engine
        .preflight_plan(&Caller::in_process(A), workcell(OnFailure::Halt))
        .await
        .unwrap();
    assert!(!report.lease.available);
    assert_eq!(report.lease.error.as_ref().unwrap().code, "lease.conflict");
    assert!(!report.ok);

    let mut p = policy();
    p.grants.insert(
        0,
        Grant {
            id: "a.no-output".into(),
            principal: A.into(),
            device: PSU.into(),
            capability: ENABLED.into(),
            operations: vec![Operation::Write],
            effect: Effect::Deny,
            max_side_effect: None,
            reason: None,
        },
    );
    let (engine, _script) = lab(p).await;
    let report = engine
        .preflight_plan(&Caller::in_process(A), workcell(OnFailure::Halt))
        .await
        .unwrap();
    assert_eq!(report.steps[1].verdict, StepVerdict::Refused);
    assert!(report.steps[1].errors[0].code.starts_with("policy."));
}

/// What the twin cannot know: a device that will refuse or stall. The contract checks pass,
/// so preflight says yes; only the real run finds out.
#[tokio::test(start_paused = true)]
async fn preflight_has_no_physics() {
    let (engine, script) = lab(policy()).await;
    script.fail_at(1, Fault::Reject);
    let report = engine
        .preflight_plan(&Caller::in_process(A), workcell(OnFailure::Halt))
        .await
        .unwrap();
    assert!(report.ok);
    let plan = engine
        .submit_plan(&Caller::in_process(A), workcell(OnFailure::Halt))
        .await
        .unwrap();
    assert_eq!(plan.state, PlanState::Halted);
}
