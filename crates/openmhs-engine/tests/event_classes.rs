//! Event classes (RFC 0002, milestone 6; strategy RFC §3): every audit kind the code writes
//! belongs to a class, and the class decides what may ever be aggregated.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use openmhs_types::ApprovalStatus;
use openmhs_types::audit_class::{EventClass, class_of, classify, known_kinds};
use openmhs_types::plan::PlanState;

fn source_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.map(|e| e.unwrap().path()) {
        if entry.is_dir() {
            source_files(&entry, out);
        } else if entry.extension().is_some_and(|e| e == "rs") {
            out.push(entry);
        }
    }
}

/// The first string literal among a call's arguments: `from` is just after its opening
/// parenthesis, and the search ends at the matching closing one.
fn first_literal(text: &str, from: usize) -> Option<&str> {
    let rest = &text[from..];
    let mut depth = 1;
    for (at, c) in rest.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return None;
                }
            }
            '"' => {
                let end = rest[at + 1..].find('"')?;
                return Some(&rest[at + 1..at + 1 + end]);
            }
            _ => {}
        }
    }
    None
}

fn looks_like_a_kind(literal: &str) -> bool {
    !literal.is_empty()
        && literal
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_')
        && (literal.contains('.') || literal == "read")
}

/// Every audit kind written as a literal in the workspace's library code: the first literal
/// after each call that appends an audit record.
fn kinds_in_the_code() -> BTreeSet<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for group in ["crates", "adapters"] {
        for member in std::fs::read_dir(root.join(group)).unwrap() {
            source_files(&member.unwrap().path().join("src"), &mut files);
        }
    }
    let calls = ["append_audit(", "append_audit_tx(", ".audit(", "save_plan("];
    let mut kinds = BTreeSet::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        for call in calls {
            for (at, _) in text.match_indices(call) {
                if let Some(literal) = first_literal(&text, at + call.len())
                    && looks_like_a_kind(literal)
                {
                    kinds.insert(literal.to_owned());
                }
            }
        }
    }
    kinds
}

#[test]
fn every_audit_kind_the_code_writes_is_classified() {
    let kinds = kinds_in_the_code();
    // The scan must actually find the code's kinds, or this test proves nothing.
    for expected in [
        "command.created",
        "command.transition",
        "read",
        "read.denied",
        "request.rejected",
        "device.registered",
        "gateway.started",
        "policy.updated",
        "token.created",
        "lease.acquired",
        "plan.submitted",
        "watch.created",
        "sim.fault",
    ] {
        assert!(
            kinds.contains(expected),
            "the scan did not find {expected}: {kinds:?}"
        );
    }
    assert!(
        kinds.len() >= 25,
        "only {} kinds found: {kinds:?}",
        kinds.len()
    );
    let unclassified: Vec<&String> = kinds.iter().filter(|k| classify(k).is_none()).collect();
    assert!(
        unclassified.is_empty(),
        "audit kinds with no event class (add them to openmhs_types::audit_class): {unclassified:?}"
    );
}

#[test]
fn generated_kinds_are_classified_by_their_family() {
    for state in PlanState::ALL {
        let kind = format!("plan.{}", state.name().to_lowercase());
        assert!(classify(&kind).is_some(), "{kind}");
    }
    for status in [
        ApprovalStatus::Pending,
        ApprovalStatus::Approved,
        ApprovalStatus::Denied,
        ApprovalStatus::Expired,
        ApprovalStatus::Revoked,
        ApprovalStatus::Consumed,
    ] {
        let kind = format!("approval.{}", status.name().to_lowercase());
        assert_eq!(class_of(&kind), EventClass::Critical, "{kind}");
    }
    for kind in [
        "plan.step.started",
        "lease.renewed",
        "lease.released",
        "approval.requested",
    ] {
        assert_eq!(class_of(kind), EventClass::Critical, "{kind}");
    }
}

#[test]
fn the_classes_are_the_strategy_rfcs() {
    let expected = [
        ("command.created", EventClass::Critical),
        ("command.transition", EventClass::Critical),
        ("gateway.recovered", EventClass::Critical),
        ("sim.fault", EventClass::Critical),
        ("read.denied", EventClass::Security),
        ("request.rejected", EventClass::Security),
        ("credential.rejected", EventClass::Security),
        ("grant.added", EventClass::Security),
        ("policy.updated", EventClass::Security),
        ("token.revoked", EventClass::Security),
        ("topology.set", EventClass::Security),
        ("gateway.started", EventClass::Operational),
        ("gateway.stopped", EventClass::Operational),
        ("device.registered", EventClass::Operational),
        ("device.availability", EventClass::Operational),
        ("command.preflight", EventClass::Operational),
        ("plan.preflight", EventClass::Operational),
        ("read", EventClass::Telemetry),
    ];
    for (kind, class) in expected {
        assert_eq!(class_of(kind), class, "{kind}");
    }
}

#[test]
fn an_unknown_kind_is_kept_in_full() {
    assert_eq!(classify("something.new"), None);
    assert_eq!(class_of("something.new"), EventClass::Critical);
    assert!(!class_of("something.new").may_be_aggregated());
    // Only a read can ever be aggregated; nothing a command, approval, plan, lease, refusal
    // or credential leaves is.
    for kind in known_kinds() {
        assert_eq!(class_of(kind).may_be_aggregated(), kind == "read", "{kind}");
    }
}
