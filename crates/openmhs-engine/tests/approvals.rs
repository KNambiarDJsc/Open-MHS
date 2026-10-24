//! M8: approvals are single-use, bound to one exact command, and expire (ADR-0017).
//!
//! Every test drives the real engine, journal and simulated supply. Where a test says a
//! device was or was not touched, it counts the simulator's own `execute` calls.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use common::*;
use openmhs_engine::Engine;
use openmhs_policy::{Caller, Effect, Policy};
use openmhs_types::descriptor::Operation;
use openmhs_types::{ApprovalStatus, CommandState};
use serde_json::json;

use CommandState::*;

const REQUESTER: &str = AGENT;

fn requester() -> Caller {
    Caller::in_process(REQUESTER)
}

fn approver() -> Caller {
    Caller::in_process(OPERATOR)
}

/// Writes to `power.output.enabled` need approval; everything else is allowed.
fn ask_to_enable() -> Policy {
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
    policy
}

fn enable(on: bool) -> serde_json::Value {
    write(ENABLED, json!(on))
}

async fn held(engine: &Engine, request: &serde_json::Value) -> openmhs_types::Receipt {
    let r = engine.submit(&requester(), request).await.unwrap();
    assert_eq!(r.state, AwaitingApproval, "{r:?}");
    r
}

/// A command held for approval has an approval record bound to it, PENDING, with the
/// fingerprint of exactly that command.
#[tokio::test(start_paused = true)]
async fn a_held_command_gets_a_pending_approval_bound_to_its_fingerprint() {
    let (engine, psu) = setup(ask_to_enable()).await;
    let r = held(&engine, &enable(true)).await;
    let a = engine
        .journal()
        .approval_for_command(&r.command_id)
        .unwrap()
        .expect("an approval record exists");
    assert_eq!(a.status, ApprovalStatus::Pending);
    assert_eq!(a.requester, REQUESTER);
    assert_eq!((a.device.as_str(), a.capability.as_str()), (PSU, ENABLED));
    assert_eq!(
        a.fingerprint,
        openmhs_engine::fingerprint(REQUESTER, &r.request)
    );
    assert_eq!(
        Some(a.expires_at),
        r.deadline_at,
        "approvals expire with the command"
    );
    assert_eq!(psu.execute_calls(), 0);
}

/// A command that requires approval cannot run without it, and runs once approved.
#[tokio::test(start_paused = true)]
async fn approved_commands_run_and_spend_their_approval() {
    let (engine, psu) = setup(ask_to_enable()).await;
    let r = held(&engine, &enable(true)).await;
    assert_eq!(psu.execute_calls(), 0, "held means not dispatched");

    let done = engine.approve(&approver(), &r.command_id).await.unwrap();
    assert_eq!(done.state, Verified);
    assert_eq!(done.policy.approved_by.as_deref(), Some(OPERATOR));
    assert_eq!(psu.execute_calls(), 1);

    let a = engine
        .journal()
        .approval_for_command(&r.command_id)
        .unwrap()
        .unwrap();
    assert_eq!(a.status, ApprovalStatus::Consumed);
    assert_eq!(a.decided_by.as_deref(), Some(OPERATOR));
    assert!(a.consumed_at.is_some());
}

/// Approving twice does not run the command twice.
#[tokio::test(start_paused = true)]
async fn an_approval_cannot_be_used_twice() {
    let (engine, psu) = setup(ask_to_enable()).await;
    let r = held(&engine, &enable(true)).await;
    engine.approve(&approver(), &r.command_id).await.unwrap();
    let again = engine
        .approve(&approver(), &r.command_id)
        .await
        .unwrap_err();
    assert_eq!(again.code(), "command.invalid_state");
    assert_eq!(psu.execute_calls(), 1);
}

/// Two approvers racing to approve the same held command: one wins, the device is
/// written once.
#[tokio::test(start_paused = true)]
async fn concurrent_approvals_do_not_duplicate_execution() {
    let mut policy = ask_to_enable();
    policy.approvers.push("human.second".into());
    let (engine, psu) = setup(policy).await;
    let r = held(&engine, &enable(true)).await;
    let tasks: Vec<_> = [OPERATOR, "human.second"]
        .into_iter()
        .map(|who| {
            let (e, id) = (engine.clone(), r.command_id.clone());
            tokio::spawn(async move { e.approve(&Caller::in_process(who), &id).await })
        })
        .collect();
    let mut ok = 0;
    for t in tasks {
        if t.await.unwrap().is_ok() {
            ok += 1;
        }
    }
    assert_eq!(ok, 1, "exactly one approval wins");
    assert_eq!(psu.execute_calls(), 1);
}

