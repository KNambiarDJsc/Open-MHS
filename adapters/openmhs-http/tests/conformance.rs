//! The HTTP adapter against the simulated device, over real TCP sockets.

use std::sync::Arc;

use openmhs_conformance::{Options, render};
use openmhs_http::sim::HttpSim;
use openmhs_http::{HttpAdapter, load};
use openmhs_types::{Clock, SystemClock};

#[tokio::test(flavor = "multi_thread")]
async fn led_controller_reaches_its_declared_level() {
    let profile = load(include_str!("../profiles/led-controller.json")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let sim = HttpSim::new(profile.clone());
    let server = tokio::spawn(sim.clone().serve(listener));

    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let adapter = HttpAdapter::new(profile, clock.clone())
        .unwrap()
        .at(format!("http://{addr}"))
        .simulated();
    let report =
        openmhs_conformance::run(Arc::new(adapter), sim.device(), Options::new(clock)).await;
    server.abort();

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("conformance");
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(
            dir.join("http.json"),
            serde_json::to_string_pretty(&report).unwrap(),
        );
    }
    assert!(report.passed, "{}", render(&report));
}
