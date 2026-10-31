//! Persistence adversary, part two: the journal's files themselves. Power lost in the
//! middle of a write, bytes damaged on disk, a journal that cannot be written, a stored
//! value edited behind the gateway's back, and a journal rolled back to an older copy.
//!
//! The property is the same as in `persistence_adversary.rs`: whatever the files say after
//! the fault, the gateway either refuses to use them or uses them without sending anything
//! twice and without calling something that may have run "not executed".
//!
//! The report is docs/architecture/PERSISTENCE_ADVERSARY.md.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use openmhs_adapter::HardwareAdapter;
use openmhs_engine::journal::{FaultPhase, Journal};
use openmhs_engine::{Engine, EngineError, EngineOptions};
use openmhs_policy::{Caller, Policy};
use openmhs_sim::{ExecFault, PsuHandle, VirtualPsu};
use openmhs_types::evidence::{AuditCheckpoint, verify_evidence};
use openmhs_types::{Clock, CommandState, Receipt};
use serde_json::{Value as Json, json};

use CommandState::*;

fn agent() -> Caller {
    Caller::in_process(AGENT)
}

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

fn engine_on(journal: Journal, clock: &Arc<dyn Clock>, policy: Policy) -> Engine {
    Engine::with_journal(journal, clock.clone(), policy, EngineOptions::default())
}

async fn gateway(path: &Path, clock: &Arc<dyn Clock>, device: Arc<dyn HardwareAdapter>) -> Engine {
    let engine = engine_on(Journal::open(path).unwrap(), clock, allow_all());
    engine.register(device).await.unwrap();
    engine
}

fn set(v: f64, key: &str) -> Json {
    let mut r = write(SETPOINT, volts(v));
    r["idempotency_key"] = json!(key);
    r
}

fn wal(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", path.display()))
}

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

fn code(e: &EngineError) -> String {
    match e {
        EngineError::Denied(e) => e.code.clone(),
        other => other.code().to_owned(),
    }
}

// ---------------------------------------------------------------- power lost mid-write

/// The journal's files as they stood after one commit.
struct Snapshot {
    /// The durable write that had just committed.
    site: String,
    /// Device calls made before the *next* write began: what had happened in the world by
    /// the time power was lost during it.
    calls_before_next: u64,
    db: Vec<u8>,
    wal: Vec<u8>,
}

