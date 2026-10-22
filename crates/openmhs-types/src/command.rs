//! Command requests (`command.schema.json`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::descriptor::Operation;
use crate::value::Value;

/// Deadline applied when a request does not set `deadline_ms`.
pub const DEFAULT_DEADLINE_MS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub device: String,
    pub capability: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestOperation {
    Read,
    Write,
    Invoke,
}

impl From<RequestOperation> for Operation {
    fn from(op: RequestOperation) -> Self {
        match op {
            RequestOperation::Read => Operation::Read,
            RequestOperation::Write => Operation::Write,
            RequestOperation::Invoke => Operation::Invoke,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRequest {
    pub target: Target,
    pub operation: RequestOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
}

impl CommandRequest {
    pub fn deadline_ms(&self) -> u64 {
        self.deadline_ms.unwrap_or(DEFAULT_DEADLINE_MS)
    }
}
