//! Persistence adversary: a fault at every durable boundary of a command, and what the
//! gateway does about it. The property under test, for every case:
//!
//! - nothing is sent again automatically: one request reaches the device at most once;
//! - a command that may have reached the device is never recorded as not executed;
//! - while such a command is unsettled, no further write on its capability is sent;
//! - after a restart, recovery sends nothing and leaves nothing in flight;
//! - the audit evidence stays valid and complete.
//!
//! Faults are injected at the journal (`Journal::inject`): an I/O error before the commit
//! (the transaction rolls back) or after it (stored, but the caller is told it failed),
//! either once (`IoError`) or followed by every later write failing, as a stopped process
//! makes none (`Crash`); the file is then reopened by a new engine on the same simulated
//! device. Disk full is SQLite's own `SQLITE_FULL` (`max_page_count`). One case kills a
//! real process with `abort()` at the boundary and recovers in this one.
//!
//! The report is docs/architecture/PERSISTENCE_ADVERSARY.md.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use common::*;
use openmhs_adapter::{
    AdapterCommand, AdapterError, ExecuteReport, HardwareAdapter, Health, Progress,
};
use openmhs_engine::journal::{Fault, FaultEffect, FaultPhase, Journal};
use openmhs_engine::{Engine, EngineOptions, StorageLimits};
use openmhs_policy::{Caller, Effect, Policy};
use openmhs_sim::{PsuHandle, VirtualPsu};
use openmhs_types::descriptor::Operation;
use openmhs_types::evidence::verify_evidence;
use openmhs_types::{Clock, CommandState, Receipt, StateValue};
use serde_json::{Value as Json, json};

use CommandState::*;

const UNRESOLVED: &str = "command.outcome_unresolved";

/// Every durable write a successful command makes, in order.
const COMMAND_SITES: &[&str] = &[
    "command.created",
    "transition:VALIDATING",
    "transition:APPROVED",
    "expectation",
    "transition:DISPATCHED",
    "transition:ACCEPTED",
    "transition:APPLIED",
    "transition:VERIFIED",
];

fn agent() -> Caller {
    Caller::in_process(AGENT)
}

/// The physical supply, with the setpoint declared not idempotent so that a repeat is a
/// second effect and the unresolved hold applies. It outlives gateway restarts.
fn device(clock: &Arc<dyn Clock>) -> (Arc<NotIdempotent>, PsuHandle) {
    let (psu, handle) = VirtualPsu::new(clock.clone(), 42);
    (
        Arc::new(NotIdempotent {
            inner: psu,
            capability: SETPOINT,
        }),
        handle,
    )
}

async fn gateway(
    path: &Path,
    clock: &Arc<dyn Clock>,
    policy: Policy,
    device: Arc<dyn HardwareAdapter>,
) -> Engine {
    let engine = Engine::with_journal(
        Journal::open(path).unwrap(),
        clock.clone(),
        policy,
        EngineOptions::default(),
    );
    engine.register(device).await.unwrap();
    engine
}

fn set(v: f64, key: Option<&str>) -> Json {
    let mut r = write(SETPOINT, volts(v));
    if let Some(key) = key {
        r["idempotency_key"] = json!(key);
    }
    r
}

/// The device may have acted on it and nobody has settled what happened.
fn unsettled(state: CommandState) -> bool {
    matches!(state, Dispatched | Accepted | TimedOut | UnknownOutcome)
}

/// The record says the device never acted on it.
fn says_not_executed(state: CommandState) -> bool {
    matches!(
        state,
        Created
            | Validating
            | Denied
            | AwaitingApproval
            | Approved
            | Expired
            | Cancelled
            | Rejected
    )
}

fn first(engine: &Engine) -> Option<Receipt> {
    engine.receipt_by_key(AGENT, PSU, "first").unwrap()
}

fn evidence_complete(engine: &Engine, label: &str) {
    let report = verify_evidence(&engine.journal().export_evidence().unwrap(), None);
    assert!(report.valid && report.complete, "{label}: {report:?}");
}

