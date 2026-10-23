//! The MQTT adapter against the simulated broker and device, over real TCP sockets.

use std::sync::Arc;

use openmhs_conformance::{Options, render};
use openmhs_mqtt::sim::MqttSim;
use openmhs_mqtt::{MqttAdapter, load};
use openmhs_types::{Clock, SystemClock};

#[tokio::test(flavor = "multi_thread")]
async fn greenhouse_node_reaches_its_declared_level() {
    let profile = load(include_str!("../profiles/greenhouse-node.json")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let sim = MqttSim::new(profile.clone());
    let server = tokio::spawn(sim.clone().serve(listener));

    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let adapter = MqttAdapter::new(profile, clock.clone())
        .unwrap()
        .at("127.0.0.1", port)
        .simulated();
    let report =
        openmhs_conformance::run(Arc::new(adapter), sim.device(), Options::new(clock)).await;
    server.abort();

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("conformance");
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(
            dir.join("mqtt.json"),
            serde_json::to_string_pretty(&report).unwrap(),
        );
    }
    assert!(report.passed, "{}", render(&report));
}
