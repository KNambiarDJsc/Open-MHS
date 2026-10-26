//! A write whose outcome is unknown is never sent again blindly (spec/lifecycle.md §3
//! rule 3, §5): it is reconciled by readback or resolved by a person first.

mod common;

use common::*;
use openmhs_engine::EngineError;
use openmhs_policy::Caller;
use openmhs_sim::ExecFault;
use openmhs_types::{CommandState, Outcome};
use serde_json::json;

use CommandState::*;

const UNRESOLVED: &str = "command.outcome_unresolved";

fn agent() -> Caller {
    Caller::in_process(AGENT)
}

#[tokio::test(start_paused = true)]
async fn a_write_with_an_unknown_outcome_is_not_sent_again_until_reconciled() {
    let (engine, psu) = setup_not_idempotent(allow_all(), SETPOINT).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::ApplyThenHang));
    let req = with(write(SETPOINT, volts(1.0)), "deadline_ms", json!(2_000));
    let first = engine.submit(&agent(), &req).await.unwrap();
    assert_eq!(first.state, UnknownOutcome);
    psu.clear_faults();

    // The agent retries: refused before anything reaches the device.
    let retry = engine.submit(&agent(), &req).await.unwrap();
    assert_eq!(retry.state, Denied);
    assert_eq!(codes(&retry), [UNRESOLVED]);
    assert!(retry.policy.errors[0].message.contains(&first.command_id));
    assert_schema_valid(&retry);
    let preflight = engine.preflight(&agent(), &req).await.unwrap();
    assert_eq!(preflight.errors[0].code, UNRESOLVED);
    assert_eq!(psu.execute_calls(), 1, "the retry was never sent");

    // Once the first command is settled by readback, a new one may go.
    let settled = engine.reconcile(&first.command_id).await.unwrap();
    assert_eq!(settled.state, ReconciledApplied);
    let next = engine.submit(&agent(), &req).await.unwrap();
    assert_eq!(next.state, Verified);
    assert_eq!(psu.execute_calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn the_same_idempotency_key_returns_the_uncertain_receipt_without_resending() {
    let (engine, psu) = setup_not_idempotent(allow_all(), SETPOINT).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::Hang));
    let req = with(
        with(write(SETPOINT, volts(1.0)), "deadline_ms", json!(2_000)),
        "idempotency_key",
        json!("set-1v"),
    );
    let first = engine.submit(&agent(), &req).await.unwrap();
    let again = engine.submit(&agent(), &req).await.unwrap();
    assert_eq!(again.command_id, first.command_id);
    assert_eq!(again.state, UnknownOutcome);
    assert_eq!(psu.execute_calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_person_resolves_an_outcome_with_a_recorded_reason() {
    let (engine, psu) = setup_not_idempotent(allow_all(), SETPOINT).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::Hang));
    let req = with(write(SETPOINT, volts(1.0)), "deadline_ms", json!(2_000));
    let first = engine.submit(&agent(), &req).await.unwrap();
    assert_eq!(first.state, UnknownOutcome);
    let id = &first.command_id;

    let by_agent = engine.resolve(&Caller::in_process("agent:robot"), id, false, "looks fine");
    assert!(
        matches!(&by_agent, Err(EngineError::Denied(e)) if e.code == "policy.resolve.not_human"),
        "{by_agent:?}"
    );
    let operator = Caller::in_process(OPERATOR);
    let no_reason = engine.resolve(&operator, id, false, "  ");
    assert!(
        matches!(&no_reason, Err(EngineError::InvalidRequest(e)) if e[0].code == "command.resolve.reason"),
        "{no_reason:?}"
    );

    let resolved = engine
        .resolve(&operator, id, false, "front panel shows 0.00 V")
        .unwrap();
    assert_eq!(resolved.state, ReconciledNotApplied);
    assert_eq!(resolved.outcome, Outcome::Failed);
    let detail = resolved.transitions.last().unwrap().detail.clone().unwrap();
    assert_eq!(
        detail,
        "resolved by human.operator: front panel shows 0.00 V"
    );
    assert_schema_valid(&resolved);
    assert!(matches!(
        engine.resolve(&operator, id, true, "again"),
        Err(EngineError::InvalidState { .. })
    ));
    assert_eq!(
        psu.execute_calls(),
        1,
        "resolving never contacts the device"
    );

    psu.clear_faults();
    assert_eq!(engine.submit(&agent(), &req).await.unwrap().state, Verified);
}

#[tokio::test(start_paused = true)]
async fn only_the_uncertain_capability_is_held() {
    let (engine, psu) = setup_not_idempotent(allow_all(), SETPOINT).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::Hang));
    let req = with(write(SETPOINT, volts(1.0)), "deadline_ms", json!(2_000));
    assert_eq!(
        engine.submit(&agent(), &req).await.unwrap().state,
        UnknownOutcome
    );
    psu.clear_faults();
    let other = engine
        .submit(&agent(), &write(ENABLED, json!(true)))
        .await
        .unwrap();
    assert!(!codes(&other).contains(&UNRESOLVED.to_owned()), "{other:?}");
}

#[tokio::test(start_paused = true)]
async fn an_idempotent_capability_may_be_sent_again() {
    // Setting the same level twice has the same effect as once.
    let (engine, psu) = setup(allow_all()).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::Hang));
    let req = with(write(SETPOINT, volts(1.0)), "deadline_ms", json!(2_000));
    assert_eq!(
        engine.submit(&agent(), &req).await.unwrap().state,
        UnknownOutcome
    );
    psu.clear_faults();
    assert_eq!(engine.submit(&agent(), &req).await.unwrap().state, Verified);
}
