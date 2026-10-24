//! The capability graph (spec/topology.md): registered devices and their components,
//! linked by relationships from their descriptors and from the operator's topology.
//!
//! The graph is descriptive except in one place: it decides what a lease claims
//! ([`Graph::claim`]). Leasing a device claims what it contains and the resources it
//! shares; claiming a resource claims every device that shares it.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::descriptor::{Descriptor, Relationship, RelationshipKind};
use crate::issue::Issue;
use crate::topology::TopologyRelationship;
use crate::validate::relationship_issues;

/// Where a relationship came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Descriptor,
    Topology,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    #[serde(flatten)]
    pub relationship: Relationship,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    pub devices: Vec<Node>,
    pub relationships: Vec<Edge>,
    /// Each shared resource and the devices that use it.
    pub resources: BTreeMap<String, BTreeSet<String>>,
    /// Each frame and the devices with capabilities expressed in it.
    pub frames: BTreeMap<String, BTreeSet<String>>,
    /// Problems that do not stop the gateway: relationships to devices that are not
    /// registered, and `contains` cycles.
    pub issues: Vec<Issue>,
}

/// What a lease actually holds once the graph is applied.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub devices: BTreeSet<String>,
    pub resources: BTreeSet<String>,
}

impl Claim {
    pub fn overlaps(&self, other: &Claim) -> bool {
        !self.devices.is_disjoint(&other.devices) || !self.resources.is_disjoint(&other.resources)
    }
}

impl Graph {
    pub fn build(descriptors: &[Descriptor], topology: &[TopologyRelationship]) -> Self {
        let mut devices: Vec<Node> = descriptors
            .iter()
            .map(|d| Node {
                id: d.device.id.clone(),
                components: d.components.iter().map(|c| c.id.clone()).collect(),
            })
            .collect();
        devices.sort_by(|a, b| a.id.cmp(&b.id));
        let registered: BTreeSet<&str> = descriptors.iter().map(|d| d.device.id.as_str()).collect();

        let mut relationships = Vec::new();
        for d in descriptors {
            for r in &d.relationships {
                relationships.push(Edge {
                    from: d.device.id.clone(),
                    relationship: r.clone(),
                    origin: Origin::Descriptor,
                });
            }
        }
        for t in topology {
            relationships.push(Edge {
                from: t.from.clone(),
                relationship: t.relationship(),
                origin: Origin::Topology,
            });
        }
        relationships
            .sort_by(|a, b| (&a.from, a.relationship.kind).cmp(&(&b.from, b.relationship.kind)));

        let mut graph = Graph {
            devices,
            ..Graph::default()
        };
        for (i, e) in relationships.iter().enumerate() {
            let r = &e.relationship;
            if let Some(target) = &r.target
                && !registered.contains(target.as_str())
            {
                graph.issues.push(Issue::new(
                    "graph.target.unregistered",
                    format!("/relationships/{i}/target"),
                    format!(
                        "{} {} '{target}', which is not registered",
                        e.from,
                        r.kind.name()
                    ),
                ));
            }
            if let Some(resource) = &r.resource {
                graph
                    .resources
                    .entry(resource.clone())
                    .or_default()
                    .insert(e.from.clone());
            }
            if let Some(frame) = &r.frame {
                graph
                    .frames
                    .entry(frame.clone())
                    .or_default()
                    .insert(e.from.clone());
            }
        }
        graph.relationships = relationships;
        graph.issues.extend(graph.contains_cycles());
        graph
    }

    fn contains_edges(&self) -> impl Iterator<Item = (&str, &str)> {
        self.relationships.iter().filter_map(|e| {
            (e.relationship.kind == RelationshipKind::Contains)
                .then_some(())
                .and(e.relationship.target.as_deref())
                .map(|t| (e.from.as_str(), t))
        })
    }

    /// A device that (transitively) contains itself. Reported once per cycle member.
    fn contains_cycles(&self) -> Vec<Issue> {
        let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (from, to) in self.contains_edges() {
            children.entry(from).or_default().push(to);
        }
        let mut issues = Vec::new();
        for &start in children.keys() {
            let mut stack: Vec<&str> = children[start].clone();
            let mut seen = BTreeSet::new();
            while let Some(d) = stack.pop() {
                if d == start {
                    issues.push(Issue::new(
                        "graph.contains.cycle",
                        "",
                        format!("'{start}' contains itself through other devices"),
                    ));
                    break;
                }
                if seen.insert(d)
                    && let Some(next) = children.get(d)
                {
                    stack.extend(next);
                }
            }
        }
        issues
    }

    /// What leasing `devices` and `resources` claims: those, everything the claimed devices
    /// contain, the resources they share, and every device sharing a claimed resource,
    /// until nothing more is added.
    pub fn claim<'a>(
        &self,
        devices: impl IntoIterator<Item = &'a str>,
        resources: impl IntoIterator<Item = &'a str>,
    ) -> Claim {
        let mut claim = Claim {
            devices: devices.into_iter().map(str::to_owned).collect(),
            resources: resources.into_iter().map(str::to_owned).collect(),
        };
        loop {
            let before = (claim.devices.len(), claim.resources.len());
            for e in &self.relationships {
                let r = &e.relationship;
                match (r.kind, r.target.as_ref(), r.resource.as_ref()) {
                    (RelationshipKind::Contains, Some(target), _)
                        if claim.devices.contains(&e.from) =>
                    {
                        claim.devices.insert(target.clone());
                    }
                    (RelationshipKind::SharesResource, _, Some(resource)) => {
                        if claim.devices.contains(&e.from) {
                            claim.resources.insert(resource.clone());
                        }
                        if claim.resources.contains(resource) {
                            claim.devices.insert(e.from.clone());
                        }
                    }
                    _ => {}
                }
            }
            if (claim.devices.len(), claim.resources.len()) == before {
                return claim;
            }
        }
    }
}