/// Power is lost while a write is on its way to disk, for every write a command makes:
/// the log holds a torn, partial transaction at its end. Storage is assumed to honour its
/// syncs (`synchronous=FULL`); a disk that does not is the rollback case below. SQLite must
/// come back at the last complete commit, and recovery must be safe from there.
#[tokio::test(start_paused = true)]
async fn power_lost_mid_write_comes_back_at_the_last_commit_and_recovers_safely() {
    let dir = tempfile::tempdir().unwrap();
    let live = dir.path().join("live.sqlite");
    let clock = clock();
    let (device, psu) = device(&clock);
    let engine = gateway(&live, &clock, device.clone()).await;

    let snapshots: Arc<Mutex<Vec<Snapshot>>> = Arc::default();
    let take = |site: &str| Snapshot {
        site: site.to_owned(),
        calls_before_next: 0,
        db: std::fs::read(&live).unwrap(),
        wal: std::fs::read(wal(&live)).unwrap_or_default(),
    };
    snapshots.lock().unwrap().push(take("start"));
    {
        let (snapshots, live, psu) = (snapshots.clone(), live.clone(), psu.clone());
        engine
            .journal()
            .observe_commits(Box::new(move |site, phase| {
                let mut s = snapshots.lock().unwrap();
                match phase {
                    FaultPhase::BeforeCommit => {
                        if let Some(last) = s.last_mut() {
                            last.calls_before_next = psu.execute_calls();
                        }
                    }
                    FaultPhase::AfterCommit => s.push(Snapshot {
                        site: site.to_owned(),
                        calls_before_next: 0,
                        db: std::fs::read(&live).unwrap(),
                        wal: std::fs::read(wal(&live)).unwrap_or_default(),
                    }),
                }
            }));
    }
    let done = engine.submit(&agent(), &set(1.0, "first")).await.unwrap();
    assert_eq!(done.state, Verified);
    let snapshots = std::mem::take(&mut *snapshots.lock().unwrap());
    drop(engine);

    // The command's state after each snapshot's commit.
    let mut states: Vec<Option<CommandState>> = Vec::new();
    for s in &snapshots {
        let previous = states.last().copied().flatten();
        states.push(if s.site == "command.created" {
            Some(Created)
        } else if let Some(state) = s.site.strip_prefix("transition:") {
            Some(serde_json::from_value(json!(state)).unwrap())
        } else {
            previous
        });
    }

    let mut cases = 0;
    for (k, pair) in snapshots.windows(2).enumerate() {
        let (before, after) = (&pair[0], &pair[1]);
        // Only a write that appended to the same log can be torn this way.
        if before.db != after.db || !after.wal.starts_with(&before.wal) {
            continue;
        }
        let (from, to) = (before.wal.len(), after.wal.len());
        let mut cuts = vec![from, from + 1, from + (to - from) / 2, to - 1, to];
        cuts.sort_unstable();
        cuts.dedup();
        for cut in cuts.into_iter().filter(|c| (from..=to).contains(c)) {
            cases += 1;
            let label = format!("torn {} at {cut} of {from}..{to}", after.site);
            let case = tempfile::tempdir().unwrap();
            let path = case.path().join("journal.sqlite");
            std::fs::write(&path, &after.db).unwrap();
            std::fs::write(wal(&path), &after.wal[..cut]).unwrap();

            let journal = Journal::open(&path).unwrap_or_else(|e| panic!("{label}: {e}"));
            let engine = engine_on(journal, &clock, allow_all());
            engine.register(device.clone()).await.unwrap();
            let expected = if cut == to { states[k + 1] } else { states[k] };
            let record = engine.receipt_by_key(AGENT, PSU, "first").unwrap();
            assert_eq!(record.as_ref().map(|r| r.state), expected, "{label}");

            let calls = psu.execute_calls();
            engine.recover().await.unwrap();
            assert_eq!(
                psu.execute_calls(),
                calls,
                "{label}: recovery sent something"
            );
            if let Some(r) = engine.receipt_by_key(AGENT, PSU, "first").unwrap() {
                assert!(
                    !matches!(
                        r.state,
                        Created
                            | Validating
                            | Approved
                            | Dispatched
                            | Accepted
                            | TimedOut
                            | Applied
                    ),
                    "{label}: in flight after recovery: {:?}",
                    r.state
                );
                if before.calls_before_next > 0 && cut < to {
                    assert!(
                        !says_not_executed(r.state),
                        "{label}: ran, recorded {:?}",
                        r.state
                    );
                }
            } else {
                assert_eq!(before.calls_before_next, 0, "{label}: ran, no record");
            }
            let report = verify_evidence(&engine.journal().export_evidence().unwrap(), None);
            assert!(report.valid && report.complete, "{label}: {report:?}");
        }
    }
    println!("{cases} torn writes over {} commits", snapshots.len() - 1);
    assert!(
        cases >= 20,
        "only {cases} torn writes: the test proves little"
    );
}

// ---------------------------------------------------------------- damaged bytes

