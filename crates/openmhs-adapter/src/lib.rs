//! The adapter contract.
//!
//! An adapter translates between the OpenMHS model and one device or protocol. It is only
//! ever called by the gateway's engine, after authorization and validation. Adapters must
//! not retry state-changing commands on their own and must not report success they have
//! not observed — see `spec/conformance.md`.

use std::collections::BTreeMap;

use async_trait::async_trait;
use openmhs_types::descriptor::Operation;
use openmhs_types::{Clock, StateValue, Timestamp, Value};
use tokio::sync::mpsc;

/// A validated, unit-normalized command handed to an adapter.
///
/// Values and parameters are already converted to the units declared in the descriptor,
/// so adapters never perform unit conversion.
#[derive(Debug, Clone, PartialEq)]
pub struct AdapterCommand {
    pub command_id: String,
    pub capability: String,
    /// `Write` or `Invoke`.
    pub operation: Operation,
    /// The value to write (`Write` only).
    pub value: Option<Value>,
    /// Action parameters (`Invoke` only).
    pub params: BTreeMap<String, Value>,
    /// Whether the descriptor declares this capability idempotent.
    pub idempotent: bool,
    /// The engine stops waiting at this time.
    pub deadline: Timestamp,
}

/// What the device reported for a command.
#[derive(Debug, Clone, PartialEq)]
pub enum ExecuteReport {
    /// The device reports the change applied. Optional named results for actions.
    Applied { result: BTreeMap<String, Value> },
    /// The device refused the command; nothing was applied.
    Rejected { reason: String },
}

/// Classified adapter failure. The class decides the command's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterErrorKind {
    /// No response in time. The command may or may not have taken effect.
    Timeout,
    /// Connection lost. The command may or may not have taken effect.
    Disconnected,
    /// The device answered with something that could not be interpreted.
    MalformedResponse,
    /// The device reported it could not carry out the command.
    DeviceFailure,
    /// The adapter guarantees nothing was sent to the device.
    NotSent,
    /// The capability or operation is not implemented by this adapter.
    Unsupported,
    /// A bug or crash inside the adapter.
    Internal,
}

impl AdapterErrorKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::Timeout => "adapter.timeout",
            Self::Disconnected => "adapter.disconnected",
            Self::MalformedResponse => "adapter.malformed_response",
            Self::DeviceFailure => "adapter.device_failure",
            Self::NotSent => "adapter.not_sent",
            Self::Unsupported => "adapter.unsupported",
            Self::Internal => "adapter.internal",
        }
    }

    /// Whether a command that failed this way might still have changed the device.
    pub fn outcome_uncertain(self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::Disconnected | Self::MalformedResponse | Self::Internal
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterError {
    pub kind: AdapterErrorKind,
    pub message: String,
}

impl AdapterError {
    pub fn new(kind: AdapterErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.kind.code(), self.message)
    }
}

impl std::error::Error for AdapterError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    Healthy,
    Degraded(String),
    Disconnected(String),
}

/// Progress events an adapter reports while executing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressEvent {
    /// The device acknowledged the command (lifecycle `ACCEPTED`). `external_ref` is an
    /// opaque identifier the device or its server gave the work (for example a LabThings
    /// invocation id), kept in the receipt so an unresolved command can be matched to it
    /// later. It is the adapter's text, not the gateway's: treat it as untrusted.
    Accepted { external_ref: Option<String> },
}

/// The longest external reference kept.
pub const MAX_EXTERNAL_REF: usize = 256;

/// Handle an adapter uses to report progress during [`HardwareAdapter::execute`].
#[derive(Debug, Clone)]
pub struct Progress {
    tx: mpsc::UnboundedSender<ProgressEvent>,
}

impl Progress {
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<ProgressEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    /// Report that the device acknowledged the command.
    pub fn accepted(&self) {
        // The engine may already have stopped listening (e.g. after a timeout); that is fine.
        let _ = self.tx.send(ProgressEvent::Accepted { external_ref: None });
    }

    /// As [`Progress::accepted`], recording `external_ref` (at most [`MAX_EXTERNAL_REF`]
    /// characters, no control characters; anything else is dropped, not truncated).
    pub fn accepted_with_ref(&self, external_ref: impl Into<String>) {
        let r: String = external_ref.into();
        let ok = !r.is_empty()
            && r.chars().count() <= MAX_EXTERNAL_REF
            && !r.chars().any(char::is_control);
        let _ = self.tx.send(ProgressEvent::Accepted {
            external_ref: ok.then_some(r),
        });
    }
}

#[async_trait]
pub trait HardwareAdapter: Send + Sync + 'static {
    /// Must equal the descriptor's `adapter.id`.
    fn adapter_id(&self) -> &str;

    /// The device descriptor this adapter serves (JSON, validated by the engine).
    async fn describe(&self) -> Result<serde_json::Value, AdapterError>;

    async fn connect(&self) -> Result<(), AdapterError>;

    async fn health(&self) -> Health;

    /// Read the current value of a readable property, with timestamp and quality.
    async fn read(&self, capability: &str) -> Result<StateValue, AdapterError>;

    /// Carry out a write or invoke. Call `progress.accepted()` when the device acknowledges.
    /// Return only what the device reported; never retry a non-idempotent command.
    async fn execute(
        &self,
        command: AdapterCommand,
        progress: Progress,
    ) -> Result<ExecuteReport, AdapterError>;

    async fn shutdown(&self) -> Result<(), AdapterError>;
}

/// A clock driven by Tokio's timer, so tests that pause Tokio time control it too.
#[derive(Debug, Clone)]
pub struct TokioClock {
    epoch: Timestamp,
    start: tokio::time::Instant,
}

impl TokioClock {
    /// Starts at the current wall-clock time.
    pub fn new() -> Self {
        Self::starting_at(openmhs_types::SystemClock.now())
    }

    /// Starts at a fixed time; useful for reproducible tests.
    pub fn starting_at(epoch: Timestamp) -> Self {
        Self {
            epoch,
            start: tokio::time::Instant::now(),
        }
    }
}

impl Default for TokioClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for TokioClock {
    fn now(&self) -> Timestamp {
        let elapsed = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.epoch.plus_ms(elapsed)
    }
}
