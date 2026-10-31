//! Receipt integrity, independent verification and transaction failure boundaries.
#![allow(clippy::unwrap_used)]

mod common;
use common::*;
use openmhs_engine::journal::{Inserted, Journal};
use openmhs_policy::Caller;
use openmhs_types::evidence::{
    AuditCheckpoint, EvidenceBundle, EvidenceVerification, audit_hash, verify_evidence,
    verify_evidence_reader,
};
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

/// The bundle as JSON text with its members in the given order.
fn bundle_text(bundle: &EvidenceBundle, order: &[&str]) -> String {
    let value = serde_json::to_value(bundle).unwrap();
    let members: Vec<String> = order
        .iter()
        .map(|k| format!("{}:{}", json!(k), value[*k]))
        .collect();
    format!("{{{}}}", members.join(","))
}

/// The whole-bundle verifier as it was before it became a streaming state machine, kept as
/// an independent reference for the equivalence test below.
fn reference_verify(
    bundle: &EvidenceBundle,
    anchor: Option<&AuditCheckpoint>,
) -> EvidenceVerification {
    use openmhs_types::evidence::{GENESIS, receipt_digest};
    use std::collections::{BTreeMap, BTreeSet};
    let mut errors = Vec::new();
    let mut unbound_commands = Vec::new();
    if bundle.version != 1 {
        errors.push(format!("unsupported evidence version {}", bundle.version));
    }
    let mut previous = GENESIS.to_owned();
    let mut latest = BTreeMap::new();
    let mut created = BTreeSet::new();
    for (expected, record) in (1i64..).zip(&bundle.audit) {
        if record.seq != expected
            || record.prev_hash != previous
            || record.hash
                != audit_hash(
                    &previous,
                    record.seq,
                    record.at_ms,
                    &record.kind,
                    &record.subject,
                    &record.data,
                )
        {
            errors.push(format!("invalid audit record at sequence {}", record.seq));
        }
        previous.clone_from(&record.hash);
        if record.kind == "command.created" && !created.insert(record.subject.as_str()) {
            errors.push(format!("duplicate command creation: {}", record.subject));
        }
        if matches!(
            record.kind.as_str(),
            "command.created" | "command.transition"
        ) {
            latest.insert(record.subject.as_str(), record);
        }
    }
    if bundle.checkpoint != AuditCheckpoint::at_head(&bundle.audit) {
        errors.push("bundle checkpoint does not match audit head".into());
    }
    if let Some(anchor) = anchor {
        let matches = if anchor.seq == 0 {
            anchor.hash == GENESIS
        } else {
            bundle
                .audit
                .iter()
                .any(|r| r.seq == anchor.seq && r.hash == anchor.hash)
        };
        if !matches {
            errors.push("independent checkpoint is missing or does not match".into());
        }
    }
    let mut seen = BTreeSet::new();
    for receipt in &bundle.receipts {
        let id = receipt.command_id.as_str();
        if !seen.insert(id) {
            errors.push(format!("duplicate receipt: {id}"));
        }
        if !created.contains(id) {
            errors.push(format!("receipt has no command creation record: {id}"));
        }
        let Some(record) = latest.get(id) else {
            continue;
        };
        if receipt.audit_hash.as_deref() != Some(record.hash.as_str()) {
            errors.push(format!(
                "receipt audit pointer differs from latest record: {id}"
            ));
        }
        match serde_json::from_str::<serde_json::Value>(&record.data) {
            Ok(data) => match data.get("receipt_digest") {
                None => unbound_commands.push(id.to_owned()),
                Some(digest) => {
                    let expected = receipt_digest(receipt).ok();
                    if digest["version"] != 1 || expected.as_deref() != digest["sha256"].as_str() {
                        errors.push(format!("receipt digest mismatch: {id}"));
                    }
                }
            },
            Err(_) => errors.push(format!("invalid command audit JSON: {id}")),
        }
    }
    for id in latest.keys() {
        if !seen.contains(id) {
            errors.push(format!("missing receipt: {id}"));
        }
    }
    EvidenceVerification {
        valid: errors.is_empty(),
        complete: errors.is_empty() && unbound_commands.is_empty(),
        records: bundle.audit.len(),
        receipts: bundle.receipts.len(),
        unbound_commands,
        errors,
    }
}

fn stream(text: &str, anchor: Option<&AuditCheckpoint>) -> Result<EvidenceVerification, String> {
    verify_evidence_reader(text.as_bytes(), anchor).map_err(|e| e.to_string())
}

