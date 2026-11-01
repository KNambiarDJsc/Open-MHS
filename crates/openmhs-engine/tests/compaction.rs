//! Compaction (RFC 0002, milestone 5): finished history leaves the journal for its sealed
//! segments, nothing live is touched, every command stays answerable, and the archival
//! drills of the RFC all hold.
#![allow(clippy::unwrap_used)]

mod fixture;

use std::collections::BTreeMap;
use std::path::Path;

use fixture::*;
use openmhs_engine::journal::{
    Fault, FaultEffect, FaultPhase, Inserted, Journal, SegmentLimits, SegmentState,
};
use openmhs_types::evidence::{AuditCheckpoint, verify_evidence};
use openmhs_types::{CommandState, Receipt, Timestamp};
use serde_json::json;

fn ts(ms: i64) -> Timestamp {
    Timestamp::from_millis(ms)
}

fn through(seq: i64) -> SegmentLimits {
    SegmentLimits {
        max_bytes: u64::MAX,
        through_seq: Some(seq),
    }
}

/// Write, seal and compact the next segment up to the current head.
fn archive_next(j: &Journal, at: i64) -> openmhs_engine::journal::Compaction {
    let row = j.create_segment(&through(i64::MAX)).unwrap();
    j.seal_segment(row.number, ts(at)).unwrap();
    j.compact_segment(row.number, ts(at + 1), false).unwrap()
}

/// Run whatever steps a segment still needs to reach `COMPACTED`.
fn finish_archive(j: &Journal, at: i64) {
    loop {
        let rows = j.segments().unwrap();
        let Some(row) = rows.last() else {
            archive_next(j, at);
            continue;
        };
        match row.state {
            SegmentState::Compacted => return,
            SegmentState::Planned | SegmentState::Written => {
                j.create_segment(&through(i64::MAX)).unwrap();
            }
            SegmentState::Verified => {
                j.seal_segment(row.number, ts(at)).unwrap();
            }
            SegmentState::Sealed => {
                j.compact_segment(row.number, ts(at + 1), false).unwrap();
            }
        }
    }
}

struct Before {
    receipts: BTreeMap<&'static str, Receipt>,
}

/// Two finished commands (one keyed), one held, a lot of routine records; the first
/// segment is built but not yet sealed or compacted.
fn populated(dir: &Path) -> (Journal, Before) {
    let j = open(dir);
    finished(&j, "cmd-a", Some("key-a"), 10);
    finished(&j, "cmd-b", None, 20);
    held(&j, "cmd-live", Some("key-live"), 30);
    noise(&j, 20, 40);
    let receipts = ["cmd-a", "cmd-b", "cmd-live"]
        .into_iter()
        .map(|id| (id, j.receipt(id).unwrap().unwrap()))
        .collect();
    (j, Before { receipts })
}

fn compacted(dir: &Path) -> (Journal, Before) {
    let (j, before) = populated(dir);
    archive_next(&j, 100);
    (j, before)
}

fn subjects(j: &Journal) -> Vec<String> {
    let mut s: Vec<String> = j
        .audit_records()
        .unwrap()
        .into_iter()
        .map(|r| r.subject)
        .collect();
    s.sort();
    s.dedup();
    s
}

