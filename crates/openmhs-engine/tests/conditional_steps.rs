//! Conditional plan steps (spec/plans.md `when`): a false condition skips the step and its
//! dependents and sends nothing for them; an undecidable condition fails the step rather than
//! skipping it; a dry run reports the skip and still counts the plan as `ok`.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

mod common;

use common::*;
use openmhs_policy::Caller;
use openmhs_sim::Faults;
use openmhs_types::Value;
use openmhs_types::plan::{PlanSpec, PlanState, StepStatus, StepVerdict};
use serde_json::json;

fn agent() -> Caller {
    Caller::in_process(AGENT)
}

fn write_req(capability: &str, value: serde_json::Value) -> serde_json::Value {
    json!({ "target": { "device": PSU, "capability": capability }, "operation": "write", "value": value })
}

fn tripped(is: bool) -> serde_json::Value {
    json!([{ "device": PSU, "capability": "protection.tripped", "op": "eq", "value": is }])
}

fn plan() -> PlanSpec {
    serde_json::from_value(json!({ "steps": [
        { "id": "set-1", "request": write_req(SETPOINT, volts(1.0)) },
        { "id": "enable-if-tripped", "when": tripped(true),
          "request": write_req(ENABLED, json!(true)) },
        { "id": "after-skipped", "after": ["enable-if-tripped"],
          "request": write_req(SETPOINT, volts(1.5)) },
        { "id": "set-2", "after": ["set-1"], "when": tripped(false),
          "request": write_req(SETPOINT, volts(2.0)) }
    ] }))
    .unwrap()
}

#[tokio::test(start_paused = true)]
async fn a_false_condition_skips_the_step_and_its_dependents_and_sends_nothing() {
    let (engine, psu) = setup(allow_all()).await;
    let p = engine.submit_plan(&agent(), plan()).await.unwrap();
    assert_eq!(p.state, PlanState::Succeeded, "{p:?}");
    let status = |id: &str| p.step(id).unwrap().status;
    assert_eq!(status("set-1"), StepStatus::Succeeded);
    assert_eq!(status("enable-if-tripped"), StepStatus::Skipped);
    assert_eq!(status("after-skipped"), StepStatus::Skipped);
    assert_eq!(status("set-2"), StepStatus::Succeeded);
    assert!(
        p.step("after-skipped")
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("was skipped")
    );
    assert_eq!(
        psu.execute_calls(),
        2,
        "only the two steps that ran reached the device"
    );
    let enabled = engine.read(&agent(), PSU, ENABLED).await.unwrap();
    assert_eq!(
        enabled.value,
        Value::Bool(false),
        "the skipped step changed nothing"
    );
    let skips = engine
        .journal()
        .audit_records()
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == "plan.step.skipped")
        .count();
    assert_eq!(skips, 2);
    assert!(engine.verify_audit().unwrap().is_valid());
}

#[tokio::test(start_paused = true)]
async fn a_condition_that_cannot_be_decided_fails_the_step_instead_of_skipping_it() {
    let (engine, psu) = setup(allow_all()).await;
    psu.set_faults(Faults {
        disconnected: true,
        ..Faults::default()
    });
    let spec: PlanSpec = serde_json::from_value(json!({ "steps": [
        { "id": "maybe", "when": tripped(false), "request": write_req(SETPOINT, volts(1.0)) }
    ] }))
    .unwrap();
    let p = engine.submit_plan(&agent(), spec).await.unwrap();
    assert_eq!(p.step("maybe").unwrap().status, StepStatus::Failed, "{p:?}");
    assert_ne!(p.state, PlanState::Succeeded);
    assert_eq!(psu.execute_calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_dry_run_reports_the_skip_and_the_plan_is_still_ok() {
    let (engine, psu) = setup(allow_all()).await;
    let check = engine.preflight_plan(&agent(), plan()).await.unwrap();
    assert!(check.ok, "{check:?}");
    let verdict = |id: &str| check.steps.iter().find(|s| s.id == id).unwrap().verdict;
    assert_eq!(verdict("enable-if-tripped"), StepVerdict::Skipped);
    assert_eq!(verdict("after-skipped"), StepVerdict::Skipped);
    assert_eq!(verdict("set-2"), StepVerdict::Allowed);
    assert_eq!(
        psu.execute_calls(),
        0,
        "a dry run sends nothing to the device"
    );
}

#[tokio::test(start_paused = true)]
async fn a_condition_on_something_unreadable_is_refused_when_the_plan_is_checked() {
    let (engine, _psu) = setup(allow_all()).await;
    let spec: PlanSpec = serde_json::from_value(json!({ "steps": [
        { "id": "x", "when": [{ "device": PSU, "capability": "no.such.thing", "op": "eq", "value": true }],
          "request": write_req(SETPOINT, volts(1.0)) }
    ] }))
    .unwrap();
    let e = engine.submit_plan(&agent(), spec).await.unwrap_err();
    assert!(
        format!("{e:?}").contains("plan.precondition.unknown"),
        "{e:?}"
    );
}