/// An approval whose time has passed is refused, the command never runs, and the record
/// says EXPIRED.
#[tokio::test(start_paused = true)]
async fn an_expired_approval_is_rejected() {
    let (engine, psu) = setup(ask_to_enable()).await;
    let r = held(&engine, &with(enable(true), "deadline_ms", json!(2_000))).await;
    tokio::time::advance(Duration::from_millis(2_500)).await;
    let late = engine.approve(&approver(), &r.command_id).await.unwrap();
    assert_eq!(late.state, Expired);
    assert_eq!(psu.execute_calls(), 0);
    let a = engine
        .journal()
        .approval_for_command(&r.command_id)
        .unwrap()
        .unwrap();
    assert_eq!(a.status, ApprovalStatus::Expired);
}

/// A revoked approval cannot be used, and revoking one withdraws the command waiting on it.
#[tokio::test(start_paused = true)]
async fn a_revoked_approval_is_rejected() {
    let (engine, psu) = setup(ask_to_enable()).await;
    let r = held(&engine, &enable(true)).await;
    let a = engine
        .journal()
        .approval_for_command(&r.command_id)
        .unwrap()
        .unwrap();
    let revoked = engine
        .revoke_approval(&approver(), &a.id, Some("changed my mind".into()))
        .unwrap();
    assert_eq!(revoked.status, ApprovalStatus::Revoked);
    assert_eq!(
        engine.receipt(&r.command_id).unwrap().unwrap().state,
        Cancelled
    );
    let err = engine
        .approve(&approver(), &r.command_id)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "command.invalid_state");
    assert_eq!(psu.execute_calls(), 0);
}

/// Pre-approval: ask, get approved, then submit the same command naming the approval.
#[tokio::test(start_paused = true)]
async fn an_approval_requested_ahead_runs_exactly_its_command() {
    let (engine, psu) = setup(ask_to_enable()).await;
    let a = engine
        .request_approval(&requester(), &enable(true), None)
        .await
        .unwrap();
    assert_eq!(a.status, ApprovalStatus::Pending);
    assert_eq!(a.command_id, None);

    // Not approved yet: nothing runs.
    let early = engine
        .submit_approved(&requester(), &enable(true), &a.id)
        .await
        .unwrap();
    assert_eq!(early.state, Denied);
    assert_eq!(codes(&early), vec!["approval.pending"]);

    let approved = engine.approve_approval(&approver(), &a.id).await.unwrap();
    assert_eq!(approved.status, ApprovalStatus::Approved);
    assert_eq!(psu.execute_calls(), 0, "approving ahead runs nothing");

    let run = engine
        .submit_approved(&requester(), &enable(true), &a.id)
        .await
        .unwrap();
    assert_eq!(run.state, Verified);
    assert_eq!(run.policy.approved_by.as_deref(), Some(OPERATOR));
    assert_eq!(psu.execute_calls(), 1);
    let spent = engine.journal().approval(&a.id).unwrap().unwrap();
    assert_eq!(spent.status, ApprovalStatus::Consumed);
    assert_eq!(spent.command_id.as_deref(), Some(run.command_id.as_str()));
}

/// An approved command that is then changed in any way that matters is refused, and the
/// approval is not spent on it.
#[tokio::test(start_paused = true)]
async fn a_modified_command_is_rejected() {
    let (engine, psu) = setup(ask_to_enable()).await;
    let a = engine
        .request_approval(&requester(), &enable(true), None)
        .await
        .unwrap();
    engine.approve_approval(&approver(), &a.id).await.unwrap();

    // Same capability, opposite value: it needs approval too, but not this approval.
    let flipped = engine
        .submit_approved(&requester(), &enable(false), &a.id)
        .await
        .unwrap();
    assert_eq!(flipped.state, Denied);
    assert_eq!(codes(&flipped), vec!["approval.mismatch"]);
    // An added parameter changes what is being asked, so it is refused as well.
    let padded = engine
        .submit_approved(
            &requester(),
            &with(
                enable(true),
                "params",
                json!({ "extra": { "value": 1, "unit": "V" } }),
            ),
            &a.id,
        )
        .await
        .unwrap();
    assert_eq!(padded.state, Denied);
    assert_eq!(psu.execute_calls(), 0);
    // The approval is still there for the command it was given for.
    let still = engine.journal().approval(&a.id).unwrap().unwrap();
    assert_eq!(still.status, ApprovalStatus::Approved);
    let run = engine
        .submit_approved(&requester(), &enable(true), &a.id)
        .await
        .unwrap();
    assert_eq!(run.state, Verified);
}

