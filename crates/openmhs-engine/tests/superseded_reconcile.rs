//! A later write to the same state makes a reading say nothing about an earlier command
//! whose outcome is unknown (spec/lifecycle.md §5, research/lab-automation/03_micromanager
//! finding 2). Reconciliation must then refuse to conclude, in either direction, and leave
//! the command to a person.
#![allow(clippy::unwrap_used)]

mod common;

use common::*;
use openmhs_engine::journal::Journal;
use openmhs_policy::Caller;
use openmhs_sim::ExecFault;
use openmhs_types::CommandState;
use serde_json::json;

use CommandState::*;

const SUPERSEDED: &str = "command.reconcile.superseded";

fn agent() -> Caller {
    Caller::in_process(AGENT)
}

fn operator() -> Caller {
    Caller::in_process(OPERATOR)
}

fn timed(capability: &str, value: f64) -> serde_json::Value {
    with(write(capability, volts(value)), "deadline_ms", json!(2_000))
}

#[tokio::test(start_paused = true)]
async fn an_applied_write_superseded_by_a_newer_one_is_not_reported_not_applied() {
    let (engine, psu) = setup(allow_all()).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::ApplyThenHang));
    let lost = engine
        .submit(&agent(), &timed(SETPOINT, 1.0))
        .await
        .unwrap();
    assert_eq!(lost.state, UnknownOutcome);
    psu.clear_faults();

    // An idempotent write is not held back by the unknown one (lifecycle §3 rule 3).
    let newer = engine
        .submit(&agent(), &timed(SETPOINT, 2.0))
        .await
        .unwrap();
    assert_eq!(newer.state, Verified);

    // The reading (2 V) differs from the lost command's target (1 V), and that command did
    // apply. "Not applied" would be false.
    let refused = engine.reconcile(&lost.command_id).await;
    let Err(e) = refused else {
        panic!("reconciliation concluded: {:?}", refused.unwrap().state);
    };
    assert_eq!(e.code(), SUPERSEDED);
    assert!(e.to_string().contains(&newer.command_id), "{e}");
    assert_eq!(
        engine.receipt(&lost.command_id).unwrap().unwrap().state,
        UnknownOutcome,
        "the command stays uncertain"
    );
}

#[tokio::test(start_paused = true)]
async fn a_never_applied_write_superseded_by_the_same_value_is_not_reported_applied() {
    let (engine, psu) = setup(allow_all()).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::Hang));
    let lost = engine
        .submit(&agent(), &timed(SETPOINT, 1.0))
        .await
        .unwrap();
    assert_eq!(lost.state, UnknownOutcome);
    psu.clear_faults();
    let newer = engine
        .submit(&agent(), &timed(SETPOINT, 1.0))
        .await
        .unwrap();
    assert_eq!(newer.state, Verified);

    // The reading matches, but it is the newer command's effect; the lost one never applied.
    let refused = engine.reconcile(&lost.command_id).await;
    assert_eq!(refused.unwrap_err().code(), SUPERSEDED);
}

#[tokio::test(start_paused = true)]
async fn a_person_can_still_settle_a_superseded_command_on_the_record() {
    let (engine, psu) = setup(allow_all()).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::ApplyThenHang));
    let lost = engine
        .submit(&agent(), &timed(SETPOINT, 1.0))
        .await
        .unwrap();
    psu.clear_faults();
    engine
        .submit(&agent(), &timed(SETPOINT, 2.0))
        .await
        .unwrap();
    assert_eq!(
        engine.reconcile(&lost.command_id).await.unwrap_err().code(),
        SUPERSEDED
    );

    let settled = engine
        .resolve(
            &operator(),
            &lost.command_id,
            true,
            "device log shows both set-points were received",
        )
        .unwrap();
    assert_eq!(settled.state, ReconciledApplied);
}

#[tokio::test(start_paused = true)]
async fn a_later_write_the_device_refused_does_not_supersede() {
    let (engine, psu) = setup(allow_all()).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::ApplyThenHang));
    let lost = engine
        .submit(&agent(), &timed(SETPOINT, 1.0))
        .await
        .unwrap();
    psu.clear_faults();
    psu.update_faults(|f| f.execute = Some(ExecFault::Reject));
    let refused = engine
        .submit(&agent(), &timed(SETPOINT, 2.0))
        .await
        .unwrap();
    assert_eq!(refused.state, Rejected);
    psu.clear_faults();

    // The refused write changed nothing, so the reading still speaks for the lost command.
    let settled = engine.reconcile(&lost.command_id).await.unwrap();
    assert_eq!(settled.state, ReconciledApplied);
}

