//! Portable audit evidence and an offline verifier. No gateway or device is contacted.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Receipt, canonical_json};

pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditRecord {
    pub seq: i64,
    pub at_ms: i64,
    pub kind: String,
    pub subject: String,
    /// The original JSON bytes are retained because the chain covers those exact bytes.
    pub data: String,
    pub prev_hash: String,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditCheckpoint {
    pub seq: i64,
    pub hash: String,
}

impl AuditCheckpoint {
    pub fn at_head(records: &[AuditRecord]) -> Self {
        records.last().map_or(
            Self {
                seq: 0,
                hash: GENESIS.into(),
            },
            |record| Self {
                seq: record.seq,
                hash: record.hash.clone(),
            },
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceBundle {
    pub version: u32,
    pub audit: Vec<AuditRecord>,
    pub receipts: Vec<Receipt>,
    pub checkpoint: AuditCheckpoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceVerification {
    pub valid: bool,
    /// False for legacy commands whose last audit event has no receipt digest.
    pub complete: bool,
    pub records: usize,
    pub receipts: usize,
    pub unbound_commands: Vec<String>,
    pub errors: Vec<String>,
}

pub fn audit_hash(
    prev: &str,
    seq: i64,
    at_ms: i64,
    kind: &str,
    subject: &str,
    data: &str,
) -> String {
    // The SHA-256 of "{prev}\n{seq}\n{at_ms}\n{kind}\n{subject}\n{data}", fed in pieces: a
    // gateway verifies every record at start, so this avoids building the string.
    let mut h = Sha256::new();
    for part in [
        prev.as_bytes(),
        b"\n",
        seq.to_string().as_bytes(),
        b"\n",
        at_ms.to_string().as_bytes(),
        b"\n",
        kind.as_bytes(),
        b"\n",
        subject.as_bytes(),
        b"\n",
        data.as_bytes(),
    ] {
        h.update(part);
    }
    hex(&h.finalize())
}

fn sha256(data: &str) -> String {
    hex(&Sha256::digest(data.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    out
}

/// Version 1: SHA-256 of canonical receipt JSON with only `audit_hash` removed. That
/// pointer names the record holding this digest, so including it would be circular.
pub fn receipt_digest(receipt: &Receipt) -> Result<String, serde_json::Error> {
    let mut value = serde_json::to_value(receipt)?;
    if let Some(object) = value.as_object_mut() {
        object.remove("audit_hash");
    }
    Ok(sha256(&canonical_json(&value)))
}

/// Verify a complete exported chain and its current receipts. `anchor`, when present,
/// must have been retained independently before this export; the bundle's own head is
/// only a completeness check, not an external trust root.
pub fn verify_evidence(
    bundle: &EvidenceBundle,
    anchor: Option<&AuditCheckpoint>,
) -> EvidenceVerification {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The streamed hash is byte for byte the one every existing journal and export was
    /// written with: SHA-256 of the fields joined by newlines, in lowercase hex.
    #[test]
    fn audit_hash_is_unchanged_from_the_joined_string() {
        let cases = [
            (
                GENESIS,
                1,
                1_800_000_000_000,
                "command.created",
                "cmd-1",
                r#"{"a":1}"#,
            ),
            ("ab", -5, 0, "", "", ""),
            ("x", i64::MAX, i64::MIN, "k\nind", "subj ect", "d\u{e9}ta\n"),
        ];
        for (prev, seq, at, kind, subject, data) in cases {
            let joined = format!("{prev}\n{seq}\n{at}\n{kind}\n{subject}\n{data}");
            let expected: String = Sha256::digest(joined.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert_eq!(audit_hash(prev, seq, at, kind, subject, data), expected);
            assert_eq!(sha256(&joined), expected);
        }
    }
}
