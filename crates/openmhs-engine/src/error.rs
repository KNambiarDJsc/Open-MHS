use openmhs_adapter::AdapterError;
use openmhs_types::{CommandState, ErrorObject, Issue};

use crate::journal::JournalError;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The request does not match `command.schema.json`. No command was created.
    #[error("invalid request: {}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    InvalidRequest(Vec<ErrorObject>),
    /// Reads go through [`crate::Engine::read`]; they are not commands.
    #[error("reads are not commands; use read")]
    NotACommand,
    #[error("idempotency key reused with a different request (existing command {0})")]
    IdempotencyConflict(String),
    #[error("unknown command {0}")]
    UnknownCommand(String),
    #[error("command {id} is {state:?}; cannot {action}")]
    InvalidState {
        id: String,
        state: CommandState,
        action: &'static str,
    },
    /// Refused by policy or validation (reads, approvals, cancellations).
    #[error("{0}")]
    Denied(ErrorObject),
    #[error("invalid descriptor: {}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    InvalidDescriptor(Vec<Issue>),
    #[error("device {0} is already registered")]
    DuplicateDevice(String),
    #[error("adapter: {0}")]
    Adapter(AdapterError),
    #[error("cannot reconcile: {0}")]
    NotReconcilable(String),
    #[error("journal: {0}")]
    Journal(#[from] JournalError),
}

impl EngineError {
    /// Stable error code for API responses.
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidRequest(_) => "command.schema",
            Self::NotACommand => "command.not_a_command",
            Self::IdempotencyConflict(_) => "command.idempotency.conflict",
            Self::UnknownCommand(_) => "command.unknown",
            Self::InvalidState { .. } => "command.invalid_state",
            Self::Denied(e) => &e.code,
            Self::InvalidDescriptor(_) => "registry.invalid_descriptor",
            Self::DuplicateDevice(_) => "registry.duplicate_device",
            Self::Adapter(e) => e.kind.code(),
            Self::NotReconcilable(_) => "command.not_reconcilable",
            Self::Journal(_) => "gateway.journal",
        }
    }
}
