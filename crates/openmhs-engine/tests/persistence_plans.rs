//! Persistence adversary, part three: plans and leases. A fault at every journal write a
//! two-step plan makes (its record, its lease, each step's start, the link from a step to
//! its command, each step's result), before and after the commit, as one I/O error or as a
//! crash followed by a restart. Then an operator settles the plan the documented way:
//! reconcile an uncertain step, then resume, or retry a step that never ran.
//!
//! The property: each step reaches the device exactly once over the whole run, recovery
//! sends nothing, the plan finishes, and the evidence verifies. Leases: a crash never
//! leaves two holders, and a lease never outlives its deadline.
//!
//! The report is docs/architecture/PERSISTENCE_ADVERSARY.md.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use common::*;
use openmhs_adapter::{
    AdapterCommand, AdapterError, ExecuteReport, HardwareAdapter, Health, Progress,
};
use openmhs_engine::journal::{Fault, FaultEffect, FaultPhase, Journal};
use openmhs_engine::{Engine, EngineOptions, LeaseRequest};
use openmhs_policy::Caller;
use openmhs_sim::{PsuHandle, VirtualPsu};
use openmhs_types::evidence::verify_evidence;
use openmhs_types::plan::{PlanDecision, PlanSpec, PlanState};
use openmhs_types::{Clock, CommandState, StateValue};
use serde_json::{Value as Json, json};

fn agent() -> Caller {
    Caller::in_process(AGENT)
}

fn operator() -> Caller {
    Caller::in_process(OPERATOR)
}

/// The supply, counting `execute` calls per capability.
struct Counted {
    inner: NotIdempotent,
    calls: Arc<Mutex<BTreeMap<String, u64>>>,
}

#[async_trait]
impl HardwareAdapter for Counted {
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
    async fn execute(&self, c: AdapterCommand, p: Progress) -> Result<ExecuteReport, AdapterError> {
        *self
            .calls
            .lock()
            .unwrap()
            .entry(c.capability.clone())
            .or_default() += 1;
        self.inner.execute(c, p).await
    }
    async fn shutdown(&self) -> Result<(), AdapterError> {
        self.inner.shutdown().await
    }
}

type Calls = Arc<Mutex<BTreeMap<String, u64>>>;

fn device(clock: &Arc<dyn Clock>) -> (Arc<Counted>, PsuHandle, Calls) {
    let (psu, handle) = VirtualPsu::new(clock.clone(), 42);
    let calls = Calls::default();
    (
        Arc::new(Counted {
            inner: NotIdempotent {
                inner: psu,
                capability: SETPOINT,
            },
            calls: calls.clone(),
        }),
        handle,
        calls,
    )
}

async fn gateway(path: &Path, clock: &Arc<dyn Clock>, device: Arc<dyn HardwareAdapter>) -> Engine {
    let engine = Engine::with_journal(
        Journal::open(path).unwrap(),
        clock.clone(),
        allow_all(),
        EngineOptions::default(),
    );
    engine.register(device).await.unwrap();
    engine
}

fn plan() -> PlanSpec {
    serde_json::from_value(json!({
        "steps": [
            { "id": "set-voltage", "request": write(SETPOINT, volts(0.8)), "on_failure": "ask" },
            { "id": "output-on", "after": ["set-voltage"], "request": write(ENABLED, json!(true)), "on_failure": "ask" },
        ]
    }))
    .unwrap()
}

/// Every plan and lease write a clean run makes, in order, with repeats.
async fn plan_sites() -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let clock = clock();
    let (device, _psu, _calls) = device(&clock);
    let engine = gateway(&dir.path().join("j.sqlite"), &clock, device).await;
    let sites: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = sites.clone();
    engine
        .journal()
        .observe_commits(Box::new(move |site, phase| {
            if phase == FaultPhase::BeforeCommit
                && (site.starts_with("plan") || site.starts_with("lease"))
            {
                seen.lock().unwrap().push(site.to_owned());
            }
        }));
    let p = engine.submit_plan(&agent(), plan()).await.unwrap();
    assert_eq!(p.state, PlanState::Succeeded, "{p:?}");
    sites.lock().unwrap().clone()
}