#[test]
fn compaction_moves_finished_history_and_keeps_what_is_live() {
    let dir = tempfile::tempdir().unwrap();
    let (j, before) = populated(dir.path());
    let export_before = j.export_evidence().unwrap();
    let all_rows = j.audit_records().unwrap().len();
    // Live work of every kind, and a finished command a live plan still links to.
    finished(&j, "cmd-linked", None, 60);
    for sql in [
        "INSERT INTO approvals VALUES ('appr-open','PENDING','agent.test',NULL,NULL,1,2,'{}')",
        "INSERT INTO approvals VALUES ('appr-done','CONSUMED','agent.test',NULL,NULL,1,2,'{}')",
        "INSERT INTO leases VALUES ('lease-open','agent.test',NULL,1,2,NULL,'{}')",
        "INSERT INTO leases VALUES ('lease-done','agent.test',NULL,1,2,5,'{}')",
        "INSERT INTO plans VALUES ('plan-open','agent.test',NULL,'RUNNING',1,1,'{}','{}')",
        "INSERT INTO plans VALUES ('plan-done','agent.test',NULL,'SUCCEEDED',1,1,'{}','{}')",
        "INSERT INTO plan_commands VALUES ('cmd-linked','plan-open','s1')",
    ] {
        j.execute_raw(sql).unwrap();
    }
    for subject in [
        "appr-open",
        "appr-done",
        "lease-open",
        "lease-done",
        "plan-open",
        "plan-done",
    ] {
        j.append_audit(ts(70), "x.y", subject, &json!({})).unwrap();
    }
    let rows_before = j.audit_records().unwrap().len();
    let result = archive_next(&j, 100);

    assert_eq!(result.segment.state, SegmentState::Compacted);
    assert_eq!(result.commands_archived, 2, "cmd-a and cmd-b");
    let kept = subjects(&j);
    for live in [
        "cmd-live",
        "cmd-linked",
        "appr-open",
        "lease-open",
        "plan-open",
    ] {
        assert!(
            kept.iter().any(|s| s == live),
            "{live} was deleted: {kept:?}"
        );
    }
    for gone in [
        "cmd-a",
        "cmd-b",
        "appr-done",
        "lease-done",
        "plan-done",
        "sim.psu-01",
    ] {
        assert!(!kept.iter().any(|s| s == gone), "{gone} stayed: {kept:?}");
    }
    assert!(
        kept.iter().any(|s| s.starts_with("segment-")),
        "seal and compaction records stay"
    );
    // The deleted rows are counted, and the counts add up.
    let rows_after = j.audit_records().unwrap().len() as u64;
    assert_eq!(
        rows_after + result.audit_rows_deleted,
        rows_before as u64 + 2,
        "plus the seal and the compaction record"
    );
    assert!(result.audit_rows_deleted > all_rows as u64 / 2);

    // Live commands are listed; archived ones are not, but still answered.
    let listed: Vec<String> = j
        .list_commands(None, &[], 100)
        .unwrap()
        .into_iter()
        .map(|r| r.command_id)
        .collect();
    assert!(listed.contains(&"cmd-live".to_owned()) && !listed.contains(&"cmd-a".to_owned()));
    for (id, receipt) in &before.receipts {
        assert_eq!(j.receipt(id).unwrap().as_ref(), Some(receipt), "{id}");
    }
    assert_eq!(j.receipt("never-existed").unwrap(), None);
    assert_eq!(
        j.command_by_key(AGENT, "sim.psu-01", "key-a").unwrap(),
        Some(before.receipts["cmd-a"].clone())
    );
    assert_eq!(
        j.command_by_key(AGENT, "sim.psu-01", "other").unwrap(),
        None
    );

    // The chain and the evidence are whole across the segment.
    assert!(j.verify_audit().unwrap().is_valid());
    let report = j.verify_archive().unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    let export_after = j.export_evidence().unwrap();
    let verdict = verify_evidence(&export_after, None);
    assert!(verdict.valid && verdict.complete, "{:?}", verdict.errors);
    assert_eq!(
        export_after.audit[..export_before.audit.len()],
        export_before.audit[..],
        "the history is the same, record for record"
    );
    for r in &export_before.receipts {
        assert!(export_after.receipts.contains(r), "{}", r.command_id);
    }
}

