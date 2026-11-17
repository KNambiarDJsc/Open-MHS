//! Deployment events (RFC 0005): the envelope, deployment manifests and artifact references.
//!
//! Records arrive as JSON. This crate validates them, keeps the original content in canonical form (so unknown fields
//! and extensions survive untouched), and extracts the few fields that correlation needs. It depends on nothing in the
//! gateway domain, so any program that emits events can use it.

mod canonical;
mod conditions;
mod error;
mod event;
mod manifest;
mod time_fmt;

pub use canonical::{canonical_json, sha256_digest};
pub use conditions::{Miss, Side, VerifiedCondition, parse_conditions, quantity};
pub use error::RecordError;
pub use event::{Artifact, Event, EvidenceRef, Relation};
pub use manifest::{ComponentChange, Manifest, diff};
pub use time_fmt::{format_ns, parse_rfc3339_ns};

use serde_json::Value;

/// Largest serialized event, and largest `ext` object inside it (RFC 0005 §1.2).
pub const MAX_EVENT_BYTES: usize = 64 * 1024;
pub const MAX_EXT_BYTES: usize = 32 * 1024;

/// One input line: an event, a deployment manifest, or an artifact reference, told apart by `schema`.
#[derive(Debug, Clone, PartialEq)]
pub enum Record {
    Event(Box<Event>),
    Manifest(Manifest),
    Artifact(Artifact),
}

/// Parse and validate one record of any kind.
pub fn parse_record(value: &Value) -> Result<Record, RecordError> {
    let schema = value
        .get("schema")
        .and_then(Value::as_str)
        .ok_or_else(|| RecordError::new("record.schema", "`schema` is required"))?;
    if schema.starts_with("openmhs.event/") {
        Event::parse(value).map(|e| Record::Event(Box::new(e)))
    } else if schema.starts_with("openmhs.deployment/") {
        Manifest::parse(value).map(Record::Manifest)
    } else if schema.starts_with("openmhs.artifact/") {
        Artifact::parse(value).map(Record::Artifact)
    } else {
        Err(RecordError::new(
            "record.schema",
            format!(
                "unknown schema {schema:?}; expected openmhs.event/, openmhs.deployment/ or openmhs.artifact/"
            ),
        ))
    }
}

/// Major version 0 is the only one this crate reads; minor versions only add fields.
pub(crate) fn check_major(schema: &str, prefix: &str) -> Result<(), RecordError> {
    let version = &schema[prefix.len()..];
    let major = version.split('.').next().unwrap_or_default();
    if major == "0" && version.contains('.') {
        Ok(())
    } else {
        Err(RecordError::new(
            "record.schema_version",
            format!("{schema:?}: only {prefix}0.x is understood"),
        ))
    }
}