/// Only the requester can spend an approval.
#[tokio::test(start_paused = true)]
async fn another_principal_cannot_spend_an_approval() {
    let mut policy = ask_to_enable();
    policy.grants.push(openmhs_policy::Grant {
        principal: "agent.other".into(),
        ..grant(
            "other.all",
            "*",
            &[Operation::Read, Operation::Write],
            Effect::RequireApproval,
        )
    });
    let (engine, psu) = setup(policy).await;
    let a = engine
        .request_approval(&requester(), &enable(true), None)
        .await
        .unwrap();
    engine.approve_approval(&approver(), &a.id).await.unwrap();
    let thief = Caller::in_process("agent.other");
    let r = engine
        .submit_approved(&thief, &enable(true), &a.id)
        .await
        .unwrap();
    assert_eq!(r.state, Denied);
    assert_eq!(codes(&r), vec!["approval.mismatch"]);
    assert_eq!(psu.execute_calls(), 0);
}

/// Spending an approval twice, even with different idempotency keys, runs one command.
#[tokio::test(start_paused = true)]
async fn a_spent_approval_is_not_reusable() {
    let (engine, psu) = setup(ask_to_enable()).await;
    let a = engine
        .request_approval(&requester(), &enable(true), None)
        .await
        .unwrap();
    engine.approve_approval(&approver(), &a.id).await.unwrap();
    let first = engine
        .submit_approved(
            &requester(),
            &with(enable(true), "idempotency_key", json!("k1")),
            &a.id,
        )
        .await
        .unwrap();
    assert_eq!(first.state, Verified);
    let second = engine
        .submit_approved(
            &requester(),
            &with(enable(true), "idempotency_key", json!("k2")),
            &a.id,
        )
        .await
        .unwrap();
    assert_eq!(second.state, Denied);
    assert_eq!(codes(&second), vec!["approval.consumed"]);
    assert_eq!(psu.execute_calls(), 1);
}

