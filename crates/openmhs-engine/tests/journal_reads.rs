//! Bounded API reads (RFC 0002, M2): the filtered lists and the streamed evidence export
//! return what a full read would, and an export neither holds the journal nor sees writes
//! made while it runs.
#![allow(clippy::unwrap_used)]

mod common;

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use openmhs_engine::journal::{Inserted, Journal};
use openmhs_types::evidence::{AuditCheckpoint, EvidenceBundle, verify_evidence};
use openmhs_types::{CommandRequest, CommandState, Receipt, Timestamp};
use serde_json::json;

fn create(journal: &Journal, id: &str, principal: &str, at: i64) {
    let request: CommandRequest = serde_json::from_value(write(SETPOINT, volts(1.0))).unwrap();
    let receipt = Receipt::created(
        id.into(),
        principal.into(),
        request,
        Timestamp::from_millis(at),
    );
    assert!(matches!(
        journal
            .insert_command(&receipt, "request-hash", &json!({}))
            .unwrap(),
        Inserted::New(_)
    ));
}

fn to(journal: &Journal, id: &str, state: CommandState, at: i64) {
    journal
        .transition(id, state, Timestamp::from_millis(at), None, |_| {})
        .unwrap();
}

fn ids(receipts: &[Receipt]) -> Vec<String> {
    receipts.iter().map(|r| r.command_id.clone()).collect()
}

#[test]
fn list_filters_return_what_a_full_read_would() {
    let journal = Journal::open_in_memory().unwrap();
    for i in 0..24i64 {
        let id = format!("c{i:02}");
        // Two commands share each creation time, so the id breaks the tie.
        create(
            &journal,
            &id,
            if i % 3 == 0 { "a" } else { "b" },
            100 + i / 2,
        );
        if i % 2 == 0 {
            to(&journal, &id, CommandState::Validating, 200 + i);
        }
        if i % 4 == 0 {
            to(&journal, &id, CommandState::Denied, 300 + i);
        }
    }
    let everything = journal.list_commands(None, &[], 1000).unwrap();
    assert_eq!(everything.len(), 24);
    let state_sets: [&[CommandState]; 3] = [
        &[],
        &[CommandState::Created],
        &[CommandState::Denied, CommandState::Validating],
    ];
    for principal in [None, Some("a"), Some("b"), Some("nobody")] {
        for states in state_sets {
            for limit in [1, 3, 100] {
                let expected: Vec<String> = everything
                    .iter()
                    .filter(|r| principal.is_none_or(|p| r.principal == p))
                    .filter(|r| states.is_empty() || states.contains(&r.state))
                    .take(limit)
                    .map(|r| r.command_id.clone())
                    .collect();
                let got = journal.list_commands(principal, states, limit).unwrap();
                assert_eq!(ids(&got), expected, "{principal:?} {states:?} {limit}");
            }
        }
    }
    // Newest first, ties broken by the larger id.
    assert_eq!(ids(&everything[..2]), ["c23", "c22"]);
}

/// The export of a file journal is the same bundle a full read gives.
#[test]
fn a_streamed_export_is_the_whole_journal() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::open(&dir.path().join("journal.sqlite")).unwrap();
    for i in 0..5i64 {
        create(&journal, &format!("c{i}"), AGENT, i);
    }
    to(&journal, "c1", CommandState::Validating, 10);
    let mut out = Vec::new();
    let checkpoint = journal.write_evidence(&mut out).unwrap();
    let bundle: EvidenceBundle = serde_json::from_slice(&out).unwrap();
    let audit = journal.audit_records().unwrap();
    assert_eq!(checkpoint, AuditCheckpoint::at_head(&audit));
    assert_eq!(bundle.checkpoint, checkpoint);
    assert_eq!(bundle.audit, audit);
    assert_eq!(
        ids(&bundle.receipts),
        ["c0", "c1", "c2", "c3", "c4"].map(String::from)
    );
    let result = verify_evidence(&bundle, Some(&checkpoint));
    assert!(result.valid && result.complete, "{result:?}");
}

/// A writer that, on its first write, records something in the journal being exported.
struct WritesDuringExport {
    journal: Arc<Journal>,
    out: Vec<u8>,
    wrote: bool,
}

impl Write for WritesDuringExport {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if !std::mem::replace(&mut self.wrote, true) {
            self.journal
                .append_audit(
                    Timestamp::from_millis(99),
                    "test.during_export",
                    "s",
                    &json!({}),
                )
                .unwrap();
        }
        self.out.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A slow export client must not stop commands: the export reads on its own connection,
/// from one snapshot, so a write made meanwhile neither waits for it nor appears in it.
#[test]
fn an_export_does_not_hold_the_journal_and_reads_one_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Arc::new(Journal::open(&dir.path().join("journal.sqlite")).unwrap());
    create(&journal, "c0", AGENT, 1);
    let before = journal.audit_records().unwrap();
    let (done, finished) = std::sync::mpsc::channel();
    let exporting = journal.clone();
    std::thread::spawn(move || {
        let mut writer = WritesDuringExport {
            journal: exporting.clone(),
            out: Vec::new(),
            wrote: false,
        };
        let checkpoint = exporting.write_evidence(&mut writer).unwrap();
        done.send((checkpoint, writer.out)).unwrap();
    });
    let (checkpoint, out) = finished
        .recv_timeout(Duration::from_secs(20))
        .expect("the export held the journal: a write during it never finished");
    let bundle: EvidenceBundle = serde_json::from_slice(&out).unwrap();
    assert_eq!(bundle.audit, before);
    assert_eq!(checkpoint, AuditCheckpoint::at_head(&before));
    assert!(verify_evidence(&bundle, None).valid);
    assert_eq!(journal.audit_records().unwrap().len(), before.len() + 1);
}

/// A writer that fails part-way, as a client that goes away does.
struct Fails;

impl Write for Fails {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "client went away",
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_failed_export_reports_the_error_and_leaves_the_journal_usable() {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::open(&dir.path().join("journal.sqlite")).unwrap();
    create(&journal, "c0", AGENT, 1);
    assert!(journal.write_evidence(&mut Fails).is_err());
    to(&journal, "c0", CommandState::Validating, 2);
    assert!(verify_evidence(&journal.export_evidence().unwrap(), None).valid);
}