#[test]
fn the_streaming_verifier_reports_exactly_what_the_whole_bundle_verifier_does() {
    let journal = Journal::open_in_memory().unwrap();
    created(&journal);
    journal
        .append_audit(Timestamp::from_millis(2), "test", "s", &json!({}))
        .unwrap();
    let good = journal.export_evidence().unwrap();
    let anchor = good.checkpoint.clone();

    let mut cases: Vec<(&str, EvidenceBundle, Option<AuditCheckpoint>)> = Vec::new();
    cases.push(("good", good.clone(), Some(anchor.clone())));
    cases.push(("no anchor", good.clone(), None));
    let mut edited = good.clone();
    edited.audit[0].data = "{}".into();
    cases.push(("edited record", edited, None));
    let mut missing = good.clone();
    missing.receipts.clear();
    cases.push(("missing receipt", missing, None));
    let mut duplicate = good.clone();
    duplicate.receipts.push(good.receipts[0].clone());
    cases.push(("duplicate receipt", duplicate, None));
    let mut pointer = good.clone();
    pointer.receipts[0].audit_hash = None;
    cases.push(("wrong pointer", pointer, None));
    let mut forged = good.clone();
    forged.receipts[0].principal = "forged".into();
    cases.push(("forged receipt", forged, None));
    let mut version = good.clone();
    version.version = 2;
    cases.push(("version", version, None));
    let mut head = good.clone();
    head.checkpoint.seq += 1;
    cases.push(("checkpoint", head, None));
    let mut truncated = good.clone();
    truncated.audit.pop();
    truncated.checkpoint = AuditCheckpoint::at_head(&truncated.audit);
    cases.push((
        "truncated, anchored",
        truncated.clone(),
        Some(anchor.clone()),
    ));
    cases.push(("truncated, unanchored", truncated, None));
    let mut rewritten = good.clone();
    rewritten.audit.last_mut().unwrap().data = "{\"forged\":true}".into();
    rehash(&mut rewritten);
    cases.push(("rewritten, anchored", rewritten, Some(anchor.clone())));
    let mut legacy = good.clone();
    let mut data: serde_json::Value = serde_json::from_str(&legacy.audit[0].data).unwrap();
    data.as_object_mut().unwrap().remove("receipt_digest");
    legacy.audit[0].data = data.to_string();
    rehash(&mut legacy);
    legacy.receipts[0].audit_hash = Some(legacy.audit[0].hash.clone());
    cases.push(("legacy", legacy, None));
    let mut garbage = good.clone();
    garbage.audit[0].data = "not json".into();
    rehash(&mut garbage);
    garbage.receipts[0].audit_hash = Some(garbage.audit[0].hash.clone());
    cases.push(("record data is not JSON", garbage, None));
    let mut empty = good.clone();
    empty.audit.clear();
    empty.receipts.clear();
    empty.checkpoint = AuditCheckpoint::at_head(&[]);
    cases.push((
        "empty, anchored at genesis",
        empty.clone(),
        Some(AuditCheckpoint::at_head(&[])),
    ));
    cases.push(("empty, anchored later", empty, Some(anchor)));

    let orders: [&[&str]; 3] = [
        &["version", "audit", "receipts", "checkpoint"],
        &["checkpoint", "version", "audit", "receipts"],
        &["receipts", "checkpoint", "audit", "version"],
    ];
    for (name, bundle, anchor) in &cases {
        let whole = reference_verify(bundle, anchor.as_ref());
        assert_eq!(verify_evidence(bundle, anchor.as_ref()), whole, "{name}");
        for order in orders {
            let streamed = stream(&bundle_text(bundle, order), anchor.as_ref())
                .unwrap_or_else(|e| panic!("{name} {order:?}: {e}"));
            assert_eq!(streamed, whole, "{name}, members in order {order:?}");
        }
    }
}

#[test]
fn the_streaming_verifier_refuses_files_that_are_not_one_whole_bundle() {
    let journal = Journal::open_in_memory().unwrap();
    created(&journal);
    let bundle = journal.export_evidence().unwrap();
    let all = ["version", "audit", "receipts", "checkpoint"];
    let text = bundle_text(&bundle, &all);
    assert!(stream(&text, None).is_ok());
    for member in all {
        let rest: Vec<&str> = all.iter().copied().filter(|m| *m != member).collect();
        let error = stream(&bundle_text(&bundle, &rest), None).unwrap_err();
        assert!(error.contains(member), "missing {member}: {error}");
    }
    let duplicated = text.replacen('{', "{\"version\":1,", 1);
    assert!(stream(&duplicated, None).unwrap_err().contains("duplicate"));
    let extra = text.replacen('{', "{\"extra\":1,", 1);
    assert!(stream(&extra, None).unwrap_err().contains("unknown field"));
    assert!(stream(&format!("{text} 1"), None).is_err(), "trailing data");
    assert!(stream(&text[..text.len() - 1], None).is_err(), "cut short");
    assert!(stream("[]", None).is_err());
    assert!(stream("", None).is_err());
    let unknown_record_member = text.replacen("\"seq\"", "\"extra\":0,\"seq\"", 1);
    assert!(stream(&unknown_record_member, None).is_err());
}

#[test]
fn a_wholly_corrupt_log_lists_a_bounded_number_of_errors() {
    let journal = Journal::open_in_memory().unwrap();
    created(&journal);
    for i in 0..1_500 {
        journal
            .append_audit(Timestamp::from_millis(i + 2), "test", "s", &json!({}))
            .unwrap();
    }
    let mut bundle = journal.export_evidence().unwrap();
    for record in &mut bundle.audit {
        record.hash = "0".repeat(64);
    }
    bundle.checkpoint = AuditCheckpoint::at_head(&bundle.audit);
    let result = verify_evidence(&bundle, None);
    assert!(!result.valid);
    assert_eq!(result.records, bundle.audit.len());
    assert!(result.records > 1_500);
    assert!(result.errors.len() <= 1_003, "{}", result.errors.len());
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.contains("more invalid audit records not listed"))
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
