//! Typed values as they appear on the wire (`common.schema.json#/$defs/value`).

use serde::{Deserialize, Serialize};

/// A number with an explicit unit. Bare numbers are never interpreted as quantities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quantity {
    pub value: f64,
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Quantity(Quantity),
    Integer(i64),
    Bool(bool),
    Text(String),
}

impl Value {
    pub fn kind(&self) -> &'static str {
        match self {
            Value::Quantity(_) => "quantity",
            Value::Integer(_) => "integer",
            Value::Bool(_) => "boolean",
            Value::Text(_) => "string",
        }
    }
}
