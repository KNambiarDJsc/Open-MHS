use std::fmt;

/// Why a record was refused: a stable code and a sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordError {
    pub code: &'static str,
    pub message: String,
}

impl RecordError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RecordError {}
