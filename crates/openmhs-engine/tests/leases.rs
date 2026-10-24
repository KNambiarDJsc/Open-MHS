//! M9: leases (ADR-0018, spec/topology.md). A live lease keeps everyone but its holder
//! from changing what it claims; leases are all or nothing, end on their deadline, and
//! can only be taken on devices the caller could operate.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use openmhs_engine::{Engine, EngineError, LeaseRequest};
use openmhs_policy::{AuthMethod, Caller, Effect, Grant, Policy, Scope};
use openmhs_types::descriptor::Operation;
use openmhs_types::{CommandState, Topology};

const A: &str = "agent.a";
const B: &str = "agent.b";
const AXIS: &str = "sim.axis-01";
const THERMO: &str = "sim.thermo-01";

fn everything(principal: &str, effect: Effect) -> Grant {
    Grant {
        id: format!("{principal}.all"),
        principal: principal.into(),
        device: "*".into(),
        capability: "*".into(),
        operations: vec![Operation::Read, Operation::Write, Operation::Invoke],
        effect,
        max_side_effect: None,
        reason: None,
    }
}

fn two_agents() -> Policy {
    Policy {
        grants: vec![everything(A, Effect::Allow), everything(B, Effect::Allow)],
        approvers: vec![OPERATOR.into()],
    }
}

/// The virtual lab with the example workcell topology: the stage and the sensor share
/// the bench.
async fn lab(policy: Policy) -> Engine {
    let engine = Engine::in_memory(clock(), policy).unwrap();
    for sim in openmhs_sim::virtual_lab(clock(), 7) {
        engine.register(sim.adapter).await.unwrap();
    }
    let topology = Topology::parse(include_str!(
        "../../../spec/examples/topology/virtual-workcell.json"
    ))
    .unwrap();
    engine.set_topology(topology).unwrap();
    engine
}

fn lease(devices: &[&str], ttl_ms: u64) -> LeaseRequest {
    LeaseRequest {
        devices: devices.iter().map(|d| (*d).to_owned()).collect(),
        ttl_ms,
        ..LeaseRequest::default()
    }
}

fn code(e: EngineError) -> String {
    match e {
        EngineError::Denied(e) => e.code,
        other => format!("{other:?}"),
    }
}

fn volts_on_psu(v: f64) -> serde_json::Value {
    write(SETPOINT, volts(v))
}

#[tokio::test(start_paused = true)]
async fn the_holder_operates_and_everyone_else_is_refused() {
    let engine = lab(two_agents()).await;
    let a = Caller::in_process(A);
    let b = Caller::in_process(B);
    let l = engine.acquire_lease(&a, lease(&[PSU], 60_000)).unwrap();
    assert_eq!(l.holder, A);
    assert!(l.claim.devices.contains(PSU));

    let refused = engine.submit(&b, &volts_on_psu(1.0)).await.unwrap();
    assert_eq!(refused.state, CommandState::Denied);
    assert_eq!(codes(&refused), ["lease.held"]);
    assert!(refused.policy.errors[0].message.contains(&l.id));
    // Reads are not leased.
    engine.read(&b, PSU, SETPOINT).await.unwrap();
    // The holder is not affected by its own lease.
    let ran = engine.submit(&a, &volts_on_psu(1.0)).await.unwrap();
    assert_eq!(ran.state, CommandState::Verified, "{ran:?}");

    engine.release_lease(&a, &l.id, None).unwrap();
    let after = engine.submit(&b, &volts_on_psu(1.5)).await.unwrap();
    assert_eq!(after.state, CommandState::Verified, "{after:?}");
}

/// Leasing the stage claims the sensor sharing its bench; a lease asking for anything
/// already claimed gets nothing at all.
#[tokio::test(start_paused = true)]
async fn leases_follow_the_graph_and_are_all_or_nothing() {
    let engine = lab(two_agents()).await;
    let a = Caller::in_process(A);
    let b = Caller::in_process(B);
    let held = engine.acquire_lease(&a, lease(&[AXIS], 60_000)).unwrap();
    assert!(held.claim.devices.contains(THERMO), "{:?}", held.claim);
    assert!(held.claim.resources.contains("workcell.bench"));
    assert!(
        !held.claim.devices.contains(PSU),
        "depends_on does not claim"
    );

    let refused = engine
        .acquire_lease(&b, lease(&[PSU, AXIS], 60_000))
        .unwrap_err();
    assert_eq!(code(refused), "lease.conflict");
    // Nothing of that request was taken: the supply is still free.
    let psu = engine.acquire_lease(&b, lease(&[PSU], 60_000)).unwrap();
    assert_eq!(engine.leases(true, 10).unwrap().len(), 2);
    assert_ne!(psu.id, held.id);
}

