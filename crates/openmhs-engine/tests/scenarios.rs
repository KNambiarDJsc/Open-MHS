//! End-to-end scenarios through the engine and the simulated power supply.

mod common;

use common::*;
use openmhs_engine::EngineError;
use openmhs_policy::{Effect, Policy};
use openmhs_sim::ExecFault;
use openmhs_types::descriptor::Operation;
use openmhs_types::{CommandState, Outcome, Quantity, Value};
use serde_json::json;

use CommandState::*;

#[tokio::test(start_paused = true)]
async fn write_is_dispatched_applied_and_verified_with_evidence() {
    let (engine, psu) = setup(allow_all()).await;
    let r = engine
        .submit(
            AGENT,
            &with(
                write(SETPOINT, volts(1.5)),
                "correlation_id",
                json!("run-1"),
            ),
        )
        .await
        .unwrap();

    assert_eq!(
        states(&r),
        [
            Created, Validating, Approved, Dispatched, Accepted, Applied, Verified
        ]
    );
    assert_eq!(r.outcome, Outcome::Succeeded);
    assert_eq!(r.policy.rules[0].rule_id, "grant.agent.psu");
    let v = r.verification.as_ref().unwrap();
    assert_eq!(v.matched, Some(true));
    assert_eq!(v.capability.as_deref(), Some(SETPOINT));
    assert_eq!(r.correlation_id.as_deref(), Some("run-1"));
    assert!((psu.truth().setpoint_v - 1.5).abs() < 1e-12);
    assert_schema_valid(&r);
}

#[tokio::test(start_paused = true)]
async fn values_are_converted_to_declared_units() {
    let (engine, psu) = setup(allow_all()).await;
    let r = engine
        .submit(
            AGENT,
            &write(SETPOINT, json!({ "value": 1500, "unit": "mV" })),
        )
        .await
        .unwrap();
    assert_eq!(r.state, Verified);
    assert!(
        (psu.truth().setpoint_v - 1.5).abs() < 1e-12,
        "device received volts"
    );
}