/// While the journal still accepts writes: the same request is not sent again, and while
/// it is unsettled nothing else on its capability is sent.
async fn same_gateway_carries_on(engine: &Engine, psu: &PsuHandle, label: &str, sent: u64) {
    let record = first(engine);
    match &record {
        None => assert_eq!(sent, 0, "{label}: sent with no record of it"),
        Some(r) if says_not_executed(r.state) => {
            assert_eq!(sent, 0, "{label}: sent, but recorded as {:?}", r.state);
        }
        // The journal works again, so the gateway settled what it could not record.
        Some(r) => assert!(
            !matches!(r.state, Dispatched | Accepted | TimedOut),
            "{label}: left in flight: {:?}",
            r.state
        ),
    }
    if let Some(r) = &record {
        let calls = psu.execute_calls();
        let again = engine.submit(&agent(), &set(1.0, Some("first"))).await;
        assert_eq!(psu.execute_calls(), calls, "{label}: same key sent again");
        assert_eq!(again.unwrap().command_id, r.command_id, "{label}");
        if unsettled(r.state) {
            let other = engine.submit(&agent(), &set(2.0, None)).await.unwrap();
            assert_eq!(
                psu.execute_calls(),
                calls,
                "{label}: a second write was sent while {} was {:?}",
                r.command_id,
                r.state
            );
            assert_eq!(codes(&other), [UNRESOLVED], "{label}: {other:?}");
        }
    }
}

/// After a restart: recovery sends nothing and leaves nothing in flight; an unknown
/// outcome holds the capability until reconciled, and reconciling tells the truth.
async fn restarted_gateway_settles(engine: &Engine, psu: &PsuHandle, label: &str, sent: u64) {
    let before = psu.execute_calls();
    engine.recover().await.unwrap();
    engine.recover().await.unwrap();
    assert_eq!(
        psu.execute_calls(),
        before,
        "{label}: recovery sent something"
    );
    evidence_complete(engine, label);

    let Some(r) = first(engine) else {
        assert_eq!(sent, 0, "{label}: sent with no record of it");
        return;
    };
    assert!(
        !matches!(
            r.state,
            Created | Validating | Approved | Dispatched | Accepted | TimedOut | Applied
        ),
        "{label}: still in flight after recovery: {:?}",
        r.state
    );
    if sent > 0 {
        assert!(
            !says_not_executed(r.state),
            "{label}: the device ran it, the record says {:?}",
            r.state
        );
    }
    if r.state == Verified {
        assert_eq!(sent, 1, "{label}: verified without reaching the device");
    }

    let calls = psu.execute_calls();
    let again = engine
        .submit(&agent(), &set(1.0, Some("first")))
        .await
        .unwrap();
    assert_eq!(again.command_id, r.command_id, "{label}");
    assert_eq!(psu.execute_calls(), calls, "{label}: same key sent again");

    if r.state == UnknownOutcome {
        let other = engine.submit(&agent(), &set(2.0, None)).await.unwrap();
        assert_eq!(codes(&other), [UNRESOLVED], "{label}: {other:?}");
        assert_eq!(psu.execute_calls(), calls, "{label}: sent while unresolved");
        let settled = engine.reconcile(&r.command_id).await.unwrap();
        let applied = psu.truth().setpoint_v == 1.0;
        assert_eq!(
            settled.state,
            if applied {
                ReconciledApplied
            } else {
                ReconciledNotApplied
            },
            "{label}"
        );
        assert_eq!(
            settled.state == ReconciledApplied,
            sent == 1,
            "{label}: reconciled as {:?} but sent {sent} times",
            settled.state
        );
    }
    tokio::time::sleep(Duration::from_secs(2)).await; // clear the slew window
    let next = engine.submit(&agent(), &set(2.0, None)).await.unwrap();
    assert_eq!(
        next.state, Verified,
        "{label}: no service after settling: {next:?}"
    );
}

