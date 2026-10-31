//! Receipt integrity, independent verification and transaction failure boundaries.
#![allow(clippy::unwrap_used)]

mod common;
use common::*;
use openmhs_engine::journal::{Inserted, Journal};
use openmhs_policy::Caller;
use openmhs_types::evidence::{AuditCheckpoint, EvidenceBundle, audit_hash, verify_evidence};
use openmhs_types::{CommandRequest, CommandState, Receipt, Timestamp};
use serde_json::json;

fn created(journal: &Journal) -> Receipt {
    let request: CommandRequest = serde_json::from_value(write(SETPOINT, volts(1.0))).unwrap();
    let receipt = Receipt::created(
        "test-command".into(),
        AGENT.into(),
        request,
        Timestamp::from_millis(1),
    );
    match journal
        .insert_command(&receipt, "request-hash", &json!({}))
        .unwrap()
    {
        Inserted::New(receipt) => receipt,
        _ => panic!("unexpected duplicate"),
    }
}

fn rehash(bundle: &mut EvidenceBundle) {
    let mut previous = openmhs_types::evidence::GENESIS.to_owned();
    for record in &mut bundle.audit {
        record.prev_hash.clone_from(&previous);
        record.hash = audit_hash(
            &previous,
            record.seq,
            record.at_ms,
            &record.kind,
            &record.subject,
            &record.data,
        );
        previous.clone_from(&record.hash);
    }
    bundle.checkpoint = AuditCheckpoint::at_head(&bundle.audit);
}

#[tokio::test(start_paused = true)]
async fn receipts_bind_policy_native_results_and_observations_to_the_chain() {
    let (engine, _) = setup(allow_all()).await;
    engine
        .submit(&Caller::in_process(AGENT), &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    let bundle = engine.journal().export_evidence().unwrap();
    assert!(verify_evidence(&bundle, None).complete);
    for field in [
        "principal",
        "policy",
        "adapter_result",
        "verification",
        "transitions",
    ] {
        let mut edited = serde_json::to_value(&bundle).unwrap();
        let replacement = match field {
            "principal" => json!("forged"),
            "policy" => json!({ "decision": "allow", "rules": [] }),
            "adapter_result" => json!({ "external_ref": "forged-invocation" }),
            "transitions" => json!([]),
            _ => serde_json::Value::Null,
        };
        edited["receipts"][0][field] = replacement;
        let edited = serde_json::from_value(edited).unwrap();
        assert!(
            !verify_evidence(&edited, None).valid,
            "missed edited {field}"
        );
    }
}

#[test]
fn missing_duplicate_and_modified_records_are_reported() {
    let journal = Journal::open_in_memory().unwrap();
    created(&journal);
    let bundle = journal.export_evidence().unwrap();
    let mut missing = bundle.clone();
    missing.receipts.clear();
    assert!(!verify_evidence(&missing, None).valid);
    let mut duplicate = bundle.clone();
    duplicate.receipts.push(bundle.receipts[0].clone());
    assert!(!verify_evidence(&duplicate, None).valid);
    let mut edited = bundle.clone();
    edited.audit[0].data = "{}".into();
    assert!(!verify_evidence(&edited, None).valid);
    let mut wrong_pointer = bundle;
    wrong_pointer.receipts[0].audit_hash = None;
    assert!(!verify_evidence(&wrong_pointer, None).valid);
}

#[test]
fn an_independent_checkpoint_detects_tail_truncation_and_consistent_rewrites() {
    let journal = Journal::open_in_memory().unwrap();
    created(&journal);
    journal
        .append_audit(Timestamp::from_millis(2), "test", "s", &json!({}))
        .unwrap();
    let bundle = journal.export_evidence().unwrap();
    let anchor = bundle.checkpoint.clone();
    let mut truncated = bundle.clone();
    truncated.audit.pop();
    truncated.checkpoint = AuditCheckpoint::at_head(&truncated.audit);
    assert!(
        verify_evidence(&truncated, None).complete,
        "a self-contained chain cannot detect tail loss"
    );
    assert!(!verify_evidence(&truncated, Some(&anchor)).valid);
    let mut rewritten = bundle;
    rewritten.audit.last_mut().unwrap().data = "{\"forged\":true}".into();
    rehash(&mut rewritten);
    assert!(verify_evidence(&rewritten, None).complete);
    assert!(!verify_evidence(&rewritten, Some(&anchor)).valid);
}

#[test]
fn legacy_receipts_are_reported_as_incomplete_instead_of_authenticated() {
    let journal = Journal::open_in_memory().unwrap();
    created(&journal);
    let mut legacy = journal.export_evidence().unwrap();
    let mut data: serde_json::Value = serde_json::from_str(&legacy.audit[0].data).unwrap();
    data.as_object_mut().unwrap().remove("receipt_digest");
    legacy.audit[0].data = data.to_string();
    rehash(&mut legacy);
    legacy.receipts[0].audit_hash = Some(legacy.audit[0].hash.clone());
    let result = verify_evidence(&legacy, None);
    assert!(result.valid);
    assert!(!result.complete);
    assert_eq!(result.unbound_commands, vec!["test-command"]);
}

#[test]
fn corrupt_receipt_cannot_be_laundered_by_a_later_transition() {
    let journal = Journal::open_in_memory().unwrap();
    created(&journal);
    let before = journal.audit_records().unwrap();
    journal
        .execute_raw("UPDATE commands SET receipt = json_set(receipt, '$.principal', 'forged')")
        .unwrap();
    assert!(
        journal.verify_audit().unwrap().is_valid(),
        "chain-only verification remains compatible"
    );
    assert!(journal.receipt("test-command").is_err());
    assert!(journal.list_commands(None, &[], 10).is_err());
    let error = journal
        .transition(
            "test-command",
            CommandState::Validating,
            Timestamp::from_millis(2),
            None,
            |_| {},
        )
        .unwrap_err();
    assert!(error.to_string().contains("receipt integrity"));
    assert_eq!(journal.audit_records().unwrap(), before);
    assert!(!verify_evidence(&journal.export_evidence().unwrap(), None).valid);
}

#[test]
fn a_changed_state_index_cannot_hide_admitted_work_from_recovery() {
    // Recovery finds unfinished commands through the state index; opening the journal checks
    // that index against the commands the audit chain leaves unfinished.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.sqlite");
    let journal = Journal::open(&path).unwrap();
    created(&journal);
    journal
        .execute_raw("UPDATE commands SET state = 'VERIFIED'")
        .unwrap();
    drop(journal);
    let opened = Journal::open(&path);
    assert!(
        matches!(&opened, Err(openmhs_engine::JournalError::Refused(e)) if e.code == "journal.receipt_integrity"),
        "{:?}",
        opened.err()
    );
}

#[test]
fn receipt_update_failure_rolls_back_the_audit_digest_as_well() {
    let journal = Journal::open_in_memory().unwrap();
    created(&journal);
    let before = journal.export_evidence().unwrap();
    journal.execute_raw("CREATE TRIGGER fail_receipt BEFORE UPDATE ON commands BEGIN SELECT RAISE(ABORT, 'injected storage failure'); END").unwrap();
    assert!(
        journal
            .transition(
                "test-command",
                CommandState::Validating,
                Timestamp::from_millis(2),
                None,
                |_| {}
            )
            .is_err()
    );
    assert_eq!(journal.export_evidence().unwrap(), before);
}
