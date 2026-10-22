//! Wire error object (`common.schema.json#/$defs/error`, spec/errors.md).

use serde::{Deserialize, Serialize};

use crate::issue::Issue;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorObject {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Map<String, serde_json::Value>>,
}

impl ErrorObject {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            pointer: None,
            rule_id: None,
            details: None,
        }
    }

    pub fn at(mut self, pointer: impl Into<String>) -> Self {
        self.pointer = Some(pointer.into());
        self
    }

    pub fn rule(mut self, rule_id: impl Into<String>) -> Self {
        self.rule_id = Some(rule_id.into());
        self
    }

    pub fn detail(mut self, key: &str, value: impl Into<serde_json::Value>) -> Self {
        self.details
            .get_or_insert_with(Default::default)
            .insert(key.to_owned(), value.into());
        self
    }
}

impl From<Issue> for ErrorObject {
    fn from(i: Issue) -> Self {
        let e = ErrorObject::new(i.code, i.message);
        if i.pointer.is_empty() {
            e
        } else {
            e.at(i.pointer)
        }
    }
}

impl std::fmt::Display for ErrorObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}
