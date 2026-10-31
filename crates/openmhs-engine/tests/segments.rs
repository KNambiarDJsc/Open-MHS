//! Sealed audit segments (RFC 0002, milestone 3): written once, verified from the file
//! alone, and checked against the journal they came from. Nothing is deleted.
#![allow(clippy::unwrap_used)]

mod fixture;

use std::io::BufReader;

use fixture::*;
use openmhs_engine::journal::{SegmentLimits, SegmentState};
use openmhs_types::CommandState;
use openmhs_types::segment::{SegmentError, for_each, verify_segment};

fn through(seq: i64) -> SegmentLimits {
    SegmentLimits {
        max_bytes: u64::MAX,
        through_seq: Some(seq),
    }
}

/// A journal with finished, held and noise records.
fn populated(dir: &std::path::Path) -> openmhs_engine::journal::Journal {
    let j = open(dir);
    finished(&j, "cmd-a", Some("key-a"), 10);
    held(&j, "cmd-b", Some("key-b"), 20);
    noise(&j, 5, 30);
    finished(&j, "cmd-c", None, 40);
    j
}

#[test]
fn a_segment_holds_every_record_and_final_receipt_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let j = populated(dir.path());
    let head = j.head_checkpoint().unwrap().seq;
    let row = j.create_segment(&through(head)).unwrap();
    assert_eq!(row.state, SegmentState::Verified);
    assert_eq!((row.number, row.first_seq, row.last_seq), (1, 1, head));

    let path = j.segments_dir().unwrap().join(row.file.as_deref().unwrap());
    let (mut records, mut commands) = (Vec::new(), Vec::new());
    for_each(
        BufReader::new(std::fs::File::open(&path).unwrap()),
        |r| {
            records.push(r);
            Ok(())
        },
        |c| {
            commands.push(c);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(records, j.audit_records().unwrap());
    let ids: Vec<&str> = commands
        .iter()
        .map(|c| c.receipt.command_id.as_str())
        .collect();
    assert_eq!(ids, ["cmd-a", "cmd-b", "cmd-c"]);
    for c in &commands {
        assert_eq!(
            Some(c.receipt.clone()),
            j.receipt(&c.receipt.command_id).unwrap()
        );
    }
    // The held command's file entry is its current receipt, so it is not final.
    assert_eq!(commands[1].receipt.state, CommandState::AwaitingApproval);

    let summary = verify_segment(BufReader::new(std::fs::File::open(&path).unwrap())).unwrap();
    assert_eq!(summary.footer.records, u64::try_from(head).unwrap());
    assert!(summary.unbound_commands.is_empty());
    // The manifest copy for offline verifiers lists it.
    let manifest = std::fs::read_to_string(j.manifest_path().unwrap()).unwrap();
    assert!(manifest.contains("\"VERIFIED\"") && manifest.contains(&summary.footer.sha256));
    // Nothing was deleted.
    assert_eq!(
        j.audit_records().unwrap().len(),
        usize::try_from(head).unwrap()
    );
}

#[test]
fn any_changed_bit_in_a_segment_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    let j = open(dir.path());
    finished(&j, "cmd-a", Some("k"), 10);
    held(&j, "cmd-b", None, 20);
    let head = j.head_checkpoint().unwrap().seq;
    let row = j.create_segment(&through(head)).unwrap();
    let path = j.segments_dir().unwrap().join(row.file.unwrap());
    let good = std::fs::read(&path).unwrap();
    assert!(verify_segment(good.as_slice()).is_ok());
    let mut undetected = Vec::new();
    for at in 0..good.len() {
        for bit in [0u8, 3, 7] {
            let mut bad = good.clone();
            bad[at] ^= 1 << bit;
            if verify_segment(bad.as_slice()).is_ok() {
                undetected.push((at, bit));
            }
        }
    }
    assert!(undetected.is_empty(), "undetected flips: {undetected:?}");
    // Truncation anywhere, and trailing data.
    for len in 0..good.len() {
        assert!(verify_segment(&good[..len]).is_err(), "cut at {len}");
    }
    let mut more = good.clone();
    more.extend_from_slice(b"{}\n");
    assert!(matches!(
        verify_segment(more.as_slice()),
        Err(SegmentError::Invalid(_))
    ));
}

#[test]
fn consecutive_segments_chain_and_a_command_lands_in_the_segment_of_its_last_record() {
    let dir = tempfile::tempdir().unwrap();
    let j = open(dir.path());
    held(&j, "cmd-b", Some("kb"), 10);
    let first_end = j.head_checkpoint().unwrap().seq;
    let one = j.create_segment(&through(first_end)).unwrap();
    // The held command moves on after the first segment closed.
    step(&j, "cmd-b", CommandState::Denied, 30);
    finished(&j, "cmd-c", None, 40);
    let two = j.create_segment(&through(i64::MAX)).unwrap();
    assert_eq!((two.number, two.first_seq), (2, one.last_seq + 1));
    assert_eq!(two.prev_hash, one.last_hash.clone().unwrap());
    assert_eq!(two.chain_root, one.chain_root);

    let read = |row: &openmhs_engine::journal::SegmentRow| {
        let mut ids = Vec::new();
        let path = j.segments_dir().unwrap().join(row.file.as_deref().unwrap());
        for_each(
            BufReader::new(std::fs::File::open(path).unwrap()),
            |_| Ok(()),
            |c| {
                ids.push((c.receipt.command_id, c.receipt.state));
                Ok(())
            },
        )
        .unwrap();
        ids
    };
    // The first segment shows the command where it was, the second where it ended up.
    assert_eq!(
        read(&one),
        [("cmd-b".to_owned(), CommandState::AwaitingApproval)]
    );
    assert_eq!(
        read(&two),
        [
            ("cmd-b".to_owned(), CommandState::Denied),
            ("cmd-c".to_owned(), CommandState::Denied)
        ]
    );
    assert_eq!(j.segments().unwrap(), [one, two]);
}