/// Arm `fault` for the `nth` (0-based) write at `site`.
fn arm_at(engine: &Engine, site: String, nth: usize, phase: FaultPhase, effect: FaultEffect) {
    let target = engine.clone();
    let seen = std::sync::atomic::AtomicUsize::new(0);
    engine.journal().observe_commits(Box::new(move |s, p| {
        if s == site
            && p == FaultPhase::BeforeCommit
            && seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == nth
        {
            target.journal().inject(Fault {
                site: site.clone(),
                phase,
                effect,
            });
        }
    }));
}

/// Settle a plan as an operator would: reconcile an uncertain step, then resume; retry a
/// step that did not run. Returns the final plan, if there is one.
async fn settle(engine: &Engine, label: &str) -> Option<openmhs_types::plan::Plan> {
    let id = engine.plans(&operator(), 10).unwrap().first()?.id.clone();
    for _ in 0..8 {
        let p = engine.plan(&operator(), &id).unwrap();
        if p.state.is_terminal() {
            return Some(p);
        }
        assert_eq!(p.state, PlanState::AwaitingDecision, "{label}: {p:?}");
        for command in p.steps.iter().flat_map(|s| &s.commands) {
            if engine.receipt(command).unwrap().unwrap().state == CommandState::UnknownOutcome {
                engine.reconcile(command).await.unwrap();
            }
        }
        let p = engine.plan(&operator(), &id).unwrap();
        if p.state != PlanState::AwaitingDecision {
            continue;
        }
        if engine
            .decide_plan(&operator(), &id, PlanDecision::Resume, None)
            .await
            .is_err()
        {
            engine
                .decide_plan(&operator(), &id, PlanDecision::Retry, None)
                .await
                .unwrap_or_else(|e| panic!("{label}: neither resume nor retry: {e}"));
        }
    }
    panic!("{label}: the plan did not finish");
}

async fn plan_drill(site: &str, nth: usize, phase: FaultPhase, effect: FaultEffect) {
    let label = format!("{site}#{nth} {phase:?} {effect:?}");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.sqlite");
    let clock = clock();
    let (device, psu, calls) = device(&clock);

    let engine = gateway(&path, &clock, device.clone()).await;
    arm_at(&engine, site.to_owned(), nth, phase, effect);
    let _ = engine.submit_plan(&agent(), plan()).await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    for (capability, n) in calls.lock().unwrap().iter() {
        assert!(*n <= 1, "{label}: {capability} sent {n} times");
    }
    drop(engine);

    let engine = gateway(&path, &clock, device).await;
    let before = calls.lock().unwrap().clone();
    engine.recover().await.unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        before,
        "{label}: recovery sent something"
    );

    match settle(&engine, &label).await {
        None => assert!(before.is_empty(), "{label}: no plan, but {before:?} sent"),
        Some(p) => {
            assert_eq!(p.state, PlanState::Succeeded, "{label}: {p:?}");
            let calls = calls.lock().unwrap().clone();
            assert_eq!(calls.get(SETPOINT), Some(&1), "{label}: {calls:?}");
            assert_eq!(calls.get(ENABLED), Some(&1), "{label}: {calls:?}");
            let truth = psu.truth();
            assert!(
                truth.setpoint_v == 0.8 && truth.enabled,
                "{label}: {truth:?}"
            );
        }
    }
    let report = verify_evidence(&engine.journal().export_evidence().unwrap(), None);
    assert!(report.valid && report.complete, "{label}: {report:?}");
}

#[tokio::test(start_paused = true)]
async fn a_fault_at_any_plan_or_lease_write_never_repeats_a_step() {
    let sites = plan_sites().await;
    assert!(
        sites.iter().any(|s| s == "plan.link") && sites.iter().any(|s| s.starts_with("lease:")),
        "{sites:?}"
    );
    let mut cases = 0;
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for site in &sites {
        let nth = counts.entry(site).or_default();
        for phase in [FaultPhase::BeforeCommit, FaultPhase::AfterCommit] {
            for effect in [FaultEffect::IoError, FaultEffect::Crash] {
                plan_drill(site, *nth, phase, effect).await;
                cases += 1;
            }
        }
        *nth += 1;
    }
    println!("{cases} plan cases over {} writes: {sites:?}", sites.len());
}