#[test]
fn a_key_used_before_archiving_still_returns_the_receipt_and_sends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (j, before) = compacted(dir.path());
    let rows = j.audit_records().unwrap().len();
    // The same request again: the archived command answers it.
    let request: openmhs_types::CommandRequest = serde_json::from_value(json!({
        "target": { "device": "sim.psu-01", "capability": "power.output.voltage.setpoint" },
        "operation": "write",
        "value": { "value": 1.0, "unit": "V" },
        "idempotency_key": "key-a",
    }))
    .unwrap();
    let again = Receipt::created("brand-new".into(), AGENT.into(), request, ts(500));
    match j.insert_command(&again, "hash-cmd-a", &json!({})).unwrap() {
        Inserted::Existing {
            receipt,
            request_hash,
        } => {
            assert_eq!(receipt, before.receipts["cmd-a"]);
            assert_eq!(request_hash, "hash-cmd-a");
        }
        Inserted::New(_) => panic!("an archived key was treated as new"),
    }
    assert_eq!(
        j.audit_records().unwrap().len(),
        rows,
        "nothing was written"
    );
    assert_eq!(j.receipt("brand-new").unwrap(), None);
    // A key never used is new, and survives its own archiving.
    let fresh = command(&j, "cmd-new", Some("key-new"), 600);
    assert_eq!(fresh.state, CommandState::Created);
    assert!(matches!(
        j.insert_command(&again, "hash", &json!({})).unwrap(),
        Inserted::Existing { .. }
    ));
}

#[test]
fn a_compacted_journal_restarts_and_verifies_with_and_without_an_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let (j, before) = compacted(dir.path());
    finished(&j, "cmd-after", Some("key-after"), 300);
    let anchor = j.head_checkpoint().unwrap();
    // An archived record is still found by its checkpoint.
    let archived_hash = j.export_evidence().unwrap().audit[4].hash.clone();
    assert!(
        j.contains_checkpoint(&AuditCheckpoint {
            seq: 5,
            hash: archived_hash
        })
        .unwrap()
    );
    assert!(
        !j.contains_checkpoint(&AuditCheckpoint {
            seq: 5,
            hash: "0".repeat(64)
        })
        .unwrap()
    );
    drop(j);
    let path = journal_path(dir.path());
    // Without an anchor the whole history is read: the segment, then the journal.
    let plain = Journal::open(&path).unwrap();
    assert!(plain.verify_audit().unwrap().is_valid());
    assert_eq!(plain.head_checkpoint().unwrap(), anchor);
    drop(plain);
    // With one, start-up reads from the newest seal.
    let fast = Journal::open_anchored(&path, Some(&anchor)).unwrap();
    for (id, receipt) in &before.receipts {
        assert_eq!(fast.receipt(id).unwrap().as_ref(), Some(receipt));
    }
    assert_eq!(
        fast.commands_in_states(&[CommandState::AwaitingApproval])
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn live_work_that_spans_a_segment_boundary_stays_until_it_ends() {
    let dir = tempfile::tempdir().unwrap();
    let (j, _) = populated(dir.path());
    archive_next(&j, 100);
    // cmd-live was held when segment 1 was compacted: all its records stayed.
    let held_rows = || {
        j.audit_records()
            .unwrap()
            .into_iter()
            .filter(|r| r.subject == "cmd-live")
            .count()
    };
    assert_eq!(held_rows(), 3);
    step(&j, "cmd-live", CommandState::Denied, 200);
    finished(&j, "cmd-x", None, 210);
    // It finished in segment 2: compacting that segment takes the rows of both ranges.
    let result = archive_next(&j, 300);
    assert_eq!(result.segment.number, 2);
    assert_eq!(held_rows(), 0);
    let r = j.receipt("cmd-live").unwrap().unwrap();
    assert_eq!(r.state, CommandState::Denied);
    assert_eq!(r.transitions.len(), 4);
    assert_eq!(
        j.command_by_key(AGENT, "sim.psu-01", "key-live")
            .unwrap()
            .unwrap(),
        r
    );
    assert!(j.verify_archive().unwrap().problems.is_empty());
    assert!(verify_evidence(&j.export_evidence().unwrap(), None).complete);
}

#[test]
fn compaction_is_refused_unless_the_segment_is_sealed_and_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let (j, _) = populated(dir.path());
    let one = j.create_segment(&through(10)).unwrap();
    let at = ts(1);
    let error = |r: Result<_, openmhs_engine::journal::JournalError>| match r {
        Err(e) => e.to_string(),
        Ok(_) => panic!("accepted"),
    };
    assert!(error(j.compact_segment(one.number, at, false)).contains("only a SEALED"));
    j.seal_segment(one.number, at).unwrap();
    let two = j.create_segment(&through(i64::MAX)).unwrap();
    j.seal_segment(two.number, at).unwrap();
    assert!(error(j.compact_segment(two.number, at, false)).contains("compacted in order"));
    j.compact_segment(one.number, at, false).unwrap();
    // The same range is never archived twice.
    assert!(error(j.compact_segment(one.number, at, false)).contains("already compacted"));
    j.compact_segment(two.number, at, false).unwrap();
    assert!(error(j.compact_segment(99, at, false)).contains("not in the manifest"));
}

