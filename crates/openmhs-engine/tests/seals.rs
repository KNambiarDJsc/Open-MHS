//! Seal records and start-up from the newest seal (RFC 0002, milestone 4): history before a
//! seal is read again only when no anchor kept elsewhere vouches for it, and every refusal
//! the full verification makes is still made for the part that is read.
#![allow(clippy::unwrap_used)]

mod fixture;

use fixture::*;
use openmhs_engine::journal::{
    Journal, JournalError, SEAL_KIND, SealData, SegmentLimits, SegmentState,
};
use openmhs_types::evidence::AuditCheckpoint;
use openmhs_types::{CommandState, Timestamp};

fn through(seq: i64) -> SegmentLimits {
    SegmentLimits {
        max_bytes: u64::MAX,
        through_seq: Some(seq),
    }
}

/// A journal with a sealed first segment, then a little more history. Returns the anchor an
/// operator would have kept: the head after everything.
fn sealed(dir: &std::path::Path) -> (Journal, AuditCheckpoint) {
    let j = open(dir);
    finished(&j, "cmd-a", Some("key-a"), 10);
    held(&j, "cmd-b", Some("key-b"), 20);
    noise(&j, 5, 30);
    let head = j.head_checkpoint().unwrap().seq;
    let row = j.create_segment(&through(head)).unwrap();
    j.seal_segment(row.number, Timestamp::from_millis(100))
        .unwrap();
    finished(&j, "cmd-c", None, 200);
    step(&j, "cmd-b", CommandState::Denied, 300);
    held(&j, "cmd-d", None, 400);
    let anchor = j.head_checkpoint().unwrap();
    (j, anchor)
}

fn refused(result: Result<Journal, JournalError>) -> String {
    match result {
        Err(e) => e.to_string(),
        Ok(_) => panic!("opened"),
    }
}

#[test]
fn a_seal_commits_the_chain_to_the_file_and_to_the_live_state() {
    let dir = tempfile::tempdir().unwrap();
    let j = open(dir.path());
    finished(&j, "cmd-a", None, 10);
    held(&j, "cmd-b", None, 20);
    let head = j.head_checkpoint().unwrap().seq;
    let row = j.create_segment(&through(head)).unwrap();
    assert_eq!(row.state, SegmentState::Verified);
    let sealed = j
        .seal_segment(row.number, Timestamp::from_millis(99))
        .unwrap();
    assert_eq!(sealed.state, SegmentState::Sealed);
    assert_eq!(sealed.seal_seq, Some(head + 1));

    let seal = j.audit_tail(1).unwrap().pop().unwrap();
    assert_eq!((seal.seq, seal.kind.as_str()), (head + 1, SEAL_KIND));
    let data: SealData = serde_json::from_str(&seal.data).unwrap();
    assert_eq!(Some(&data.file_sha256), row.sha256.as_ref());
    assert_eq!(Some(&data.last_hash), row.last_hash.as_ref());
    assert_eq!(data.snapshot.seq, head);
    assert_eq!(
        data.snapshot.commands,
        ["cmd-b"],
        "only the held command is live"
    );
    assert!(data.snapshot.is_consistent());
    // The chain still verifies, and sealing again changes nothing.
    assert!(j.verify_audit().unwrap().is_valid());
    assert_eq!(
        j.seal_segment(row.number, Timestamp::from_millis(5))
            .unwrap(),
        sealed
    );
    assert_eq!(j.head_checkpoint().unwrap().seq, head + 1);
}

#[test]
fn segments_are_sealed_in_order_and_only_once_verified() {
    let dir = tempfile::tempdir().unwrap();
    let j = open(dir.path());
    finished(&j, "cmd-a", None, 10);
    let one = j
        .create_segment(&through(j.head_checkpoint().unwrap().seq))
        .unwrap();
    finished(&j, "cmd-b", None, 20);
    let two = j.create_segment(&through(i64::MAX)).unwrap();
    let at = Timestamp::from_millis(1);
    let error = j.seal_segment(two.number, at).unwrap_err().to_string();
    assert!(error.contains("sealed in order"), "{error}");
    assert!(j.seal_segment(99, at).is_err());
    j.seal_segment(one.number, at).unwrap();
    j.seal_segment(two.number, at).unwrap();
}

#[test]
fn start_up_from_a_seal_gives_the_answer_full_verification_gives() {
    let dir = tempfile::tempdir().unwrap();
    let (j, anchor) = sealed(dir.path());
    let full = (
        j.verify_audit().unwrap(),
        j.commands_in_states(&[CommandState::AwaitingApproval])
            .unwrap(),
    );
    drop(j);
    let fast = Journal::open_anchored(&journal_path(dir.path()), Some(&anchor)).unwrap();
    assert_eq!(
        fast.commands_in_states(&[CommandState::AwaitingApproval])
            .unwrap(),
        full.1
    );
    assert_eq!(fast.head_checkpoint().unwrap(), anchor);
    // Opened without an anchor, the same journal verifies everything.
    let plain = open(dir.path());
    assert_eq!(plain.verify_audit().unwrap(), full.0);
}

