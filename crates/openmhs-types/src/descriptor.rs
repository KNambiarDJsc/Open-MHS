//! Device descriptor types (spec/descriptor.md). Mirrors `descriptor.schema.json`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::value::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub openmhs: String,
    pub device: DeviceInfo,
    pub adapter: AdapterBinding,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<Component>,
    pub capabilities: Vec<Capability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<serde_json::Map<String, serde_json::Value>>,
}

impl Descriptor {
    pub fn capability(&self, name: &str) -> Option<&Capability> {
        self.capabilities.iter().find(|c| c.name == name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub manufacturer: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterBinding {
    pub id: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Map<String, serde_json::Value>>,
    /// The conformance level the adapter's author claims (spec/conformance.md). A claim,
    /// not evidence: the conformance suite's report is the evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conformance: Option<ConformanceLevel>,
}

/// Cumulative conformance levels (spec/conformance.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ConformanceLevel {
    L0,
    L1,
    L2,
    L3,
}

impl ConformanceLevel {
    pub const ALL: [ConformanceLevel; 4] = [Self::L0, Self::L1, Self::L2, Self::L3];

    pub fn name(self) -> &'static str {
        match self {
            Self::L0 => "Discoverable",
            Self::L1 => "Observable",
            Self::L2 => "Controllable",
            Self::L3 => "Coordinated",
        }
    }
}

impl std::fmt::Display for ConformanceLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::L0 => "L0",
            Self::L1 => "L1",
            Self::L2 => "L2",
            Self::L3 => "L3",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    Property,
    Action,
    Event,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Read,
    Write,
    Subscribe,
    Invoke,
}

impl Operation {
    /// Operations that can change device state.
    pub fn is_mutating(self) -> bool {
        matches!(self, Operation::Write | Operation::Invoke)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffect {
    None,
    Reversible,
    Irreversible,
    Hazardous,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub kind: CapabilityKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    pub operations: Vec<Operation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<ValueSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<BTreeMap<String, ValueSpec>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<BTreeMap<String, ValueSpec>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<BTreeMap<String, ValueSpec>>,
    pub side_effect: SideEffect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness: Option<Freshness>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slew: Option<Slew>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preconditions: Vec<Precondition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<Verification>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<Timing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<serde_json::Map<String, serde_json::Value>>,
}

impl Capability {
    pub fn supports(&self, op: Operation) -> bool {
        self.operations.contains(&op)
    }

    pub fn is_mutating(&self) -> bool {
        self.operations.iter().any(|op| op.is_mutating())
    }

    /// A property that can be read, so it can serve as a precondition or readback source.
    pub fn is_readable_property(&self) -> bool {
        self.kind == CapabilityKind::Property && self.supports(Operation::Read)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    Number,
    Integer,
    Boolean,
    Enum,
    String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueSpec {
    #[serde(rename = "type")]
    pub ty: ValueType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Range {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Freshness {
    pub max_age_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimit {
    pub max: u32,
    pub per_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slew {
    pub max_change: f64,
    pub per_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
}

impl CompareOp {
    pub fn is_ordering(self) -> bool {
        matches!(
            self,
            CompareOp::Lt | CompareOp::Le | CompareOp::Gt | CompareOp::Ge
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PreconditionValue {
    One(Value),
    Many(Vec<Value>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Precondition {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub capability: String,
    pub op: CompareOp,
    pub value: PreconditionValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Verification {
    Readback {
        /// Capability to read back. Defaults to the capability itself (properties only).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capability: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tolerance: Option<Tolerance>,
        /// Fixed expected value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<Value>,
        /// Expected value is taken from this action parameter.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_param: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        settle_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
    },
    None {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tolerance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timing {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_ms: Option<u64>,
    pub timeout_ms: u64,
}