#[test]
fn a_crash_at_any_step_of_archival_loses_nothing_and_can_be_finished() {
    let steps = [
        ("segment:planned", FaultPhase::BeforeCommit),
        ("segment:planned", FaultPhase::AfterCommit),
        ("segment:written", FaultPhase::BeforeCommit),
        ("segment:written", FaultPhase::AfterCommit),
        ("segment:verified", FaultPhase::BeforeCommit),
        ("segment:verified", FaultPhase::AfterCommit),
        ("segment:sealed", FaultPhase::BeforeCommit),
        ("segment:sealed", FaultPhase::AfterCommit),
        ("segment:compacting", FaultPhase::BeforeCommit),
        ("segment:compacted", FaultPhase::BeforeCommit),
        ("segment:compacted", FaultPhase::AfterCommit),
    ];
    for (site, phase) in steps {
        let dir = tempfile::tempdir().unwrap();
        let label = format!("{site} {phase:?}");
        let before = {
            let (j, before) = populated(dir.path());
            j.inject(Fault {
                site: site.into(),
                phase,
                effect: FaultEffect::Crash,
            });
            // Run the steps until the fault fires.
            let crashed = (|| {
                let row = j.create_segment(&through(i64::MAX))?;
                j.seal_segment(row.number, ts(100))?;
                j.compact_segment(row.number, ts(101), false)
            })();
            assert!(crashed.is_err(), "{label}");
            before
        };
        // Restart, finish, restart again.
        {
            let j = Journal::open(&journal_path(dir.path())).unwrap();
            for (id, receipt) in &before.receipts {
                assert_eq!(
                    j.receipt(id).unwrap().as_ref(),
                    Some(receipt),
                    "{label} {id}"
                );
            }
            finish_archive(&j, 200);
        }
        let j = Journal::open(&journal_path(dir.path())).unwrap();
        for (id, receipt) in &before.receipts {
            assert_eq!(
                j.receipt(id).unwrap().as_ref(),
                Some(receipt),
                "{label} {id}"
            );
        }
        let report = j.verify_archive().unwrap();
        assert!(report.problems.is_empty(), "{label}: {:?}", report.problems);
        assert_eq!(j.segments().unwrap().len(), 1, "{label}");
        let verdict = verify_evidence(&j.export_evidence().unwrap(), None);
        assert!(
            verdict.valid && verdict.complete,
            "{label}: {:?}",
            verdict.errors
        );
        assert_eq!(
            j.command_by_key(AGENT, "sim.psu-01", "key-a")
                .unwrap()
                .as_ref(),
            Some(&before.receipts["cmd-a"]),
            "{label}"
        );
    }
}