/// One bit flipped at a time across the whole database file. Either the gateway refuses
/// the journal at start (`journal.corrupt`, `journal.audit_invalid`, or SQLite cannot read
/// it), or every record it reads back is exactly what was written, or the read fails.
/// Never a silently different receipt, expectation or audit record, and never a resend.
#[tokio::test(start_paused = true)]
async fn a_damaged_byte_is_refused_or_detected_never_silently_used() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.sqlite");
    let clock = clock();
    let (device, psu) = device(&clock);
    {
        let engine = gateway(&path, &clock, device.clone()).await;
        assert_eq!(
            engine
                .submit(&agent(), &set(1.0, "first"))
                .await
                .unwrap()
                .state,
            Verified
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
        psu.update_faults(|f| f.execute = Some(ExecFault::Hang));
        let mut uncertain = set(2.0, "second");
        uncertain["deadline_ms"] = json!(2_000);
        assert_eq!(
            engine.submit(&agent(), &uncertain).await.unwrap().state,
            UnknownOutcome
        );
        psu.clear_faults();
    }
    // Closing the last connection folds the log into the file.
    let original = Journal::open(&path).unwrap();
    let keys = ["first", "second"];
    let mut receipts: BTreeMap<&str, Receipt> = BTreeMap::new();
    let mut expectations = BTreeMap::new();
    for key in keys {
        let r = original.command_by_key(AGENT, PSU, key).unwrap().unwrap();
        expectations.insert(
            key,
            format!("{:?}", original.expectation(&r.command_id).unwrap()),
        );
        receipts.insert(key, r);
    }
    let bundle = original.export_evidence().unwrap();
    drop(original);
    let base = std::fs::read(&path).unwrap();
    assert!(!wal(&path).exists() || std::fs::read(wal(&path)).unwrap().is_empty());

    let stride = (base.len() / 700).max(1);
    let (mut refused, mut unchanged, mut detected) = (0, 0, 0);
    let mut violations = Vec::new();
    for offset in (0..base.len()).step_by(stride) {
        let mut damaged = base.clone();
        damaged[offset] ^= 1 << (offset % 8);
        let case = tempfile::tempdir().unwrap();
        let p = case.path().join("journal.sqlite");
        std::fs::write(&p, &damaged).unwrap();
        let label = format!("bit {} of byte {offset}", offset % 8);

        let Ok(journal) = Journal::open(&p) else {
            refused += 1;
            continue;
        };
        let mut silently_changed = Vec::new();
        let mut caught = false;
        for key in keys {
            let id = &receipts[key].command_id;
            match journal.receipt(id) {
                Ok(Some(r)) if r == receipts[key] => {}
                Ok(other) => silently_changed.push(format!("receipt {key}: {other:?}")),
                Err(_) => caught = true,
            }
            match journal.expectation(id) {
                Ok(e) if format!("{e:?}") == expectations[key] => {}
                Ok(other) => silently_changed.push(format!("expectation {key}: {other:?}")),
                Err(_) => caught = true,
            }
            match journal.command_by_key(AGENT, PSU, key) {
                Ok(Some(r)) if r == receipts[key] => {}
                Ok(other) => silently_changed.push(format!("lookup by key {key}: {other:?}")),
                Err(_) => caught = true,
            }
        }
        match journal.export_evidence() {
            Ok(b) if b == bundle => {}
            Ok(b) => {
                if verify_evidence(&b, None).valid {
                    silently_changed.push("evidence differs but verifies".into());
                } else {
                    caught = true;
                }
            }
            Err(_) => caught = true,
        }
        // Resubmitting a stored command must never run it again.
        let (fresh, fresh_psu) = device_like(&clock);
        let engine = engine_on(journal, &clock, allow_all());
        if engine.register(fresh).await.is_ok() {
            for key in keys {
                let _ = engine.submit(&agent(), &set(1.0, key)).await;
            }
            if fresh_psu.execute_calls() > 0 {
                silently_changed.push("a stored command was sent again".into());
            }
        }
        if !silently_changed.is_empty() {
            violations.push(format!("{label}: {}", silently_changed.join("; ")));
        } else if caught {
            detected += 1;
        } else {
            unchanged += 1;
        }
    }
    println!(
        "{} flips: {refused} refused at start, {detected} detected on use, {unchanged} without effect",
        refused + detected + unchanged + violations.len()
    );
    assert!(
        violations.is_empty(),
        "{} silent changes:\n{}",
        violations.len(),
        violations.join("\n")
    );
    assert!(refused > 0, "no flip was refused: the test proves little");
}

/// A second simulated supply, so that sends during a corruption case are counted apart.
fn device_like(clock: &Arc<dyn Clock>) -> (Arc<NotIdempotent>, PsuHandle) {
    device(clock)
}

// ---------------------------------------------------------------- an unwritable journal

