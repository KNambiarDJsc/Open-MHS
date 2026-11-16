//! Deployment intelligence over recorded robot events (RFC 0005, ADR-0026 to ADR-0034).
//!
//! Observation only: this crate reads events, manifests and artifact references and derives task attempts, incidents
//! and release comparisons. Nothing here can command a robot or a device.
//!
//! - [`Store`]: one SQLite file with numbered migrations; ingestion is idempotent per event id, and a second record
//!   with the same id and different content is kept as a conflict, never overwritten.
//! - [`Store::rebuild`]: replays every event through the correlation rules ([`RULES_VERSION`]). The result depends
//!   only on the set of stored events, not on the order they arrived in.
//! - [`stats`]: Wilson and Newcombe intervals and the Mantel-Haenszel risk difference used by comparisons.

mod derive;
mod query;
pub mod stats;
mod store;

pub use derive::{RULES_VERSION, RebuildReport};
pub use query::{
    Arm, CompareSpec, Comparison, IncidentDetail, IncidentSummary, Member, OutcomeKind, Paired,
    Stratum, Verdict,
};
pub use store::{IngestResult, IngestStatus, Store};

#[derive(Debug, thiserror::Error)]
pub enum OpsError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Store(String),
}