async fn drill(site: &str, phase: FaultPhase, effect: FaultEffect) {
    let label = format!("{site} {phase:?} {effect:?}");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.sqlite");
    let clock = clock();
    let (device, psu) = device(&clock);

    let engine = gateway(&path, &clock, allow_all(), device.clone()).await;
    engine.journal().inject(Fault {
        site: site.into(),
        phase,
        effect,
    });
    let _ = engine.submit(&agent(), &set(1.0, Some("first"))).await;
    // Let an abandoned adapter call, if any, run to its end.
    tokio::time::sleep(Duration::from_secs(10)).await;
    let sent = psu.execute_calls();
    assert!(sent <= 1, "{label}: sent {sent} times");
    if effect == FaultEffect::IoError {
        same_gateway_carries_on(&engine, &psu, &label, sent).await;
    }
    drop(engine);

    let engine = gateway(&path, &clock, allow_all(), device).await;
    restarted_gateway_settles(&engine, &psu, &label, sent).await;
}

/// One test per boundary, phase and effect, so the report can list each case.
macro_rules! drills {
    ($($module:ident: $site:literal;)*) => {$(
        mod $module {
            use super::*;
            #[tokio::test(start_paused = true)]
            async fn io_error_before_commit() {
                drill($site, FaultPhase::BeforeCommit, FaultEffect::IoError).await;
            }
            #[tokio::test(start_paused = true)]
            async fn io_error_after_commit() {
                drill($site, FaultPhase::AfterCommit, FaultEffect::IoError).await;
            }
            #[tokio::test(start_paused = true)]
            async fn crash_before_commit() {
                drill($site, FaultPhase::BeforeCommit, FaultEffect::Crash).await;
            }
            #[tokio::test(start_paused = true)]
            async fn crash_after_commit() {
                drill($site, FaultPhase::AfterCommit, FaultEffect::Crash).await;
            }
        }
    )*
        #[test]
        fn every_command_boundary_is_drilled() {
            assert_eq!([$($site),*], COMMAND_SITES);
        }
    };
}

drills! {
    created: "command.created";
    validating: "transition:VALIDATING";
    approved: "transition:APPROVED";
    expectation: "expectation";
    dispatched: "transition:DISPATCHED";
    accepted: "transition:ACCEPTED";
    applied: "transition:APPLIED";
    verified: "transition:VERIFIED";
}

/// Recovery itself is interrupted while settling an in-flight command, then runs again.
#[tokio::test(start_paused = true)]
async fn a_crash_during_recovery_is_recovered_by_the_next_start() {
    for phase in [FaultPhase::BeforeCommit, FaultPhase::AfterCommit] {
        let label = format!("recovery {phase:?}");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.sqlite");
        let clock = clock();
        let (device, psu) = device(&clock);

        let engine = gateway(&path, &clock, allow_all(), device.clone()).await;
        engine.journal().inject(Fault {
            site: "transition:APPLIED".into(),
            phase: FaultPhase::BeforeCommit,
            effect: FaultEffect::Crash,
        });
        let _ = engine.submit(&agent(), &set(1.0, Some("first"))).await;
        tokio::time::sleep(Duration::from_secs(10)).await;
        drop(engine);
        assert_eq!(psu.execute_calls(), 1);

        let engine = gateway(&path, &clock, allow_all(), device.clone()).await;
        engine.journal().inject(Fault {
            site: "transition:UNKNOWN_OUTCOME".into(),
            phase,
            effect: FaultEffect::Crash,
        });
        assert!(engine.recover().await.is_err(), "{label}");
        drop(engine);

        let engine = gateway(&path, &clock, allow_all(), device).await;
        restarted_gateway_settles(&engine, &psu, &label, 1).await;
    }
}