#[test]
fn a_full_disk_cannot_leave_a_half_compacted_journal() {
    let dir = tempfile::tempdir().unwrap();
    let (j, before) = populated(dir.path());
    let row = j.create_segment(&through(i64::MAX)).unwrap();
    j.seal_segment(row.number, ts(100)).unwrap();
    // The journal may not grow past its size now. Whatever compaction does, it does whole.
    j.set_max_bytes(j.size_bytes().unwrap()).unwrap();
    let outcome = j.compact_segment(row.number, ts(101), false);
    let state = j.segment(row.number).unwrap().unwrap().state;
    match outcome {
        Ok(_) => assert_eq!(state, SegmentState::Compacted),
        Err(_) => {
            assert_eq!(state, SegmentState::Sealed);
            assert!(
                j.audit_records()
                    .unwrap()
                    .iter()
                    .any(|r| r.subject == "cmd-a")
            );
        }
    }
    for (id, receipt) in &before.receipts {
        assert_eq!(j.receipt(id).unwrap().as_ref(), Some(receipt), "{id}");
    }
    assert!(j.verify_archive().unwrap().problems.is_empty());
}

#[test]
fn a_lost_segment_file_is_never_treated_as_an_empty_history() {
    let dir = tempfile::tempdir().unwrap();
    let (j, _) = compacted(dir.path());
    let file = j.segments_dir().unwrap().join("seg-00000001.jsonl");
    drop(j);
    // A running gateway that loses the file: a repeated key is refused, not sent again.
    let running = Journal::open(&journal_path(dir.path())).unwrap();
    std::fs::remove_file(&file).unwrap();
    let request: openmhs_types::CommandRequest = serde_json::from_value(json!({
        "target": { "device": "sim.psu-01", "capability": "power.output.voltage.setpoint" },
        "operation": "write", "value": { "value": 1.0, "unit": "V" },
        "idempotency_key": "key-a",
    }))
    .unwrap();
    let again = Receipt::created("c".into(), AGENT.into(), request, ts(1));
    let error = running
        .insert_command(&again, "hash-cmd-a", &json!({}))
        .unwrap_err();
    assert!(error.to_string().contains("cannot be read"), "{error}");
    assert!(running.receipt("cmd-a").is_err());
    assert!(
        running
            .verify_archive()
            .unwrap()
            .problems
            .iter()
            .any(|p| p.contains("segment 1"))
    );
    drop(running);
    // And a restart without an anchor refuses to start on it.
    let error = Journal::open(&journal_path(dir.path()))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("segment 1"), "{error}");
}

#[test]
fn a_damaged_manifest_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (j, _) = compacted(dir.path());
    j.execute_raw(
        "UPDATE segments SET sha256 = replace(sha256, substr(sha256, 1, 1), 'z') WHERE number = 1",
    )
    .unwrap();
    let error = j.receipt("cmd-a").unwrap_err().to_string();
    assert!(error.contains("manifest and the seal disagree"), "{error}");
    drop(j);
    let error = Journal::open(&journal_path(dir.path()))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("manifest row"), "{error}");
}

#[test]
fn a_journal_restored_from_before_its_segments_never_overwrites_them() {
    let dir = tempfile::tempdir().unwrap();
    let (j, _) = populated(dir.path());
    drop(j);
    let old = std::fs::read(journal_path(dir.path())).unwrap();
    let j = open(dir.path());
    finished(&j, "cmd-more", None, 150);
    archive_next(&j, 200);
    let file = j.segments_dir().unwrap().join("seg-00000001.jsonl");
    let written = std::fs::read(&file).unwrap();
    drop(j);
    // The older copy is put back under the newer segments.
    std::fs::write(journal_path(dir.path()), &old).unwrap();
    for ext in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", journal_path(dir.path()).display()));
    }
    let j = open(dir.path());
    assert!(j.segments().unwrap().is_empty());
    let error = j
        .create_segment(&through(i64::MAX))
        .unwrap_err()
        .to_string();
    assert!(error.contains("restored from an older copy"), "{error}");
    assert_eq!(
        std::fs::read(&file).unwrap(),
        written,
        "the segment is untouched"
    );
}

