//! The operator's topology file (spec/topology.md): installation-specific relationships
//! between registered devices, in the same form descriptors use, plus the device each
//! one is about. Mirrors `topology.schema.json`.

use serde::{Deserialize, Serialize};

use crate::descriptor::{Relationship, RelationshipKind};

/// Topology format versions this implementation accepts.
pub const SUPPORTED_TOPOLOGY_VERSIONS: &[&str] = &["0.1"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Topology {
    pub openmhs_topology: String,
    #[serde(default)]
    pub relationships: Vec<TopologyRelationship>,
}

/// A relationship plus the registered device it starts from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopologyRelationship {
    pub from: String,
    pub kind: RelationshipKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl TopologyRelationship {
    pub fn relationship(&self) -> Relationship {
        Relationship {
            kind: self.kind,
            component: self.component.clone(),
            target: self.target.clone(),
            resource: self.resource.clone(),
            frame: self.frame.clone(),
            capabilities: self.capabilities.clone(),
            description: self.description.clone(),
        }
    }
}

impl Topology {
    /// Parse a topology file. Unknown fields and unsupported versions are errors.
    pub fn parse(text: &str) -> Result<Self, String> {
        let t: Topology = serde_json::from_str(text).map_err(|e| format!("topology: {e}"))?;
        if !SUPPORTED_TOPOLOGY_VERSIONS.contains(&t.openmhs_topology.as_str()) {
            return Err(format!(
                "topology: version '{}' is not supported (supported: {SUPPORTED_TOPOLOGY_VERSIONS:?})",
                t.openmhs_topology
            ));
        }
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_refuses_unknown_fields_and_versions() {
        let t = Topology::parse(
            r#"{"openmhs_topology":"0.1","relationships":[
                {"from":"a.dev","kind":"depends_on","target":"b.dev"}]}"#,
        )
        .unwrap();
        assert_eq!(
            t.relationships[0].relationship().target.as_deref(),
            Some("b.dev")
        );
        assert!(Topology::parse(r#"{"openmhs_topology":"9"}"#).is_err());
        assert!(
            Topology::parse(
                r#"{"openmhs_topology":"0.1","relationships":[{"from":"a","kind":"near"}]}"#
            )
            .is_err()
        );
        assert!(Topology::parse(r#"{"openmhs_topology":"0.1","extra":1}"#).is_err());
    }
}
