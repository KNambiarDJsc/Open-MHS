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

/// What a command's latest audit record says about its receipt.
enum Binding {
    /// The record's data is not JSON.
    Invalid,
    /// A legacy record: no `receipt_digest`.
    Unbound,
    /// The digest the record commits to.
    Digest {
        version_is_1: bool,
        sha256: Option<String>,
    },
}

impl Binding {
    fn of(data: &str) -> Self {
        match serde_json::from_str::<serde_json::Value>(data) {
            Err(_) => Self::Invalid,
            Ok(data) => match data.get("receipt_digest") {
                None => Self::Unbound,
                Some(digest) => Self::Digest {
                    version_is_1: digest["version"] == 1,
                    sha256: digest["sha256"].as_str().map(str::to_owned),
                },
            },
        }
    }
}

struct Latest {
    hash: String,
    binding: Binding,
}

/// What a receipt must be checked against once the whole chain has been seen.
struct ReceiptFacts {
    id: String,
    audit_hash: Option<String>,
    digest: Option<String>,
}

/// Errors listed in full; past this the rest are counted, so a wholly corrupt log of
/// millions of records cannot make the verifier's own output unbounded.
const MAX_ERRORS: usize = 1000;

/// The verifier as a state machine fed one record at a time, so a long chain is checked
/// without being held. It keeps only what the checks need, one small entry per command
/// and per receipt, never the records. `verify_evidence` and the CLI's file verifier
/// both run on it.
///
/// Records and receipts may arrive in any order, as JSON object members may; the
/// comparisons between them happen in [`finish`](Self::finish).
pub struct EvidenceVerifier<'a> {
    anchor: Option<&'a AuditCheckpoint>,
    version: Option<u32>,
    checkpoint: Option<AuditCheckpoint>,
    audit_errors: Vec<String>,
    audit_error_count: usize,
    records: usize,
    head: AuditCheckpoint,
    anchor_found: bool,
    latest: BTreeMap<String, Latest>,
    created: BTreeSet<String>,
    receipts: Vec<ReceiptFacts>,
}

impl<'a> EvidenceVerifier<'a> {
    pub fn new(anchor: Option<&'a AuditCheckpoint>) -> Self {
        Self {
            anchor,
            version: None,
            checkpoint: None,
            audit_errors: Vec::new(),
            audit_error_count: 0,
            records: 0,
            head: AuditCheckpoint {
                seq: 0,
                hash: GENESIS.into(),
            },
            anchor_found: false,
            latest: BTreeMap::new(),
            created: BTreeSet::new(),
            receipts: Vec::new(),
        }
    }

    pub fn version(&mut self, version: u32) {
        self.version = Some(version);
    }

    pub fn checkpoint(&mut self, checkpoint: AuditCheckpoint) {
        self.checkpoint = Some(checkpoint);
    }

    fn audit_error(&mut self, message: String) {
        self.audit_error_count += 1;
        if self.audit_errors.len() < MAX_ERRORS {
            self.audit_errors.push(message);
        }
    }

    /// The next audit record, in chain order.
    pub fn audit(&mut self, record: &AuditRecord) {
        self.records += 1;
        let expected = i64::try_from(self.records).unwrap_or(i64::MAX);
        if record.seq != expected
            || record.prev_hash != self.head.hash
            || record.hash
                != audit_hash(
                    &self.head.hash,
                    record.seq,
                    record.at_ms,
                    &record.kind,
                    &record.subject,
                    &record.data,
                )
        {
            self.audit_error(format!("invalid audit record at sequence {}", record.seq));
        }
        self.head.seq = record.seq;
        self.head.hash.clone_from(&record.hash);
        if let Some(anchor) = self.anchor
            && record.seq == anchor.seq
            && record.hash == anchor.hash
        {
            self.anchor_found = true;
        }
        if record.kind == "command.created" && !self.created.insert(record.subject.clone()) {
            self.audit_error(format!("duplicate command creation: {}", record.subject));
        }
        if matches!(
            record.kind.as_str(),
            "command.created" | "command.transition"
        ) {
            self.latest.insert(
                record.subject.clone(),
                Latest {
                    hash: record.hash.clone(),
                    binding: Binding::of(&record.data),
                },
            );
        }
    }

    /// The next receipt.
    pub fn receipt(&mut self, receipt: &Receipt) {
        self.receipts.push(ReceiptFacts {
            id: receipt.command_id.clone(),
            audit_hash: receipt.audit_hash.clone(),
            digest: receipt_digest(receipt).ok(),
        });
    }