/// Spending an approval and moving its command are one transaction: a crash on either
/// side of the commit leaves the approval spendable once, or spent with a command that
/// was never sent. The device sees the command at most once either way.
#[tokio::test(start_paused = true)]
async fn a_crash_while_spending_an_approval_never_makes_it_reusable() {
    for phase in [FaultPhase::BeforeCommit, FaultPhase::AfterCommit] {
        let label = format!("approval.consume {phase:?}");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.sqlite");
        let clock = clock();
        let (device, psu) = device(&clock);
        let mut policy = allow_all();
        policy.grants.insert(
            0,
            grant(
                "ask",
                SETPOINT,
                &[Operation::Write],
                Effect::RequireApproval,
            ),
        );
        let operator = Caller::in_process(OPERATOR);

        let engine = gateway(&path, &clock, policy.clone(), device.clone()).await;
        let held = engine
            .submit(&agent(), &set(1.0, Some("first")))
            .await
            .unwrap();
        assert_eq!(held.state, AwaitingApproval, "{label}");
        engine.journal().inject(Fault {
            site: "approval.consume".into(),
            phase,
            effect: FaultEffect::Crash,
        });
        assert!(engine.approve(&operator, &held.command_id).await.is_err());
        drop(engine);
        assert_eq!(psu.execute_calls(), 0, "{label}");

        let engine = gateway(&path, &clock, policy, device).await;
        engine.recover().await.unwrap();
        let r = engine.receipt(&held.command_id).unwrap().unwrap();
        let again = engine.approve(&operator, &held.command_id).await;
        match phase {
            // Rolled back: still waiting, and one approval runs it once.
            FaultPhase::BeforeCommit => {
                assert_eq!(r.state, AwaitingApproval, "{label}");
                assert_eq!(again.unwrap().state, Verified, "{label}");
                assert_eq!(psu.execute_calls(), 1, "{label}");
            }
            // Spent, never sent: recovery expires it, and it cannot be approved again.
            FaultPhase::AfterCommit => {
                assert_eq!(r.state, Expired, "{label}");
                assert!(again.is_err(), "{label}: {again:?}");
                assert_eq!(psu.execute_calls(), 0, "{label}");
            }
        }
        assert!(engine.approve(&operator, &held.command_id).await.is_err());
        evidence_complete(&engine, &label);
    }
}