#[tokio::test(start_paused = true)]
async fn readback_mismatch_is_verification_failed() {
    let (engine, psu) = setup(allow_all()).await;
    psu.update_faults(|f| f.readback_offset = 0.5);
    let r = engine
        .submit(AGENT, &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(r.state, VerificationFailed);
    assert_eq!(r.outcome, Outcome::Failed);
    assert_eq!(r.verification.as_ref().unwrap().matched, Some(false));
    assert_schema_valid(&r);
}

#[tokio::test(start_paused = true)]
async fn unreadable_readback_is_unverifiable_not_success() {
    let (engine, psu) = setup(allow_all()).await;
    // Readings go BAD right after the command applies: fault set by a parallel task once
    // the device has applied the change.
    let p = psu.clone();
    tokio::spawn(async move {
        while p.truth().applied_count == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        p.update_faults(|f| f.bad_quality = true);
    });
    let r = engine
        .submit(AGENT, &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(r.state, Unverifiable);
    assert_eq!(r.outcome, Outcome::Unverified);
}

#[tokio::test(start_paused = true)]
async fn slew_and_rate_limits_are_enforced() {
    let (engine, psu) = setup(allow_all()).await;
    let r = engine
        .submit(AGENT, &write(SETPOINT, volts(3.0)))
        .await
        .unwrap();
    assert_eq!(
        codes(&r),
        ["command.slew_exceeded"],
        "0 -> 3 V exceeds 2 V per second"
    );

    // Rate limit: 5 per second.
    for i in 1..=5 {
        let r = engine
            .submit(AGENT, &write(SETPOINT, volts(0.1 * f64::from(i))))
            .await
            .unwrap();
        assert_eq!(r.state, Verified, "write {i}");
    }
    let r = engine
        .submit(AGENT, &write(SETPOINT, volts(0.6)))
        .await
        .unwrap();
    assert_eq!(codes(&r), ["command.rate_limited"]);
    assert_eq!(psu.execute_calls(), 5);

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let r = engine
        .submit(AGENT, &write(SETPOINT, volts(0.6)))
        .await
        .unwrap();
    assert_eq!(r.state, Verified, "window has passed");
}

#[tokio::test(start_paused = true)]
async fn failed_precondition_denies_with_rule_id() {
    let (engine, psu) = setup(allow_all()).await;
    psu.set_truth(|t| t.tripped = true);
    let r = engine
        .submit(AGENT, &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(codes(&r), ["command.precondition.failed"]);
    assert_eq!(
        r.policy.errors[0].rule_id.as_deref(),
        Some("protection-not-tripped")
    );
    assert_eq!(psu.execute_calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn actions_verify_against_their_declared_property() {
    let (engine, psu) = setup(allow_all()).await;
    psu.set_truth(|t| t.tripped = true);
    let reset = json!({ "target": { "device": PSU, "capability": "protection.reset" }, "operation": "invoke", "params": {} });
    let r = engine.submit(AGENT, &reset).await.unwrap();
    assert_eq!(r.state, Verified);
    assert!(!psu.truth().tripped);
}

#[tokio::test(start_paused = true)]
async fn approval_flow() {
    let mut policy = allow_all();
    policy.grants.insert(
        0,
        grant(
            "ask.enable",
            ENABLED,
            &[Operation::Write],
            Effect::RequireApproval,
        ),
    );
    let (engine, psu) = setup(policy).await;

    let r = engine
        .submit(AGENT, &write(ENABLED, json!(true)))
        .await
        .unwrap();
    assert_eq!(r.state, AwaitingApproval);
    assert_eq!(psu.execute_calls(), 0);

    assert_eq!(
        engine
            .approve(AGENT, &r.command_id)
            .await
            .unwrap_err()
            .code(),
        "policy.approval.not_approver"
    );
    let mut self_approving = engine.policy();
    self_approving.approvers.push(AGENT.into());
    engine.set_policy(self_approving).unwrap();
    assert_eq!(
        engine
            .approve(AGENT, &r.command_id)
            .await
            .unwrap_err()
            .code(),
        "policy.approval.self"
    );

    let done = engine.approve(OPERATOR, &r.command_id).await.unwrap();
    assert_eq!(done.state, Verified);
    assert_eq!(done.policy.approved_by.as_deref(), Some(OPERATOR));
    assert!(psu.truth().enabled);
    assert_schema_valid(&done);
}

#[tokio::test(start_paused = true)]
async fn waiting_commands_can_be_cancelled_and_never_execute() {
    let mut policy = allow_all();
    policy.grants.insert(
        0,
        grant(
            "ask.enable",
            ENABLED,
            &[Operation::Write],
            Effect::RequireApproval,
        ),
    );
    let (engine, psu) = setup(policy).await;
    let r = engine
        .submit(AGENT, &write(ENABLED, json!(true)))
        .await
        .unwrap();
    let c = engine.cancel(AGENT, &r.command_id).unwrap();
    assert_eq!(c.state, Cancelled);
    assert_eq!(c.outcome, Outcome::NotExecuted);
    assert!(matches!(
        engine.approve(OPERATOR, &r.command_id).await,
        Err(EngineError::InvalidState { .. })
    ));
    assert_eq!(psu.execute_calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn device_rejection_is_rejected() {
    let (engine, psu) = setup(allow_all()).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::Reject));
    let r = engine
        .submit(AGENT, &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(r.state, Rejected);
    assert_eq!(r.outcome, Outcome::Failed);
}

#[tokio::test(start_paused = true)]
async fn lost_acknowledgement_reconciles_to_applied() {
    let (engine, psu) = setup(allow_all()).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::ApplyThenHang));
    let r = engine
        .submit(
            AGENT,
            &with(write(SETPOINT, volts(1.0)), "deadline_ms", json!(2_000)),
        )
        .await
        .unwrap();
    assert_eq!(r.state, UnknownOutcome);

    let resolved = engine.reconcile(&r.command_id).await.unwrap();
    assert_eq!(resolved.state, ReconciledApplied);
    assert_eq!(resolved.outcome, Outcome::Succeeded);
    assert_eq!(
        psu.execute_calls(),
        1,
        "reconciliation reads, it does not resend"
    );
    assert_schema_valid(&resolved);
}

#[tokio::test(start_paused = true)]
async fn lost_command_reconciles_to_not_applied() {
    let (engine, psu) = setup(allow_all()).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::Hang));
    let r = engine
        .submit(
            AGENT,
            &with(write(SETPOINT, volts(1.0)), "deadline_ms", json!(2_000)),
        )
        .await
        .unwrap();
    let resolved = engine.reconcile(&r.command_id).await.unwrap();
    assert_eq!(resolved.state, ReconciledNotApplied);
    assert_eq!(
        resolved.verification.unwrap().observed.unwrap().value,
        Value::Quantity(Quantity {
            value: 0.0,
            unit: "V".into()
        })
    );
}

#[tokio::test(start_paused = true)]
async fn garbled_response_and_disconnect_are_uncertain() {
    for fault in [ExecFault::ApplyThenMalformed, ExecFault::Disconnect] {
        let (engine, psu) = setup(allow_all()).await;
        psu.update_faults(|f| f.execute = Some(fault));
        let r = engine
            .submit(AGENT, &write(SETPOINT, volts(1.0)))
            .await
            .unwrap();
        assert_eq!(r.state, UnknownOutcome, "{fault:?}");
        assert!(
            !states(&r).contains(&TimedOut),
            "{fault:?} is not a timeout"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn adapter_panic_is_contained() {
    let (engine, psu) = setup(allow_all()).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::Crash));
    let r = engine
        .submit(AGENT, &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(r.state, UnknownOutcome);
    assert_eq!(
        r.adapter_result.unwrap().error.unwrap().code,
        "adapter.internal"
    );

    psu.clear_faults();
    let r = engine
        .submit(AGENT, &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(
        r.state, Verified,
        "engine keeps working after an adapter panic"
    );
}

#[tokio::test(start_paused = true)]
async fn reads_are_authorized_audited_and_cached() {
    let (engine, _psu) = setup(allow_all()).await;
    let v = engine.read(AGENT, PSU, SETPOINT).await.unwrap();
    assert_eq!(engine.last_known(PSU, SETPOINT), Some(v));
    let audit = engine.journal().audit_records().unwrap();
    assert!(audit.iter().any(|a| a.kind == "read" && a.subject == PSU));

    let err = engine
        .read(AGENT, PSU, "protection.reset")
        .await
        .unwrap_err();
    assert_eq!(err.code(), "command.operation.unsupported");
    let read_cmd =
        json!({ "target": { "device": PSU, "capability": SETPOINT }, "operation": "read" });
    assert!(matches!(
        engine.submit(AGENT, &read_cmd).await,
        Err(EngineError::NotACommand)
    ));
}

#[tokio::test(start_paused = true)]
async fn unknown_device_and_capability_are_denied_with_receipts() {
    let (engine, _) = setup(allow_all()).await;
    let r = engine
        .submit(AGENT, &json!({ "target": { "device": "nope", "capability": SETPOINT }, "operation": "write", "value": volts(1.0) }))
        .await
        .unwrap();
    assert_eq!(codes(&r), ["command.device.unknown"]);
    let r = engine
        .submit(AGENT, &write("power.output.turbo", json!(true)))
        .await
        .unwrap();
    assert_eq!(codes(&r), ["command.capability.unknown"]);
}

#[tokio::test(start_paused = true)]
async fn side_effect_ceiling_on_grants() {
    let mut g = grant(
        "grant.limited",
        "*",
        &[Operation::Write, Operation::Read],
        Effect::Allow,
    );
    g.max_side_effect = Some(openmhs_types::descriptor::SideEffect::None);
    let (engine, _) = setup(Policy {
        grants: vec![g],
        approvers: vec![],
    })
    .await;
    let r = engine
        .submit(AGENT, &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(codes(&r), ["policy.denied.side_effect"]);
}