#[tokio::test(start_paused = true)]
async fn a_lease_ends_at_its_deadline() {
    let engine = lab(two_agents()).await;
    let a = Caller::in_process(A);
    let b = Caller::in_process(B);
    let l = engine.acquire_lease(&a, lease(&[PSU], 1_000)).unwrap();
    assert_eq!(
        engine.submit(&b, &volts_on_psu(1.0)).await.unwrap().state,
        CommandState::Denied
    );
    tokio::time::advance(Duration::from_millis(1_001)).await;
    // No sweeper needed: the deadline alone ends it.
    let after = engine.submit(&b, &volts_on_psu(1.0)).await.unwrap();
    assert_eq!(after.state, CommandState::Verified, "{after:?}");
    assert!(engine.leases(true, 10).unwrap().is_empty());
    assert_eq!(
        code(engine.renew_lease(&a, &l.id, 1_000).unwrap_err()),
        "lease.ended"
    );
    let expired = engine
        .journal()
        .audit_records()
        .unwrap()
        .into_iter()
        .any(|r| r.kind == "lease.expired" && r.subject == l.id);
    assert!(expired, "the ending is recorded once someone looks");
}

#[tokio::test(start_paused = true)]
async fn only_the_holder_renews_and_the_holder_or_an_approver_releases() {
    let engine = lab(two_agents()).await;
    let a = Caller::in_process(A);
    let b = Caller::in_process(B);
    let l = engine.acquire_lease(&a, lease(&[PSU], 1_000)).unwrap();
    assert_eq!(
        code(engine.renew_lease(&b, &l.id, 5_000).unwrap_err()),
        "lease.not_holder"
    );
    assert_eq!(
        code(engine.release_lease(&b, &l.id, None).unwrap_err()),
        "lease.not_holder"
    );
    let renewed = engine.renew_lease(&a, &l.id, 5_000).unwrap();
    assert!(renewed.expires_at > l.expires_at);
    tokio::time::advance(Duration::from_millis(2_000)).await;
    assert_eq!(
        engine.submit(&b, &volts_on_psu(1.0)).await.unwrap().state,
        CommandState::Denied,
        "renewed past the original deadline"
    );
    // An approver can free devices a holder left leased.
    let freed = engine
        .release_lease(&Caller::in_process(OPERATOR), &l.id, Some("left over"))
        .unwrap();
    assert_eq!(freed.released_by.as_deref(), Some(OPERATOR));
    assert_eq!(
        code(engine.release_lease(&a, &l.id, None).unwrap_err()),
        "lease.ended"
    );
}

/// A lease cannot be used to lock others out of devices the caller has no business with.
#[tokio::test(start_paused = true)]
async fn a_caller_can_only_lease_what_it_could_operate() {
    let mut policy = two_agents();
    policy.grants.push(Grant {
        id: "reader.all".into(),
        principal: "agent.reader".into(),
        device: "*".into(),
        capability: "*".into(),
        operations: vec![Operation::Read],
        effect: Effect::Allow,
        max_side_effect: None,
        reason: None,
    });
    let engine = lab(policy).await;
    let reader = Caller::in_process("agent.reader");
    assert_eq!(
        code(
            engine
                .acquire_lease(&reader, lease(&[PSU], 1_000))
                .unwrap_err()
        ),
        "lease.not_authorized"
    );
    // The sensor has nothing to write or invoke, so nobody leases it directly.
    let a = Caller::in_process(A);
    assert_eq!(
        code(
            engine
                .acquire_lease(&a, lease(&[THERMO], 1_000))
                .unwrap_err()
        ),
        "lease.not_authorized"
    );
    // A credential scoped to the sensor cannot even name the supply.
    let narrow = Caller::authenticated(
        A,
        "tok_narrow",
        AuthMethod::Credential,
        vec![Scope {
            device: THERMO.into(),
            capability: "*".into(),
            operations: vec![Operation::Read],
        }],
    );
    assert_eq!(
        code(
            engine
                .acquire_lease(&narrow, lease(&[PSU], 1_000))
                .unwrap_err()
        ),
        "lease.device.unknown"
    );
    assert_eq!(
        code(
            engine
                .acquire_lease(&a, lease(&["sim.ghost-01"], 1_000))
                .unwrap_err()
        ),
        "lease.device.unknown"
    );
    for ttl in [0, 24 * 60 * 60 * 1_000 + 1] {
        assert_eq!(
            code(engine.acquire_lease(&a, lease(&[PSU], ttl)).unwrap_err()),
            "lease.ttl.invalid"
        );
    }
    assert_eq!(
        code(engine.acquire_lease(&a, lease(&[], 1_000)).unwrap_err()),
        "lease.empty"
    );
    let unknown = LeaseRequest {
        resources: vec!["nowhere".into()],
        ttl_ms: 1_000,
        ..LeaseRequest::default()
    };
    assert_eq!(
        code(engine.acquire_lease(&a, unknown).unwrap_err()),
        "lease.resource.unknown"
    );
    // Leasing the bench needs the right to operate everything that shares it; the
    // sensor shares it and cannot be operated.
    let bench = LeaseRequest {
        resources: vec!["workcell.bench".into()],
        ttl_ms: 1_000,
        ..LeaseRequest::default()
    };
    assert_eq!(
        code(engine.acquire_lease(&a, bench).unwrap_err()),
        "lease.not_authorized"
    );
    assert!(
        engine.leases(false, 10).unwrap().is_empty(),
        "nothing was taken"
    );
}