#[test]
fn an_anchor_is_what_lets_start_up_skip_history_before_the_seal() {
    let dir = tempfile::tempdir().unwrap();
    let (j, anchor) = sealed(dir.path());
    // Seq 3 is before the seal: an edit that leaves every stored hash alone.
    j.execute_raw("UPDATE audit SET data = '{\"edited\":true}' WHERE seq = 3")
        .unwrap();
    drop(j);
    let path = journal_path(dir.path());
    // Without an anchor the whole chain is read, and the edit is refused as it always was.
    assert!(refused(Journal::open(&path)).contains("broken at record 3"));
    // With one, the part the seal covers is vouched for by the anchor and is not read;
    // `verify_archive` still finds the edit when asked to read everything.
    let fast = Journal::open_anchored(&path, Some(&anchor)).unwrap();
    let report = fast.verify_archive().unwrap();
    assert_eq!(report.first_invalid, Some(3));
}

#[test]
fn an_anchor_that_does_not_fit_the_journal_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (j, anchor) = sealed(dir.path());
    j.execute_raw("UPDATE audit SET data = '{\"edited\":true}' WHERE seq = 3")
        .unwrap();
    drop(j);
    let path = journal_path(dir.path());
    // A different history, a head the journal never had, and an anchor from before the seal:
    // none lets history be skipped.
    let other = AuditCheckpoint {
        seq: anchor.seq,
        hash: "0".repeat(64),
    };
    let ahead = AuditCheckpoint {
        seq: anchor.seq + 10,
        hash: anchor.hash.clone(),
    };
    let before_seal = AuditCheckpoint {
        seq: 4,
        hash: Journal::open_in_memory()
            .unwrap()
            .head_checkpoint()
            .unwrap()
            .hash,
    };
    for (name, anchor) in [("other", other), ("ahead", ahead), ("before", before_seal)] {
        let error = refused(Journal::open_anchored(&path, Some(&anchor)));
        assert!(error.contains("broken at record 3"), "{name}: {error}");
    }
}

#[test]
fn every_refusal_still_holds_for_the_part_that_is_read() {
    // Each tampering of the records after the seal, or of the live state, is refused with an
    // anchor exactly as it is without one.
    let tamperings: [(&str, &str); 6] = [
        (
            "an edited tail record",
            "UPDATE audit SET data = '{\"x\":1}' WHERE seq = (SELECT MAX(seq) - 2 FROM audit)",
        ),
        (
            "a deleted tail record",
            "DELETE FROM audit WHERE seq = (SELECT MAX(seq) - 3 FROM audit)",
        ),
        (
            "an edited seal record",
            "UPDATE audit SET data = replace(data, '\"snapshot\"', '\"snapshoX\"') \
             WHERE kind = 'audit.segment_sealed'",
        ),
        (
            "a held command shown as finished",
            "UPDATE commands SET state = 'VERIFIED' WHERE command_id = 'cmd-d'",
        ),
        (
            "a finished command shown as held",
            "UPDATE commands SET state = 'AWAITING_APPROVAL' WHERE command_id = 'cmd-c'",
        ),
        (
            "a held command's row removed",
            "DELETE FROM commands WHERE command_id = 'cmd-d'",
        ),
    ];
    for (name, sql) in tamperings {
        let dir = tempfile::tempdir().unwrap();
        let (j, anchor) = sealed(dir.path());
        // Make cmd-b still held at the head for the last case.
        j.execute_raw(sql).unwrap();
        drop(j);
        let path = journal_path(dir.path());
        let plain = Journal::open(&path);
        let anchored = Journal::open_anchored(&path, Some(&anchor));
        assert!(plain.is_err(), "{name}: opened without an anchor");
        assert!(anchored.is_err(), "{name}: opened with an anchor");
    }
}

#[test]
fn a_damaged_segment_file_means_everything_is_read_and_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let (j, anchor) = sealed(dir.path());
    let row = j.segments().unwrap().remove(0);
    let file = j.segments_dir().unwrap().join(row.file.unwrap());
    drop(j);
    let mut bytes = std::fs::read(&file).unwrap();
    let last = bytes.len() - 5;
    bytes[last] ^= 1;
    std::fs::write(&file, bytes).unwrap();
    // The rows are intact, so the journal opens (after reading all of them), and the
    // damaged file is what `verify_archive` reports.
    let j = Journal::open_anchored(&journal_path(dir.path()), Some(&anchor)).unwrap();
    let report = j.verify_archive().unwrap();
    assert!(report.first_invalid.is_none());
    assert!(
        report.problems.iter().any(|p| p.contains("segment 1")),
        "{:?}",
        report.problems
    );
}

#[test]
fn verify_archive_of_a_healthy_journal_is_clean() {
    let dir = tempfile::tempdir().unwrap();
    let (j, anchor) = sealed(dir.path());
    let report = j.verify_archive().unwrap();
    assert_eq!(report.problems, Vec::<String>::new());
    assert_eq!(
        (report.segments, report.records),
        (1, u64::try_from(anchor.seq).unwrap())
    );
}
