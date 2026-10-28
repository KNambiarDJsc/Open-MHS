//! Procedures for a model (spec/procedures.md): list, check and run approved ones; propose
//! new ones. There is no tool to approve: that is a person's decision.

use std::collections::BTreeMap;

use openmhs_types::procedure::Procedure;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{guidance, standard_formats, untrusted};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = standard_formats)]
pub struct RunArgs {
    /// The procedure's name, from list_procedures.
    pub name: String,
    /// A version; the newest approved one if omitted.
    #[serde(default)]
    pub version: Option<u32>,
    /// Parameter values, e.g. {"voltage": {"value": 2, "unit": "V"}}, in the declared units.
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
}

impl From<RunArgs> for openmhs_api::RunProcedureParams {
    fn from(a: RunArgs) -> Self {
        Self {
            name: a.name,
            version: a.version,
            params: a.params,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposeArgs {
    /// {"name", "version", "description"?, "params"?: {name: {"type": "quantity"|"integer"|
    /// "boolean"|"string", "unit"?, "min"?, "max"?, "default"?}}, "plan": a plan in which
    /// {"$param": "<name>"} stands for a parameter}.
    pub procedure: Value,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(transform = standard_formats)]
pub struct ProcedureView {
    pub name: String,
    pub version: u32,
    /// `proposed` (cannot run yet) or `approved`.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Parameters with their types, units, ranges and defaults.
    pub params: Value,
    pub proposed_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
    pub sha256: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProcedureList {
    pub procedures: Vec<ProcedureView>,
}

pub fn view(p: &Procedure) -> ProcedureView {
    let mut params = serde_json::to_value(&p.spec.params).unwrap_or(Value::Null);
    // Descriptions were written by whoever proposed the procedure, possibly a model.
    untrusted::descriptor(&mut params);
    ProcedureView {
        name: p.spec.name.clone(),
        version: p.spec.version,
        status: guidance::name(&p.status),
        description: p.spec.description.as_deref().map(untrusted::quote),
        params,
        proposed_by: p.proposed_by.clone(),
        approved_by: p.approved_by.clone(),
        sha256: p.sha256.clone(),
    }
}

pub fn list_view(list: &[Procedure]) -> ProcedureList {
    ProcedureList {
        procedures: list.iter().map(view).collect(),
    }
}