#[test]
fn a_range_that_is_not_full_is_not_closed_unless_asked() {
    let dir = tempfile::tempdir().unwrap();
    let j = populated(dir.path());
    let small = SegmentLimits {
        max_bytes: 1 << 30,
        through_seq: None,
    };
    let error = j.create_segment(&small).unwrap_err().to_string();
    assert!(error.contains("through-seq"), "{error}");
    assert!(j.segments().unwrap().is_empty());
    // A byte budget closes the segment once the records reach it.
    let tiny = SegmentLimits {
        max_bytes: 600,
        through_seq: None,
    };
    let row = j.create_segment(&tiny).unwrap();
    assert!(row.last_seq < j.head_checkpoint().unwrap().seq);
    // An in-memory journal has no place for files.
    let memory = openmhs_engine::journal::Journal::open_in_memory().unwrap();
    assert!(memory.create_segment(&through(1)).is_err());
}

#[test]
fn a_crash_at_any_step_leaves_a_journal_that_can_write_the_segment_again() {
    use openmhs_engine::journal::{Fault, FaultEffect, FaultPhase};
    let sites = [
        ("segment:planned", FaultPhase::BeforeCommit),
        ("segment:planned", FaultPhase::AfterCommit),
        ("segment:written", FaultPhase::BeforeCommit),
        ("segment:written", FaultPhase::AfterCommit),
        ("segment:verified", FaultPhase::BeforeCommit),
        ("segment:verified", FaultPhase::AfterCommit),
    ];
    for (site, phase) in sites {
        let dir = tempfile::tempdir().unwrap();
        let before = {
            let j = populated(dir.path());
            let records = j.audit_records().unwrap();
            j.inject(Fault {
                site: site.into(),
                phase,
                effect: FaultEffect::Crash,
            });
            let head = records.len() as i64;
            assert!(
                j.create_segment(&through(head)).is_err(),
                "{site} {phase:?}"
            );
            records
        };
        // Restart.
        let j = open(dir.path());
        let head = before.len() as i64;
        // Repeating finishes whatever was left; if the crash came after the last commit
        // there is nothing left, and the range is already done.
        match j.create_segment(&through(head)) {
            Ok(row) => assert_eq!(row.state, SegmentState::Verified, "{site} {phase:?}"),
            Err(e) => assert!(
                e.to_string().contains("no audit records after"),
                "{site} {phase:?}: {e}"
            ),
        }
        let rows = j.segments().unwrap();
        assert_eq!(rows.len(), 1, "{site} {phase:?}");
        assert_eq!(rows[0].state, SegmentState::Verified, "{site} {phase:?}");
        assert_eq!(j.audit_records().unwrap(), before, "{site} {phase:?}");
        let dirlist: Vec<String> = std::fs::read_dir(j.segments_dir().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            dirlist,
            ["seg-00000001.jsonl"],
            "{site} {phase:?}: {dirlist:?}"
        );
    }
}

#[test]
fn a_full_disk_while_writing_leaves_no_file_and_the_range_can_be_retried() {
    let dir = tempfile::tempdir().unwrap();
    let j = populated(dir.path());
    let head = j.head_checkpoint().unwrap().seq;
    j.fail_segment_writes_after(Some(300));
    assert!(j.create_segment(&through(head)).is_err());
    let leftovers = std::fs::read_dir(j.segments_dir().unwrap())
        .unwrap()
        .count();
    assert_eq!(leftovers, 0, "no partial file stays behind");
    j.fail_segment_writes_after(None);
    let row = j.create_segment(&through(head)).unwrap();
    assert_eq!(row.state, SegmentState::Verified);
    assert_eq!(j.segments().unwrap().len(), 1);
}

#[test]
fn a_file_that_differs_from_the_journal_is_not_verified() {
    let dir = tempfile::tempdir().unwrap();
    let j = populated(dir.path());
    let head = j.head_checkpoint().unwrap().seq;
    let row = j.create_segment(&through(head)).unwrap();
    // The journal's row is edited after the file was written: the file no longer agrees.
    j.execute_raw("UPDATE audit SET data = '{}' WHERE seq = 3")
        .unwrap();
    let error = j.verify_segment_file(row.number).unwrap_err().to_string();
    assert!(error.contains("differs from the journal"), "{error}");
    // A damaged file is refused too.
    let path = j.segments_dir().unwrap().join(row.file.unwrap());
    let mut bytes = std::fs::read(&path).unwrap();
    let at = bytes.len() / 2;
    bytes[at] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    assert!(j.verify_segment_file(row.number).is_err());
}