/// A crash on either side of taking or releasing a lease: after the restart there is at
/// most one holder, the holder can still use the device, another principal cannot, and
/// the lease ends at its deadline whatever was recorded.
#[tokio::test(start_paused = true)]
async fn a_crash_while_taking_or_releasing_a_lease_never_leaves_two_holders() {
    let other = Caller::in_process("agent.other");
    let mut policy = allow_all();
    let mut g = policy.grants[0].clone();
    g.id = "other".into();
    g.principal = "agent.other".into();
    policy.grants.push(g);
    let request = || LeaseRequest {
        devices: vec![PSU.into()],
        resources: vec![],
        ttl_ms: 60_000,
        reason: None,
        plan_id: None,
    };
    for (site, phase) in [
        ("lease:acquired", FaultPhase::BeforeCommit),
        ("lease:acquired", FaultPhase::AfterCommit),
        ("lease:released", FaultPhase::BeforeCommit),
        ("lease:released", FaultPhase::AfterCommit),
    ] {
        let label = format!("{site} {phase:?}");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.sqlite");
        let clock = clock();
        let (device, _psu, calls) = device(&clock);
        let open = |policy: openmhs_policy::Policy| {
            let (path, clock, device) = (path.clone(), clock.clone(), device.clone());
            async move {
                let engine = Engine::with_journal(
                    Journal::open(&path).unwrap(),
                    clock,
                    policy,
                    EngineOptions::default(),
                );
                engine.register(device).await.unwrap();
                engine
            }
        };

        let engine = open(policy.clone()).await;
        let fault = Fault {
            site: site.into(),
            phase,
            effect: FaultEffect::Crash,
        };
        let held = if site == "lease:acquired" {
            engine.journal().inject(fault);
            engine.acquire_lease(&agent(), request()).ok()
        } else {
            let lease = engine.acquire_lease(&agent(), request()).unwrap();
            engine.journal().inject(fault);
            assert!(
                engine.release_lease(&agent(), &lease.id, None).is_err(),
                "{label}"
            );
            Some(lease)
        };
        drop(engine);

        let engine = open(policy.clone()).await;
        engine.recover().await.unwrap();
        let active = engine.leases(true, 10).unwrap();
        assert!(active.len() <= 1, "{label}: {active:?}");
        let recorded = active.first().cloned();
        match (site, phase) {
            ("lease:acquired", FaultPhase::BeforeCommit)
            | ("lease:released", FaultPhase::AfterCommit) => {
                assert!(recorded.is_none(), "{label}: {recorded:?}");
            }
            _ => assert!(recorded.is_some(), "{label}"),
        }
        let _ = held;
        let before = calls.lock().unwrap().values().sum::<u64>();
        let theirs = engine
            .submit(&other, &write(ENABLED, json!(true)))
            .await
            .unwrap();
        if recorded.is_some() {
            assert_eq!(theirs.state, CommandState::Denied, "{label}: {theirs:?}");
            assert_eq!(
                calls.lock().unwrap().values().sum::<u64>(),
                before,
                "{label}"
            );
            let mine = engine
                .submit(&agent(), &write(ENABLED, json!(false)))
                .await
                .unwrap();
            assert_eq!(mine.state, CommandState::Verified, "{label}: {mine:?}");
            // Whatever the journal says, the lease ends at its deadline.
            tokio::time::sleep(Duration::from_secs(61)).await;
            assert!(engine.leases(true, 10).unwrap().is_empty(), "{label}");
            let later = engine
                .submit(&other, &write(ENABLED, json!(true)))
                .await
                .unwrap();
            assert_eq!(later.state, CommandState::Verified, "{label}: {later:?}");
        } else {
            assert_eq!(theirs.state, CommandState::Verified, "{label}: {theirs:?}");
        }
        let report = verify_evidence(&engine.journal().export_evidence().unwrap(), None);
        assert!(report.valid && report.complete, "{label}: {report:?}");
    }
}