    /// Compare everything seen and report.
    pub fn finish(self) -> EvidenceVerification {
        let mut errors = Vec::new();
        let mut unbound_commands = Vec::new();
        match self.version {
            Some(1) => {}
            Some(version) => errors.push(format!("unsupported evidence version {version}")),
            None => errors.push("bundle has no version".into()),
        }
        errors.extend(self.audit_errors);
        let hidden = self.audit_error_count.saturating_sub(MAX_ERRORS);
        if hidden > 0 {
            errors.push(format!("{hidden} more invalid audit records not listed"));
        }
        if self.checkpoint.as_ref() != Some(&self.head) {
            errors.push("bundle checkpoint does not match audit head".into());
        }
        if let Some(anchor) = self.anchor {
            let matches = if anchor.seq == 0 {
                anchor.hash == GENESIS
            } else {
                self.anchor_found
            };
            if !matches {
                errors.push("independent checkpoint is missing or does not match".into());
            }
        }
        let mut seen = BTreeSet::new();
        for receipt in &self.receipts {
            let id = receipt.id.as_str();
            if !seen.insert(id) {
                errors.push(format!("duplicate receipt: {id}"));
            }
            if !self.created.contains(id) {
                errors.push(format!("receipt has no command creation record: {id}"));
            }
            let Some(record) = self.latest.get(id) else {
                continue;
            };
            if receipt.audit_hash.as_deref() != Some(record.hash.as_str()) {
                errors.push(format!(
                    "receipt audit pointer differs from latest record: {id}"
                ));
            }
            match &record.binding {
                Binding::Invalid => errors.push(format!("invalid command audit JSON: {id}")),
                Binding::Unbound => unbound_commands.push(id.to_owned()),
                Binding::Digest {
                    version_is_1,
                    sha256,
                } => {
                    if !version_is_1 || receipt.digest != *sha256 {
                        errors.push(format!("receipt digest mismatch: {id}"));
                    }
                }
            }
        }
        for id in self.latest.keys() {
            if !seen.contains(id.as_str()) {
                errors.push(format!("missing receipt: {id}"));
            }
        }
        EvidenceVerification {
            valid: errors.is_empty(),
            complete: errors.is_empty() && unbound_commands.is_empty(),
            records: self.records,
            receipts: self.receipts.len(),
            unbound_commands,
            errors,
        }
    }
}

/// Verify a complete exported chain and its current receipts. `anchor`, when present,
/// must have been retained independently before this export; the bundle's own head is
/// only a completeness check, not an external trust root.
pub fn verify_evidence(
    bundle: &EvidenceBundle,
    anchor: Option<&AuditCheckpoint>,
) -> EvidenceVerification {
    let mut verifier = EvidenceVerifier::new(anchor);
    verifier.version(bundle.version);
    verifier.checkpoint(bundle.checkpoint.clone());
    bundle.audit.iter().for_each(|r| verifier.audit(r));
    bundle.receipts.iter().for_each(|r| verifier.receipt(r));
    verifier.finish()
}

/// Verify an evidence bundle read from JSON without holding it: the same checks as
/// [`verify_evidence`] on the same bytes, with memory that grows with the number of
/// commands, not with the length of the audit log. The bundle must be one JSON object
/// with exactly the members of [`EvidenceBundle`], in any order.
pub fn verify_evidence_reader(
    reader: impl std::io::Read,
    anchor: Option<&AuditCheckpoint>,
) -> Result<EvidenceVerification, serde_json::Error> {
    use serde::de::DeserializeSeed;
    let mut verifier = EvidenceVerifier::new(anchor);
    let mut de = serde_json::Deserializer::from_reader(reader);
    stream::Bundle(&mut verifier).deserialize(&mut de)?;
    de.end()?;
    Ok(verifier.finish())
}

mod stream {
    use std::fmt;

    use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};

    use super::{AuditCheckpoint, AuditRecord, EvidenceVerifier};
    use crate::Receipt;

    const MEMBERS: [&str; 4] = ["version", "audit", "receipts", "checkpoint"];

    /// The bundle object: each member is handed to the verifier as it is parsed.
    pub struct Bundle<'v, 'a>(pub &'v mut EvidenceVerifier<'a>);

    impl<'de> DeserializeSeed<'de> for Bundle<'_, '_> {
        type Value = ();
        fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
            d.deserialize_struct("EvidenceBundle", &MEMBERS, self)
        }
    }

    impl<'de> Visitor<'de> for Bundle<'_, '_> {
        type Value = ();
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an evidence bundle")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
            let mut seen = [false; 4];
            while let Some(key) = map.next_key::<String>()? {
                let Some(index) = MEMBERS.iter().position(|m| *m == key) else {
                    return Err(de::Error::unknown_field(&key, &MEMBERS));
                };
                if std::mem::replace(&mut seen[index], true) {
                    return Err(de::Error::duplicate_field(MEMBERS[index]));
                }
                match index {
                    0 => self.0.version(map.next_value()?),
                    1 => map.next_value_seed(Records(self.0))?,
                    2 => map.next_value_seed(Receipts(self.0))?,
                    _ => self.0.checkpoint(map.next_value::<AuditCheckpoint>()?),
                }
            }
            match seen.iter().position(|s| !s) {
                Some(missing) => Err(de::Error::missing_field(MEMBERS[missing])),
                None => Ok(()),
            }
        }
    }

    struct Records<'v, 'a>(&'v mut EvidenceVerifier<'a>);

    impl<'de> DeserializeSeed<'de> for Records<'_, '_> {
        type Value = ();
        fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
            d.deserialize_seq(self)
        }
    }

    impl<'de> Visitor<'de> for Records<'_, '_> {
        type Value = ();
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a list of audit records")
        }
        fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<(), S::Error> {
            while let Some(record) = seq.next_element::<AuditRecord>()? {
                self.0.audit(&record);
            }
            Ok(())
        }
    }

    struct Receipts<'v, 'a>(&'v mut EvidenceVerifier<'a>);

    impl<'de> DeserializeSeed<'de> for Receipts<'_, '_> {
        type Value = ();
        fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
            d.deserialize_seq(self)
        }
    }

    impl<'de> Visitor<'de> for Receipts<'_, '_> {
        type Value = ();
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a list of receipts")
        }
        fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<(), S::Error> {
            while let Some(receipt) = seq.next_element::<Receipt>()? {
                self.0.receipt(&receipt);
            }
            Ok(())
        }
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