/// The journal file is read-only (a remounted filesystem, lost permissions). The gateway
/// either refuses to open it or refuses every command; nothing reaches a device.
#[tokio::test(start_paused = true)]
async fn a_read_only_journal_sends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.sqlite");
    let clock = clock();
    let (device, psu) = device(&clock);
    {
        let engine = gateway(&path, &clock, device.clone()).await;
        assert_eq!(
            engine
                .submit(&agent(), &set(1.0, "first"))
                .await
                .unwrap()
                .state,
            Verified
        );
    }
    let calls = psu.execute_calls();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions.clone()).unwrap();

    let outcome = match Journal::open(&path) {
        Err(e) => format!("refused at start: {e}"),
        Ok(journal) => {
            let engine = engine_on(journal, &clock, allow_all());
            let _ = engine.register(device.clone()).await;
            tokio::time::sleep(Duration::from_secs(2)).await;
            let r = engine.submit(&agent(), &set(2.0, "second")).await;
            assert!(
                r.as_ref().is_err() || r.as_ref().unwrap().state == Denied,
                "{r:?}"
            );
            "every command refused".to_owned()
        }
    };
    assert_eq!(psu.execute_calls(), calls, "{outcome}: something was sent");

    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&path, permissions).unwrap();
}

// ---------------------------------------------------------------- edited behind its back

/// The readback target a command will be reconciled against is bound to the audit chain
/// by its `DISPATCHED` record. Changing it in the database is refused, never used: a
/// changed target could turn "applied" into "not applied", which invites a resend.
#[tokio::test(start_paused = true)]
async fn an_edited_expectation_is_refused_not_used_to_reconcile() {
    let (engine, psu) = setup_not_idempotent(allow_all(), SETPOINT).await;
    psu.update_faults(|f| f.execute = Some(ExecFault::ApplyThenHang));
    let mut req = set(1.0, "first");
    req["deadline_ms"] = json!(2_000);
    let r = engine.submit(&agent(), &req).await.unwrap();
    assert_eq!(r.state, UnknownOutcome);
    psu.clear_faults();

    let dispatched = engine
        .journal()
        .audit_records()
        .unwrap()
        .into_iter()
        .find(|a| a.subject == r.command_id && a.data.contains("\"to\":\"DISPATCHED\""))
        .unwrap();
    assert!(
        dispatched.data.contains("expectation_sha256"),
        "{}",
        dispatched.data
    );

    engine
        .journal()
        .execute_raw("UPDATE commands SET expectation = replace(expectation, '1.0', '2.0')")
        .unwrap();
    let reconciled = engine.reconcile(&r.command_id).await;
    assert!(
        matches!(&reconciled, Err(e) if code(e) == "journal.receipt_integrity"),
        "{reconciled:?}"
    );
    assert_eq!(
        engine.receipt(&r.command_id).unwrap().unwrap().state,
        UnknownOutcome
    );
}

/// An edited audit record makes the journal unusable at the next start; a journal the
/// gateway cannot trust is not extended.
#[tokio::test(start_paused = true)]
async fn a_broken_audit_chain_is_refused_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.sqlite");
    let clock = clock();
    let (device, _psu) = device(&clock);
    {
        let engine = gateway(&path, &clock, device).await;
        engine.submit(&agent(), &set(1.0, "first")).await.unwrap();
        engine
            .journal()
            .execute_raw("UPDATE audit SET data = data || ' ' WHERE seq = 3")
            .unwrap();
    }
    let opened = Journal::open(&path);
    assert!(
        matches!(&opened, Err(openmhs_engine::JournalError::Refused(e)) if e.code == "journal.audit_invalid"),
        "{:?}",
        opened.err()
    );
}

