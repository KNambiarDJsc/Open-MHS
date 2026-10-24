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
    DEFAULT_APPROVAL_TTL_MS, Engine, EngineOptions, MAX_APPROVAL_TTL_MS, RecoveryReport,
    fingerprint,
};
pub use error::EngineError;
pub use journal::{AuditRecord, AuditVerification, Journal, JournalError};