/// A command held for approval is checked again when approved, so a lease taken in the
/// meantime stops it.
#[tokio::test(start_paused = true)]
async fn a_lease_taken_while_a_command_waits_for_approval_stops_it() {
    let mut policy = two_agents();
    policy.grants.insert(
        0,
        Grant {
            id: "b.psu.ask".into(),
            principal: B.into(),
            device: PSU.into(),
            capability: SETPOINT.into(),
            operations: vec![Operation::Write],
            effect: Effect::RequireApproval,
            max_side_effect: None,
            reason: None,
        },
    );
    let engine = lab(policy).await;
    let b = Caller::in_process(B);
    let waiting = engine.submit(&b, &volts_on_psu(1.0)).await.unwrap();
    assert_eq!(waiting.state, CommandState::AwaitingApproval);
    engine
        .acquire_lease(&Caller::in_process(A), lease(&[PSU], 60_000))
        .unwrap();
    let approved = engine
        .approve(&Caller::in_process(OPERATOR), &waiting.command_id)
        .await
        .unwrap();
    assert_eq!(approved.state, CommandState::Denied);
    assert_eq!(codes(&approved), ["lease.held"]);
}

/// Many callers racing for the same device: exactly one gets it.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_acquisitions_grant_exactly_one() {
    let mut policy = two_agents();
    for i in 0..8 {
        policy
            .grants
            .push(everything(&format!("agent.racer-{i}"), Effect::Allow));
    }
    let engine = Arc::new(lab(policy).await);
    let mut tasks = Vec::new();
    for i in 0..8 {
        let engine = engine.clone();
        tasks.push(tokio::spawn(async move {
            engine.acquire_lease(
                &Caller::in_process(format!("agent.racer-{i}")),
                lease(&[PSU], 60_000),
            )
        }));
    }
    let mut won = 0;
    for t in tasks {
        match t.await.unwrap() {
            Ok(_) => won += 1,
            Err(e) => assert_eq!(code(e), "lease.conflict"),
        }
    }
    assert_eq!(won, 1);
}

/// Leases are in the journal: a restarted gateway still honours them, and their deadlines.
#[tokio::test(start_paused = true)]
async fn leases_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.db");
    let open = || async {
        let engine = Engine::open(&path, clock(), two_agents()).unwrap();
        for sim in openmhs_sim::virtual_lab(clock(), 7) {
            engine.register(sim.adapter).await.unwrap();
        }
        engine.recover().await.unwrap();
        engine
    };
    let first = open().await;
    let l = first
        .acquire_lease(&Caller::in_process(A), lease(&[PSU], 10_000))
        .unwrap();
    drop(first);

    let second = open().await;
    let b = Caller::in_process(B);
    let refused = second.submit(&b, &volts_on_psu(1.0)).await.unwrap();
    assert_eq!(codes(&refused), ["lease.held"]);
    assert_eq!(second.lease(&l.id).unwrap().holder, A);
    tokio::time::advance(Duration::from_millis(10_001)).await;
    let after = second.submit(&b, &volts_on_psu(1.0)).await.unwrap();
    assert_eq!(after.state, CommandState::Verified, "{after:?}");
}