#[tokio::test(start_paused = true)]
async fn a_later_write_that_may_have_applied_supersedes_even_if_it_is_itself_unknown() {
    let (engine, psu) = setup(allow_all()).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::ApplyThenHang));
    let first = engine
        .submit(&agent(), &timed(SETPOINT, 1.0))
        .await
        .unwrap();
    let second = engine
        .submit(&agent(), &timed(SETPOINT, 2.0))
        .await
        .unwrap();
    assert_eq!(second.state, UnknownOutcome);
    psu.clear_faults();

    assert_eq!(
        engine
            .reconcile(&first.command_id)
            .await
            .unwrap_err()
            .code(),
        SUPERSEDED
    );
    // The newest one has nothing after it: the reading can speak for it.
    let settled = engine.reconcile(&second.command_id).await.unwrap();
    assert_eq!(settled.state, ReconciledApplied);
}

#[tokio::test(start_paused = true)]
async fn a_write_with_nothing_after_it_reconciles_as_before() {
    let (engine, psu) = setup(allow_all()).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::ApplyThenHang));
    let applied = engine
        .submit(&agent(), &timed(SETPOINT, 1.0))
        .await
        .unwrap();
    psu.clear_faults();
    assert_eq!(
        engine.reconcile(&applied.command_id).await.unwrap().state,
        ReconciledApplied
    );

    psu.update_faults(|f| f.execute = Some(ExecFault::Hang));
    let lost = engine
        .submit(&agent(), &timed(SETPOINT, 3.0))
        .await
        .unwrap();
    psu.clear_faults();
    assert_eq!(
        engine.reconcile(&lost.command_id).await.unwrap().state,
        ReconciledNotApplied
    );
}

#[tokio::test(start_paused = true)]
async fn the_supersession_is_chained_evidence_and_only_names_the_same_state() {
    let (engine, psu) = setup(allow_all()).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::ApplyThenHang));
    let lost = engine
        .submit(&agent(), &timed(SETPOINT, 1.0))
        .await
        .unwrap();
    psu.clear_faults();
    // A write to something else on the device is no supersession.
    let other = engine
        .submit(
            &agent(),
            &with(
                write(
                    "power.output.current.limit",
                    json!({"value": 0.5, "unit": "A"}),
                ),
                "deadline_ms",
                json!(2_000),
            ),
        )
        .await
        .unwrap();
    assert_eq!(other.state, Verified);
    let newer = engine
        .submit(&agent(), &timed(SETPOINT, 2.0))
        .await
        .unwrap();

    let records = engine.journal().audit_records().unwrap();
    let noted: Vec<_> = records
        .iter()
        .filter(|r| r.kind == "command.superseded")
        .collect();
    assert_eq!(noted.len(), 1, "one record, for the same-state write only");
    assert_eq!(noted[0].subject, lost.command_id);
    let data: serde_json::Value = serde_json::from_str(&noted[0].data).unwrap();
    assert_eq!(data["by"], json!(newer.command_id));
    assert_eq!(engine.verify_audit().unwrap().first_invalid, None);
}

#[tokio::test(start_paused = true)]
async fn the_refusal_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.sqlite");
    let (lost_id, newer_id) = {
        let (engine, psu) = setup_with_journal(allow_all(), Journal::open(&path).unwrap()).await;
        psu.update_faults(|f| f.execute = Some(ExecFault::ApplyThenHang));
        let lost = engine
            .submit(&agent(), &timed(SETPOINT, 1.0))
            .await
            .unwrap();
        psu.clear_faults();
        let newer = engine
            .submit(&agent(), &timed(SETPOINT, 2.0))
            .await
            .unwrap();
        (lost.command_id, newer.command_id)
    };
    let (engine, _psu) = setup_with_journal(allow_all(), Journal::open(&path).unwrap()).await;
    engine.recover().await.unwrap();
    let refused = engine.reconcile(&lost_id).await.unwrap_err();
    assert_eq!(refused.code(), SUPERSEDED);
    assert!(refused.to_string().contains(&newer_id), "{refused}");
}