/// SQLite's own disk-full error, at every point a command can run out of journal: the
/// limit is swept a page at a time so that the failure lands on each write in turn.
#[tokio::test(start_paused = true)]
async fn a_full_journal_mid_command_holds_the_capability_until_recovery() {
    let mut stranded = 0;
    let mut violations = Vec::new();
    for slack_pages in 1..=64u64 {
        let label = format!("disk full after {slack_pages} pages");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.sqlite");
        let clock = clock();
        let (device, psu) = device(&clock);
        let engine = gateway(&path, &clock, allow_all(), device.clone()).await;
        let used = engine.journal().size_bytes().unwrap();
        engine
            .set_storage_limits(StorageLimits {
                max_bytes: used + slack_pages * 4096,
                reserve_bytes: 0,
            })
            .unwrap();

        let mut failed = None;
        for i in 0..400u64 {
            let v = if i % 2 == 0 { 1.0 } else { 2.0 };
            // Records of varying size, so that the journal runs out at a different write in
            // each sweep.
            let mut request = set(v, None);
            request["correlation_id"] =
                json!("c".repeat(1 + ((slack_pages * 37 + i * 11) % 120) as usize));
            if let Err(e) = engine.submit(&agent(), &request).await {
                failed = Some(e);
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let failed = failed.unwrap_or_else(|| panic!("{label}: never ran out"));
        let last = engine.commands(None, &[], 1).unwrap();
        println!(
            "{label}: {failed}; last command {:?}",
            last.first().map(|r| (r.state, r.transitions.len()))
        );
        tokio::time::sleep(Duration::from_secs(10)).await;

        // Every call that reached the device has its dispatch on record.
        let all = engine.commands(None, &[], 1000).unwrap();
        let dispatched = all
            .iter()
            .filter(|r| r.transitions.iter().any(|t| t.state == Dispatched))
            .count() as u64;
        assert!(psu.execute_calls() <= dispatched, "{label}");

        // An operator makes room without restarting: an open outcome still holds.
        let open: Vec<&Receipt> = all.iter().filter(|r| unsettled(r.state)).collect();
        engine
            .set_storage_limits(StorageLimits {
                max_bytes: used + 64 * 1024 * 1024,
                reserve_bytes: 0,
            })
            .unwrap();
        if let Some(stuck) = open.first() {
            stranded += 1;
            let calls = psu.execute_calls();
            let next = engine.submit(&agent(), &set(1.5, None)).await.unwrap();
            if psu.execute_calls() != calls || codes(&next) != [UNRESOLVED] {
                violations.push(format!(
                    "{label}: {} left {:?}; the next write was sent and ended {:?}",
                    stuck.command_id, stuck.state, next.state
                ));
            }
        }
        drop(engine);

        let engine = gateway(&path, &clock, allow_all(), device).await;
        let calls = psu.execute_calls();
        engine.recover().await.unwrap();
        assert_eq!(
            psu.execute_calls(),
            calls,
            "{label}: recovery sent something"
        );
        let in_flight = engine
            .commands(None, &[Dispatched, Accepted, TimedOut, Applied], 1000)
            .unwrap();
        assert!(in_flight.is_empty(), "{label}: {in_flight:?}");
        evidence_complete(&engine, &label);
    }
    assert!(
        stranded > 0,
        "no sweep left a dispatched command open: the test proves nothing"
    );
    assert!(
        violations.is_empty(),
        "{stranded} stranded:\n{}",
        violations.join("\n")
    );
}

// ---------------------------------------------------------------- a real process death

/// The simulated supply, counting `execute` calls in a file so that they survive the
/// process that made them.
struct Marked {
    inner: NotIdempotent,
    marks: std::path::PathBuf,
}

#[async_trait]
impl HardwareAdapter for Marked {
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
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.marks)
            .unwrap();
        f.write_all(b"x").unwrap();
        f.sync_all().unwrap();
        self.inner.execute(c, p).await
    }
    async fn shutdown(&self) -> Result<(), AdapterError> {
        self.inner.shutdown().await
    }
}

fn marked(clock: &Arc<dyn Clock>, dir: &Path) -> (Arc<Marked>, PsuHandle) {
    let (psu, handle) = VirtualPsu::new(clock.clone(), 42);
    (
        Arc::new(Marked {
            inner: NotIdempotent {
                inner: psu,
                capability: SETPOINT,
            },
            marks: dir.join("executions"),
        }),
        handle,
    )
}

const CHILD: &str = "OPENMHS_ABORT_DRILL";

/// Runs only as the child of the test below: arms an abort and submits.
#[tokio::test(start_paused = true)]
async fn abort_drill_child() {
    let Ok(spec) = std::env::var(CHILD) else {
        return;
    };
    let [dir, site, phase]: [&str; 3] = spec.split('|').collect::<Vec<_>>().try_into().unwrap();
    let dir = Path::new(dir);
    let clock = clock();
    let (device, _psu) = marked(&clock, dir);
    let engine = gateway(&dir.join("journal.sqlite"), &clock, allow_all(), device).await;
    engine.journal().inject(Fault {
        site: site.into(),
        phase: if phase == "before" {
            FaultPhase::BeforeCommit
        } else {
            FaultPhase::AfterCommit
        },
        effect: FaultEffect::Abort,
    });
    let _ = engine.submit(&agent(), &set(1.0, Some("first"))).await;
    unreachable!("the fault at {site} did not fire");
}