/// Many submissions racing on one approval: exactly one command runs.
#[tokio::test(start_paused = true)]
async fn concurrent_consumption_runs_one_command() {
    let (engine, psu) = setup(ask_to_enable()).await;
    let a = engine
        .request_approval(&requester(), &enable(true), None)
        .await
        .unwrap();
    engine.approve_approval(&approver(), &a.id).await.unwrap();
    let tasks: Vec<_> = (0..8)
        .map(|i| {
            let (e, id) = (engine.clone(), a.id.clone());
            tokio::spawn(async move {
                let req = with(enable(true), "idempotency_key", json!(format!("race-{i}")));
                e.submit_approved(&Caller::in_process(REQUESTER), &req, &id)
                    .await
                    .unwrap()
            })
        })
        .collect();
    let mut verified = 0;
    let mut refused = 0;
    for t in tasks {
        match t.await.unwrap().state {
            Verified => verified += 1,
            Denied => refused += 1,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!((verified, refused), (1, 7));
    assert_eq!(psu.execute_calls(), 1, "the device was written once");
}

/// Policy is evaluated at the moment of use. An approval does not outlive the grant.
#[tokio::test(start_paused = true)]
async fn policy_still_applies_after_approval() {
    let (engine, psu) = setup(ask_to_enable()).await;
    let a = engine
        .request_approval(&requester(), &enable(true), None)
        .await
        .unwrap();
    engine.approve_approval(&approver(), &a.id).await.unwrap();
    // The operator withdraws the agent's access altogether.
    engine
        .set_policy(Policy {
            grants: vec![],
            approvers: vec![OPERATOR.into()],
        })
        .unwrap();
    let r = engine
        .submit_approved(&requester(), &enable(true), &a.id)
        .await
        .unwrap();
    assert_eq!(r.state, Denied);
    assert_eq!(codes(&r), vec!["policy.denied.no_grant"]);
    assert_eq!(psu.execute_calls(), 0);
}

/// If the held command is altered in the journal after it was held, approving it fails
/// closed: the fingerprint is recomputed from what is stored now.
#[tokio::test(start_paused = true)]
async fn tampering_with_a_held_command_is_caught_at_approval() {
    let (engine, psu) = setup(ask_to_enable()).await;
    let r = held(&engine, &enable(false)).await;
    // Someone with the database flips the stored value.
    let changed = engine
        .journal()
        .execute_raw(
            "UPDATE commands SET receipt = replace(receipt, '\"value\":false', '\"value\":true') \
             WHERE state = 'AWAITING_APPROVAL'",
        )
        .unwrap();
    assert_eq!(changed, 1, "the test must really change the stored command");
    let out = engine.approve(&approver(), &r.command_id).await.unwrap();
    assert_eq!(out.state, Denied);
    assert_eq!(codes(&out), vec!["approval.mismatch"]);
    assert_eq!(psu.execute_calls(), 0);
}

/// Who may decide: not an agent, not the requester, not a non-approver.
#[tokio::test(start_paused = true)]
async fn only_a_human_approver_other_than_the_requester_can_decide() {
    let mut policy = ask_to_enable();
    policy.approvers.push("agent:sneaky".into());
    policy.approvers.push(REQUESTER.into());
    let (engine, _psu) = setup(policy).await;
    let a = engine
        .request_approval(&requester(), &enable(true), None)
        .await
        .unwrap();
    let code = |who: Caller| {
        let engine = engine.clone();
        let id = a.id.clone();
        async move {
            engine
                .approve_approval(&who, &id)
                .await
                .unwrap_err()
                .code()
                .to_owned()
        }
    };
    assert_eq!(
        code(Caller::in_process("agent:sneaky")).await,
        "policy.approval.not_human"
    );
    assert_eq!(
        code(Caller::in_process("human.random")).await,
        "policy.approval.not_approver"
    );
    assert_eq!(code(requester()).await, "policy.approval.self");
    // An approver whose credential does not reach the device cannot decide either.
    let narrow = Caller::authenticated(
        OPERATOR,
        "tok_narrow",
        openmhs_policy::AuthMethod::Credential,
        vec![openmhs_policy::Scope {
            device: "sim.axis-01".into(),
            capability: "*".into(),
            operations: vec![Operation::Write],
        }],
    );
    assert_eq!(code(narrow).await, "auth.scope.denied");
}

/// Approvals that policy does not require, or that the command could never pass, are not issued.
#[tokio::test(start_paused = true)]
async fn approvals_are_only_issued_for_commands_that_need_one() {
    let (engine, _psu) = setup(ask_to_enable()).await;
    let free = engine
        .request_approval(&requester(), &write(SETPOINT, volts(1.0)), None)
        .await
        .unwrap_err();
    assert_eq!(free.code(), "approval.not_required");
    let impossible = engine
        .request_approval(&requester(), &write(SETPOINT, volts(9_999.0)), None)
        .await
        .unwrap_err();
    assert!(
        impossible.code().starts_with("command.") || impossible.code().starts_with("constraint"),
        "{}",
        impossible.code()
    );
    let nobody = engine
        .request_approval(&Caller::in_process("agent.stranger"), &enable(true), None)
        .await
        .unwrap_err();
    assert_eq!(nobody.code(), "policy.denied.no_grant");
    let ttl = engine
        .request_approval(&requester(), &enable(true), Some(0))
        .await
        .unwrap_err();
    assert_eq!(ttl.code(), "approval.ttl.invalid");
}

/// Pre-approvals expire too.
#[tokio::test(start_paused = true)]
async fn a_pre_approval_expires() {
    let (engine, psu) = setup(ask_to_enable()).await;
    let a = engine
        .request_approval(&requester(), &enable(true), Some(1_000))
        .await
        .unwrap();
    engine.approve_approval(&approver(), &a.id).await.unwrap();
    tokio::time::advance(Duration::from_millis(1_500)).await;
    let r = engine
        .submit_approved(&requester(), &enable(true), &a.id)
        .await
        .unwrap();
    assert_eq!(r.state, Denied);
    assert_eq!(codes(&r), vec!["approval.expired"]);
    assert_eq!(
        engine.journal().approval(&a.id).unwrap().unwrap().status,
        ApprovalStatus::Expired
    );
    assert_eq!(psu.execute_calls(), 0);
}

/// Every approval change is audited, and the hash chain stays intact.
#[tokio::test(start_paused = true)]
async fn approval_changes_are_audited() {
    let (engine, _psu) = setup(ask_to_enable()).await;
    let r = held(&engine, &enable(true)).await;
    engine.approve(&approver(), &r.command_id).await.unwrap();
    let kinds: Vec<String> = engine
        .journal()
        .audit_records()
        .unwrap()
        .into_iter()
        .map(|a| a.kind)
        .filter(|k| k.starts_with("approval."))
        .collect();
    assert_eq!(
        kinds,
        vec![
            "approval.requested",
            "approval.approved",
            "approval.consumed"
        ]
    );
    assert!(engine.verify_audit().unwrap().is_valid());
}