#[test]
fn damaged_bytes_in_a_compacted_journal_are_refused_or_detected_never_used() {
    let dir = tempfile::tempdir().unwrap();
    let (j, before) = compacted(dir.path());
    finished(&j, "cmd-after", Some("key-after"), 300);
    let bundle = j.export_evidence().unwrap();
    let anchor = j.head_checkpoint().unwrap();
    let segment_name = "seg-00000001.jsonl";
    let segment_path = j.segments_dir().unwrap().join(segment_name);
    drop(j);
    let journal_bytes = std::fs::read(journal_path(dir.path())).unwrap();
    let segment_bytes = std::fs::read(&segment_path).unwrap();
    let copy_home = |journal: &[u8], segment: &[u8]| {
        let case = tempfile::tempdir().unwrap();
        std::fs::write(journal_path(case.path()), journal).unwrap();
        let seg_dir = case.path().join("audit").join("segments");
        std::fs::create_dir_all(&seg_dir).unwrap();
        std::fs::write(seg_dir.join(segment_name), segment).unwrap();
        case
    };
    // What a case may do, and may not.
    let check = |journal: &Journal, label: &str, violations: &mut Vec<String>| {
        for (id, receipt) in &before.receipts {
            match journal.receipt(id) {
                Ok(Some(r)) if &r == receipt => {}
                Ok(other) => violations.push(format!("{label}: receipt {id}: {other:?}")),
                Err(_) => {}
            }
        }
        match journal.command_by_key(AGENT, "sim.psu-01", "key-a") {
            Ok(Some(r)) if r == before.receipts["cmd-a"] => {}
            Ok(other) => violations.push(format!("{label}: key-a: {other:?}")),
            Err(_) => {}
        }
        if let Ok(b) = journal.export_evidence()
            && b != bundle
            && verify_evidence(&b, None).valid
        {
            violations.push(format!("{label}: evidence differs but verifies"));
        }
    };

    let mut violations = Vec::new();
    // 1. Every bit of the segment file, one at a time: refused at start without an anchor.
    let mut missed = 0;
    // Every bit of the file is covered by the segment's own test; here a bit every few
    // bytes, through all three ways of opening the journal.
    let segment_stride = (segment_bytes.len() / 150).max(1);
    for offset in (0..segment_bytes.len()).step_by(segment_stride) {
        let mut damaged = segment_bytes.clone();
        damaged[offset] ^= 1 << (offset % 8);
        let case = copy_home(&journal_bytes, &damaged);
        if Journal::open(&journal_path(case.path())).is_ok() {
            missed += 1;
            violations.push(format!("segment bit at {offset}: opened"));
        }
        // With an anchor, history before the seal is not read at start; the damage must
        // still never be used.
        if let Ok(j) = Journal::open_anchored(&journal_path(case.path()), Some(&anchor)) {
            check(
                &j,
                &format!("anchored, segment bit {offset}"),
                &mut violations,
            );
            if j.verify_archive().unwrap().problems.is_empty() {
                violations.push(format!("segment bit at {offset}: verify_archive is clean"));
            }
        }
    }
    assert_eq!(missed, 0);
    // 2. A torn segment file: every length.
    for len in (0..segment_bytes.len()).step_by(segment_stride) {
        let case = copy_home(&journal_bytes, &segment_bytes[..len]);
        if Journal::open(&journal_path(case.path())).is_ok() {
            violations.push(format!("segment cut at {len}: opened"));
        }
    }
    // 3. Bits across the journal file itself.
    let stride = (journal_bytes.len() / 200).max(1);
    let mut refused = 0;
    for offset in (0..journal_bytes.len()).step_by(stride) {
        let mut damaged = journal_bytes.clone();
        damaged[offset] ^= 1 << (offset % 8);
        let case = copy_home(&damaged, &segment_bytes);
        match Journal::open(&journal_path(case.path())) {
            Err(_) => refused += 1,
            Ok(j) => check(&j, &format!("journal bit at {offset}"), &mut violations),
        }
    }
    assert!(
        refused > 0,
        "no flip of the journal was refused: the test proves little"
    );
    assert!(
        violations.is_empty(),
        "{} violations:\n{}",
        violations.len(),
        violations.join("\n")
    );
}
