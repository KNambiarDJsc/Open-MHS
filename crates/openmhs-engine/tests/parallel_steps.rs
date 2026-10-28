//! Parallel plan steps (ADR-0025): only with `parallel`, only steps on disjoint devices,
//! never a step that needs approval, and every branch settles before the plan reacts, so
//! a failure on one branch compensates the other only after it has stopped.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;

use common::*;
use openmhs_engine::Engine;
use openmhs_policy::{Caller, Effect, Grant, Policy};
use openmhs_sim::{AxisHandle, ExecFault, Faults, PsuHandle, SimControl, VirtualAxis, VirtualPsu};
use openmhs_types::descriptor::Operation;
use openmhs_types::plan::{PlanSpec, PlanState, StepStatus};
use serde_json::json;

const AXIS: &str = "sim.axis-01";

fn agent() -> Caller {
    Caller::in_process(AGENT)
}

fn policy(axis_effect: Effect) -> Policy {
    let g = |id: &str, device: &str, effect: Effect| Grant {
        id: id.into(),
        principal: AGENT.into(),
        device: device.into(),
        capability: "*".into(),
        operations: vec![Operation::Read, Operation::Write, Operation::Invoke],
        effect,
        max_side_effect: None,
        reason: None,
    };
    Policy {
        grants: vec![
            g("psu", "sim.psu-*", Effect::Allow),
            g("axis", AXIS, axis_effect),
        ],
        approvers: vec![OPERATOR.into()],
    }
}

async fn lab(axis_effect: Effect) -> (Engine, PsuHandle, AxisHandle) {
    let clock = clock();
    let (psu, psu_handle) = VirtualPsu::new(clock.clone(), 42);
    let (axis, axis_handle) = VirtualAxis::new(clock.clone());
    let engine = Engine::in_memory(clock, policy(axis_effect)).unwrap();
    engine.register(Arc::new(psu)).await.unwrap();
    engine.register(Arc::new(axis)).await.unwrap();
    (engine, psu_handle, axis_handle)
}

fn slow(ms: u64) -> Faults {
    Faults {
        execute_latency_ms: ms,
        ..Faults::default()
    }
}

fn spec(parallel: bool, second_device: &str) -> PlanSpec {
    let second = if second_device == AXIS {
        json!({ "target": { "device": AXIS, "capability": "axis.home" }, "operation": "invoke" })
    } else {
        json!({ "target": { "device": PSU, "capability": ENABLED }, "operation": "write", "value": true })
    };
    serde_json::from_value(json!({ "parallel": parallel, "steps": [
        { "id": "supply", "request": { "target": { "device": PSU, "capability": SETPOINT },
                                        "operation": "write", "value": volts(1.0) },
          "compensate": { "target": { "device": PSU, "capability": SETPOINT },
                          "operation": "write", "value": volts(0.0) } },
        { "id": "second", "request": second, "on_failure": "compensate" }
    ] }))
    .unwrap()
}

async fn timed(engine: &Engine, spec: PlanSpec) -> (openmhs_types::plan::Plan, u128) {
    let t = tokio::time::Instant::now();
    let p = engine.submit_plan(&agent(), spec).await.unwrap();
    (p, t.elapsed().as_millis())
}

#[tokio::test(start_paused = true)]
async fn independent_steps_run_together_only_when_the_plan_asks() {
    let (engine, psu, axis) = lab(Effect::Allow).await;
    psu.set_faults(slow(2_000));
    axis.set_faults(slow(2_000));
    let (seq, seq_ms) = timed(&engine, spec(false, AXIS)).await;
    assert_eq!(seq.state, PlanState::Succeeded, "{seq:?}");

    let (engine, psu, axis) = lab(Effect::Allow).await;
    psu.set_faults(slow(2_000));
    axis.set_faults(slow(2_000));
    let (par, par_ms) = timed(&engine, spec(true, AXIS)).await;
    assert_eq!(par.state, PlanState::Succeeded, "{par:?}");
    assert!(
        par_ms + 1_500 < seq_ms,
        "two 2 s steps took {par_ms} ms together and {seq_ms} ms one after the other"
    );
}

#[tokio::test(start_paused = true)]
async fn steps_on_the_same_device_never_run_together() {
    let (engine, psu, _axis) = lab(Effect::Allow).await;
    psu.set_faults(slow(2_000));
    let (p, ms) = timed(&engine, spec(true, PSU)).await;
    assert_eq!(p.state, PlanState::Succeeded, "{p:?}");
    assert!(ms >= 4_000, "one device, one step at a time: {ms} ms");
}

#[tokio::test(start_paused = true)]
async fn a_step_that_needs_approval_runs_alone() {
    let (engine, _psu, _axis) = lab(Effect::RequireApproval).await;
    let p = engine
        .submit_plan(&agent(), spec(true, AXIS))
        .await
        .unwrap();
    // The supply ran on its own; the stage's step waits for a person, alone.
    assert_eq!(p.step("supply").unwrap().status, StepStatus::Succeeded);
    assert_eq!(
        p.step("second").unwrap().status,
        StepStatus::WaitingApproval
    );
    assert_eq!(p.state, PlanState::WaitingApproval);
}

#[tokio::test(start_paused = true)]
async fn a_failed_branch_compensates_the_other_only_after_it_has_finished() {
    let (engine, psu, axis) = lab(Effect::Allow).await;
    psu.set_faults(slow(1_000));
    axis.set_faults(Faults {
        execute: Some(ExecFault::Reject),
        ..Faults::default()
    });
    let p = engine
        .submit_plan(&agent(), spec(true, AXIS))
        .await
        .unwrap();
    assert_eq!(
        p.step("second").unwrap().status,
        StepStatus::Failed,
        "{p:?}"
    );
    // The supply's step finished (it was not cut short), then was undone.
    let supply = p.step("supply").unwrap();
    assert_eq!(supply.status, StepStatus::Compensated, "{p:?}");
    assert_eq!(supply.commands.len(), 1);
    assert_eq!(supply.compensations.len(), 1);
    assert_eq!(p.state, PlanState::Compensated);
    let set = engine.receipt(&supply.commands[0]).unwrap().unwrap();
    let undo = engine.receipt(&supply.compensations[0]).unwrap().unwrap();
    assert_eq!(set.state, openmhs_types::CommandState::Verified);
    assert!(
        set.transitions.last().unwrap().at <= undo.created_at,
        "the undo was created only after the supply's own command had settled"
    );
}

#[tokio::test(start_paused = true)]
async fn an_unknown_outcome_on_one_branch_stops_for_a_person_without_compensating() {
    let (engine, _psu, axis) = lab(Effect::Allow).await;
    axis.set_faults(Faults {
        execute: Some(ExecFault::ApplyThenHang),
        ..Faults::default()
    });
    let p = engine
        .submit_plan(&agent(), spec(true, AXIS))
        .await
        .unwrap();
    assert_eq!(p.state, PlanState::AwaitingDecision, "{p:?}");
    assert_eq!(p.step("second").unwrap().status, StepStatus::Uncertain);
    assert_eq!(p.step("supply").unwrap().status, StepStatus::Succeeded);
    assert!(
        p.step("supply").unwrap().compensations.is_empty(),
        "nothing undone by itself"
    );
}
