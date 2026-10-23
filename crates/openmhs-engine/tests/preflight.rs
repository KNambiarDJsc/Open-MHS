//! Preflight: the same checks as a submission, with no command and no execution.

mod common;

use common::*;
use openmhs_policy::{Effect, Policy};
use openmhs_types::descriptor::Operation;
use openmhs_types::receipt::Decision;

#[tokio::test(start_paused = true)]
async fn preflight_reports_denials_without_creating_or_executing_anything() {
    let (engine, psu) = setup(allow_all()).await;
    let p = engine
        .preflight(AGENT, &write(SETPOINT, volts(48.0)))
        .await
        .unwrap();
    assert_eq!(p.decision, Decision::Deny);
    assert_eq!(p.errors[0].code, "command.value.out_of_range");
    assert_eq!(p.rules[0].rule_id, "grant.agent.psu");

    let ok = engine
        .preflight(AGENT, &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(ok.decision, Decision::Allow);
    assert!(ok.errors.is_empty());

    assert!(engine.commands(None, &[], 10).unwrap().is_empty());
    assert_eq!(psu.execute_calls(), 0);
    assert_eq!(psu.truth().setpoint_v, 0.0);
}

#[tokio::test(start_paused = true)]
async fn preflight_agrees_with_submission() {
    let policy = Policy {
        grants: vec![
            grant("read", "*", &[Operation::Read], Effect::Allow),
            grant(
                "approve-setpoint",
                SETPOINT,
                &[Operation::Write],
                Effect::RequireApproval,
            ),
        ],
        approvers: vec![OPERATOR.into()],
    };
    let (engine, psu) = setup(policy).await;
    psu.set_truth(|t| t.tripped = true);
    let cases = [
        (write(SETPOINT, volts(1.0)), Decision::Deny), // precondition: protection tripped
        (write(ENABLED, serde_json::json!(true)), Decision::Deny), // no grant
    ];
    for (req, expected) in cases {
        let p = engine.preflight(AGENT, &req).await.unwrap();
        assert_eq!(p.decision, expected);
        let r = engine.submit(AGENT, &req).await.unwrap();
        assert_eq!(
            codes(&r),
            p.errors.iter().map(|e| e.code.clone()).collect::<Vec<_>>()
        );
    }

    psu.set_truth(|t| t.tripped = false);
    let req = write(SETPOINT, volts(1.0));
    assert_eq!(
        engine.preflight(AGENT, &req).await.unwrap().decision,
        Decision::RequireApproval
    );
    let r = engine.submit(AGENT, &req).await.unwrap();
    assert_eq!(r.state, openmhs_types::CommandState::AwaitingApproval);
}

#[tokio::test(start_paused = true)]
async fn preflight_is_audited_and_rejects_reads() {
    let (engine, _psu) = setup(allow_all()).await;
    engine
        .preflight(AGENT, &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    let records = engine.journal().audit_records().unwrap();
    assert!(records.iter().any(|r| r.kind == "command.preflight"));

    let read = serde_json::json!({
        "target": { "device": PSU, "capability": SETPOINT }, "operation": "read"
    });
    assert!(matches!(
        engine.preflight(AGENT, &read).await,
        Err(openmhs_engine::EngineError::NotACommand)
    ));
}