/// Recovery and the capability holds find unfinished commands through the `state` index.
/// A command whose stored state was edited to look finished (or finished, to look open)
/// is refused at start: the index must agree with the hash-chained lifecycle records.
/// Otherwise an unknown outcome would stop holding its capability, and the next write
/// would be sent.
#[tokio::test(start_paused = true)]
async fn a_state_edited_to_look_finished_is_refused_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.sqlite");
    let clock = clock();
    let (device, psu) = device(&clock);
    let (done, open) = {
        let engine = gateway(&path, &clock, device.clone()).await;
        let done = engine.submit(&agent(), &set(1.0, "first")).await.unwrap();
        assert_eq!(done.state, Verified);
        tokio::time::sleep(Duration::from_secs(2)).await;
        psu.update_faults(|f| f.execute = Some(ExecFault::Hang));
        let mut uncertain = set(2.0, "second");
        uncertain["deadline_ms"] = json!(2_000);
        let open = engine.submit(&agent(), &uncertain).await.unwrap();
        assert_eq!(open.state, UnknownOutcome);
        psu.clear_faults();
        (done.command_id, open.command_id)
    };
    let edits = [
        (
            "state column only",
            format!("UPDATE commands SET state = 'RECONCILED_APPLIED' WHERE command_id = '{open}'"),
        ),
        (
            "state column and receipt",
            format!(
                "UPDATE commands SET state = 'RECONCILED_APPLIED', \
                 receipt = json_set(receipt, '$.state', 'RECONCILED_APPLIED') \
                 WHERE command_id = '{open}'"
            ),
        ),
        (
            "a finished command made to look in flight",
            format!(
                "UPDATE commands SET state = 'DISPATCHED', \
                 receipt = json_set(receipt, '$.state', 'DISPATCHED') WHERE command_id = '{done}'"
            ),
        ),
    ];
    for (what, sql) in edits {
        let case = dir.path().join("case.sqlite");
        std::fs::copy(&path, &case).unwrap();
        Journal::open(&case).unwrap().execute_raw(&sql).unwrap();
        let opened = Journal::open(&case);
        assert!(
            matches!(&opened, Err(openmhs_engine::JournalError::Refused(e)) if e.code == "journal.receipt_integrity"),
            "{what}: {:?}",
            opened.err()
        );
        std::fs::remove_file(&case).unwrap();
    }

    // Unedited, it opens, and the unknown outcome still holds its capability.
    let engine = gateway(&path, &clock, device).await;
    engine.recover().await.unwrap();
    let calls = psu.execute_calls();
    let r = engine.submit(&agent(), &set(3.0, "third")).await;
    assert_eq!(
        psu.execute_calls(),
        calls,
        "sent past an unknown outcome: {r:?}"
    );
}

// ---------------------------------------------------------------- a rolled-back journal

/// With a checkpoint file kept outside the journal, a journal rolled back to before a
/// dispatch is refused at start instead of being believed: the gap that the rollback test
/// in `persistence_adversary.rs` pins without one.
#[tokio::test(start_paused = true)]
async fn a_checkpoint_file_catches_a_rolled_back_journal_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let live = dir.path().join("live.sqlite");
    let backup = dir.path().join("backup.sqlite");
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let checkpoint = elsewhere.join("audit-checkpoint.json");
    let clock = clock();
    let (device, psu) = device(&clock);

    let engine = gateway(&live, &clock, device.clone()).await;
    engine.set_checkpoint_file(&checkpoint).unwrap();
    engine
        .journal()
        .execute_raw(&format!("VACUUM INTO '{}'", backup.display()))
        .unwrap();
    let r = engine.submit(&agent(), &set(1.0, "first")).await.unwrap();
    assert_eq!(r.state, Verified);
    // The file names at least the dispatch, written before the device was called.
    let saved: AuditCheckpoint =
        serde_json::from_slice(&std::fs::read(&checkpoint).unwrap()).unwrap();
    let dispatch_seq = engine
        .journal()
        .audit_records()
        .unwrap()
        .into_iter()
        .find(|a| a.subject == r.command_id && a.data.contains("\"to\":\"DISPATCHED\""))
        .unwrap()
        .seq;
    assert!(saved.seq >= dispatch_seq, "{saved:?} < {dispatch_seq}");
    drop(engine);

    // The old copy is refused...
    let stale = engine_on(Journal::open(&backup).unwrap(), &clock, allow_all());
    let refused = stale.set_checkpoint_file(&checkpoint);
    assert!(
        matches!(&refused, Err(e) if code(e) == "journal.behind_checkpoint"),
        "{refused:?}"
    );
    drop(stale);
    // ...the current journal is not.
    let engine = gateway(&live, &clock, device.clone()).await;
    engine.set_checkpoint_file(&checkpoint).unwrap();
    engine.recover().await.unwrap();

    // If the checkpoint cannot be written, nothing is sent, and the record says so.
    std::fs::remove_dir_all(&elsewhere).unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let calls = psu.execute_calls();
    let r = engine.submit(&agent(), &set(2.0, "second")).await.unwrap();
    assert_eq!(psu.execute_calls(), calls, "sent without its checkpoint");
    assert_eq!(r.state, Failed, "{r:?}");
    assert_eq!(
        r.adapter_result.unwrap().error.unwrap().code,
        "adapter.not_sent"
    );
}