/// Check the operator's topology against the registered devices. Any issue is fatal: a
/// topology that names a device the gateway does not have is a configuration error.
pub fn check_topology(descriptors: &[Descriptor], topology: &[TopologyRelationship]) -> Vec<Issue> {
    let by_id: BTreeMap<&str, &Descriptor> = descriptors
        .iter()
        .map(|d| (d.device.id.as_str(), d))
        .collect();
    let mut issues = Vec::new();
    for (i, t) in topology.iter().enumerate() {
        let at = format!("/relationships/{i}");
        let Some(d) = by_id.get(t.from.as_str()) else {
            issues.push(Issue::new(
                "topology.device.unknown",
                format!("{at}/from"),
                format!("device '{}' is not registered", t.from),
            ));
            continue;
        };
        issues.extend(relationship_issues(d, &t.relationship(), &at));
        if let Some(target) = &t.target
            && !by_id.contains_key(target.as_str())
        {
            issues.push(Issue::new(
                "topology.target.unknown",
                format!("{at}/target"),
                format!("device '{target}' is not registered"),
            ));
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, relationships: serde_json::Value) -> Descriptor {
        serde_json::from_value(serde_json::json!({
            "openmhs": "0.1",
            "device": { "id": id, "manufacturer": "T", "model": "T" },
            "adapter": { "id": "t", "version": "0.1.0" },
            "relationships": relationships,
            "capabilities": [{
                "name": "x", "kind": "property", "operations": ["read"],
                "value": { "type": "boolean" }, "side_effect": "none"
            }]
        }))
        .unwrap()
    }

    fn topo(
        from: &str,
        kind: RelationshipKind,
        target: Option<&str>,
        resource: Option<&str>,
    ) -> TopologyRelationship {
        TopologyRelationship {
            from: from.into(),
            kind,
            component: None,
            target: target.map(Into::into),
            resource: resource.map(Into::into),
            frame: None,
            capabilities: vec![],
            description: None,
        }
    }

    #[test]
    fn claims_follow_containment_and_shared_resources() {
        let devices = [
            device(
                "cell",
                serde_json::json!([{ "kind": "contains", "target": "arm" }]),
            ),
            device(
                "arm",
                serde_json::json!([{ "kind": "shares_resource", "resource": "bench" }]),
            ),
            device(
                "camera",
                serde_json::json!([{ "kind": "shares_resource", "resource": "bench" }]),
            ),
            device("psu", serde_json::json!([])),
            device("lonely", serde_json::json!([])),
        ];
        let topology = [topo("arm", RelationshipKind::DependsOn, Some("psu"), None)];
        let g = Graph::build(&devices, &topology);
        assert!(g.issues.is_empty(), "{:?}", g.issues);

        // The cell contains the arm, which shares the bench with the camera.
        let c = g.claim(["cell"], []);
        assert_eq!(
            c.devices,
            ["arm", "camera", "cell"].map(String::from).into()
        );
        assert_eq!(c.resources, ["bench".to_owned()].into());
        // Depending on the supply does not claim it.
        assert!(!c.devices.contains("psu"));
        // Claiming the resource alone claims both of its users.
        assert_eq!(
            g.claim([], ["bench"]).devices,
            ["arm", "camera"].map(String::from).into()
        );
        assert!(!g.claim(["lonely"], []).overlaps(&c));
        assert!(g.claim(["camera"], []).overlaps(&c));
        assert_eq!(
            g.resources["bench"],
            ["arm", "camera"].map(String::from).into()
        );
    }

    #[test]
    fn unregistered_targets_and_cycles_are_reported() {
        let devices = [
            device(
                "a",
                serde_json::json!([{ "kind": "contains", "target": "b" }]),
            ),
            device(
                "b",
                serde_json::json!([{ "kind": "contains", "target": "a" }]),
            ),
            device(
                "c",
                serde_json::json!([{ "kind": "depends_on", "target": "ghost" }]),
            ),
        ];
        let g = Graph::build(&devices, &[]);
        let codes: Vec<&str> = g.issues.iter().map(|i| i.code.as_str()).collect();
        assert!(codes.contains(&"graph.target.unregistered"), "{codes:?}");
        assert_eq!(
            codes
                .iter()
                .filter(|c| **c == "graph.contains.cycle")
                .count(),
            2
        );
        // A cycle still terminates the claim.
        assert_eq!(g.claim(["a"], []).devices.len(), 2);
    }

    #[test]
    fn a_topology_must_name_registered_devices_and_follow_the_rules() {
        let devices = [
            device("a", serde_json::json!([])),
            device("b", serde_json::json!([])),
        ];
        assert!(
            check_topology(
                &devices,
                &[topo("a", RelationshipKind::DependsOn, Some("b"), None)]
            )
            .is_empty()
        );
        let codes = |t: TopologyRelationship| -> Vec<String> {
            check_topology(&devices, &[t])
                .into_iter()
                .map(|i| i.code)
                .collect()
        };
        assert_eq!(
            codes(topo("ghost", RelationshipKind::DependsOn, Some("b"), None)),
            ["topology.device.unknown"]
        );
        assert_eq!(
            codes(topo("a", RelationshipKind::DependsOn, Some("ghost"), None)),
            ["topology.target.unknown"]
        );
        assert_eq!(
            codes(topo("a", RelationshipKind::DependsOn, Some("a"), None)),
            ["descriptor.relationship.self"]
        );
        assert_eq!(
            codes(topo("a", RelationshipKind::SharesResource, None, None)),
            ["descriptor.relationship.field_missing"]
        );
    }
}
