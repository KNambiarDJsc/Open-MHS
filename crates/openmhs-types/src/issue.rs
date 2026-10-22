//! Validation issues with stable codes (spec/errors.md).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    /// Stable dotted error code, e.g. `descriptor.unit.unknown`.
    pub code: String,
    /// JSON Pointer to the offending location (`""` for the document root).
    pub pointer: String,
    pub message: String,
}

impl Issue {
    pub fn new(
        code: impl Into<String>,
        pointer: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            pointer: pointer.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let at = if self.pointer.is_empty() {
            "/"
        } else {
            &self.pointer
        };
        write!(f, "[{}] at {}: {}", self.code, at, self.message)
    }
}