#[tokio::test(start_paused = true)]
async fn a_process_aborted_at_a_boundary_recovers_without_resending() {
    for (site, phase, expected) in [
        ("command.created", "before", None),
        ("transition:DISPATCHED", "before", Some(Expired)),
        ("transition:DISPATCHED", "after", Some(UnknownOutcome)),
        ("transition:ACCEPTED", "before", Some(UnknownOutcome)),
        ("transition:ACCEPTED", "after", Some(UnknownOutcome)),
        ("transition:APPLIED", "before", Some(UnknownOutcome)),
    ] {
        let label = format!("abort at {site} {phase}");
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["abort_drill_child", "--exact", "--test-threads=1"])
            .env(CHILD, format!("{}|{site}|{phase}", dir.path().display()))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(!status.success(), "{label}: the child did not die");
        assert_ne!(
            status.code(),
            Some(101),
            "{label}: the child panicked instead"
        );
        let sent = std::fs::read(dir.path().join("executions"))
            .map(|m| m.len() as u64)
            .unwrap_or(0);

        let clock = clock();
        let (device, _psu) = marked(&clock, dir.path());
        let engine = gateway(
            &dir.path().join("journal.sqlite"),
            &clock,
            allow_all(),
            device,
        )
        .await;
        engine.recover().await.unwrap();
        engine.recover().await.unwrap();
        let after = std::fs::read(dir.path().join("executions"))
            .map(|m| m.len() as u64)
            .unwrap_or(0);
        assert_eq!(after, sent, "{label}: recovery sent something");
        let r = first(&engine);
        assert_eq!(r.as_ref().map(|r| r.state), expected, "{label}");
        if sent > 0 {
            assert!(!says_not_executed(r.unwrap().state), "{label}");
        }
        let again = engine.submit(&agent(), &set(1.0, Some("first"))).await;
        let resent = std::fs::read(dir.path().join("executions"))
            .map(|m| m.len() as u64)
            .unwrap_or(0);
        if expected.is_some() {
            assert_eq!(resent, sent, "{label}: same key sent again: {again:?}");
        }
        evidence_complete(&engine, &label);
    }
}

// ---------------------------------------------------------------- the journal's own limits

/// Without a checkpoint file the gateway trusts the journal it is given: a journal rolled
/// back to an earlier copy (a restored backup, a lost write-ahead log, a disk that
/// acknowledged writes it never made) is consistent, opened without complaint, and
/// believed. Only a checkpoint kept somewhere else shows that records are missing. With
/// `--checkpoint-file` it is refused at start: `persistence_storage.rs`.
#[tokio::test(start_paused = true)]
async fn a_rolled_back_journal_is_only_detected_against_an_external_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let live = dir.path().join("live.sqlite");
    let backup = dir.path().join("backup.sqlite");
    let no_wal = dir.path().join("no-wal.sqlite");
    let clock = clock();
    let (device, psu) = device(&clock);

    let engine = gateway(&live, &clock, allow_all(), device.clone()).await;
    // A consistent backup taken before the command.
    engine
        .journal()
        .execute_raw(&format!("VACUUM INTO '{}'", backup.display()))
        .unwrap();
    let receipt = engine
        .submit(&agent(), &set(1.0, Some("first")))
        .await
        .unwrap();
    assert_eq!(receipt.state, Verified);
    // The main file alone, while its write-ahead log still holds the recent commits: what
    // is left if the log is lost.
    std::fs::copy(&live, &no_wal).unwrap();
    let checkpoint = engine.journal().export_evidence().unwrap().checkpoint;
    drop(engine);
    assert_eq!(psu.execute_calls(), 1);

    for stale in [&backup, &no_wal] {
        let label = stale.file_name().unwrap().to_string_lossy().into_owned();
        let engine = gateway(stale, &clock, allow_all(), device.clone()).await;
        engine.recover().await.unwrap();
        assert!(
            first(&engine).is_none(),
            "{label}: the command should be gone"
        );
        assert_eq!(psu.execute_calls(), 1, "{label}: recovery sent something");

        // On its own the stale journal looks whole...
        let bundle = engine.journal().export_evidence().unwrap();
        let alone = verify_evidence(&bundle, None);
        assert!(alone.valid && alone.complete, "{label}: {alone:?}");
        // ...against the checkpoint kept elsewhere it does not.
        let against = verify_evidence(&bundle, Some(&checkpoint));
        assert!(!against.valid, "{label}: {against:?}");
    }
}
