//! OpenMHS engine: the trusted core that the gateway runs (ADR-0001).
//!
//! It owns the device registry, the command lifecycle (spec/lifecycle.md), the durable
//! journal, the state cache and the audit log. Every write or invoke goes through
//! [`Engine::submit`]; nothing else calls an adapter's `execute`.
//!
//! The engine contains no device-specific logic; devices are described by descriptors
//! and driven by adapters.

mod engine;
mod error;
pub mod journal;

pub use engine::{
    DEFAULT_APPROVAL_TTL_MS, Engine, EngineOptions, LeaseRequest, MAX_APPROVAL_TTL_MS,
    MAX_CONCURRENT_PROBES, MAX_PROBE_BACKOFF_MS, RecoveryReport, StorageLimits, fingerprint,
    probe_backoff_ms,
};
pub use error::EngineError;
pub use journal::{AuditRecord, AuditVerification, Journal, JournalError};
pub use openmhs_types::graph::{Claim, Graph};
