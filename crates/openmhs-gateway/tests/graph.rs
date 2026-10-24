//! M9: the capability graph over the API, and the topology the operator gives at start
//! (spec/topology.md).

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

use std::net::{Ipv4Addr, SocketAddr};

use openmhs_api::{Client, CreateTokenParams, Role};
use openmhs_gateway::{GatewayConfig, GatewayError};
use openmhs_types::Topology;
use openmhs_types::descriptor::RelationshipKind;
use openmhs_types::graph::Origin;

const WORKCELL: &str = include_str!("../../../spec/examples/topology/virtual-workcell.json");

fn config(home: &std::path::Path, topology: Option<Topology>) -> GatewayConfig {
    let mut config = GatewayConfig::new(home);
    config.listen = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    config.simulation = true;
    config.topology = topology;
    config
}

/// The example topology applies to the virtual lab, and any caller sees the result.
#[tokio::test(flavor = "multi_thread")]
async fn the_graph_merges_the_topology_and_is_visible_to_agents() {
    let home = tempfile::tempdir().unwrap();
    let topology = Topology::parse(WORKCELL).unwrap();
    let gw = openmhs_gateway::start(config(home.path(), Some(topology)))
        .await
        .unwrap();
    let operator = Client::new(&gw.url, &gw.operator_token).unwrap();
    // Even a credential scoped to one device sees the whole graph, as it sees the device list.
    let narrow = operator
        .create_token(CreateTokenParams::scoped(
            "agent:graph",
            Role::Agent,
            vec![openmhs_policy::Scope {
                device: "sim.thermo-01".into(),
                capability: "*".into(),
                operations: vec![openmhs_types::descriptor::Operation::Read],
            }],
        ))
        .await
        .unwrap();
    let agent = Client::new(&gw.url, &narrow.token).unwrap();
    let g = agent.graph().await.unwrap();

    assert!(g.issues.is_empty(), "{:?}", g.issues);
    assert_eq!(g.devices.len(), 3);
    let bench = &g.resources["workcell.bench"];
    assert!(bench.contains("sim.axis-01") && bench.contains("sim.thermo-01"));
    let depends: Vec<_> = g
        .relationships
        .iter()
        .filter(|e| e.relationship.kind == RelationshipKind::DependsOn)
        .collect();
    assert_eq!(depends.len(), 1);
    assert_eq!(depends[0].from, "sim.axis-01");
    assert_eq!(
        depends[0].relationship.target.as_deref(),
        Some("sim.psu-01")
    );
    assert_eq!(depends[0].origin, Origin::Topology);
    // The claim rule over the same graph: the bench ties the stage and the sensor.
    let claim = g.claim(["sim.axis-01"], []);
    assert!(claim.devices.contains("sim.thermo-01"));
    assert!(
        !claim.devices.contains("sim.psu-01"),
        "depends_on does not claim"
    );

    let audited = gw
        .engine()
        .journal()
        .audit_records()
        .unwrap()
        .into_iter()
        .any(|a| a.kind == "topology.set");
    assert!(audited);
    gw.shutdown().await.unwrap();
}

/// A topology that names a device the gateway does not have stops the start, rather than
/// leaving that device unclaimed by every lease.
#[tokio::test(flavor = "multi_thread")]
async fn a_topology_naming_an_unknown_device_stops_the_gateway() {
    for (text, code) in [
        (
            r#"{"openmhs_topology":"0.1","relationships":[
                {"from":"sim.ghost-01","kind":"shares_resource","resource":"bench"}]}"#,
            "topology.device.unknown",
        ),
        (
            r#"{"openmhs_topology":"0.1","relationships":[
                {"from":"sim.axis-01","kind":"depends_on","target":"sim.ghost-01"}]}"#,
            "topology.target.unknown",
        ),
        (
            r#"{"openmhs_topology":"0.1","relationships":[
                {"from":"sim.axis-01","kind":"frame","frame":"bench","capabilities":["axis.nope"]}]}"#,
            "descriptor.relationship.unknown_capability",
        ),
    ] {
        let home = tempfile::tempdir().unwrap();
        let topology = Topology::parse(text).unwrap();
        match openmhs_gateway::start(config(home.path(), Some(topology))).await {
            Ok(gw) => {
                gw.shutdown().await.unwrap();
                panic!("started with a bad topology ({code})");
            }
            Err(GatewayError::Config(message)) => assert!(message.contains(code), "{message}"),
            Err(other) => panic!("unexpected error {other}"),
        }
    }
}

/// Without a topology the graph is just the descriptors' own relationships (none, for the
/// virtual lab).
#[tokio::test(flavor = "multi_thread")]
async fn without_a_topology_the_graph_has_only_descriptor_relationships() {
    let home = tempfile::tempdir().unwrap();
    let gw = openmhs_gateway::start(config(home.path(), None))
        .await
        .unwrap();
    let operator = Client::new(&gw.url, &gw.operator_token).unwrap();
    let g = operator.graph().await.unwrap();
    assert_eq!(g.devices.len(), 3);
    assert!(
        g.relationships
            .iter()
            .all(|e| e.origin == Origin::Descriptor)
    );
    gw.shutdown().await.unwrap();
}
