//! The Modbus adapter against the simulated device, over real TCP sockets.

use std::sync::Arc;

use openmhs_conformance::{Options, render};
use openmhs_modbus::sim::ModbusSim;
use openmhs_modbus::{ModbusAdapter, load};
use openmhs_types::{Clock, SystemClock};

#[tokio::test(flavor = "multi_thread")]
async fn climate_chamber_reaches_its_declared_level() {
    let profile = load(include_str!("../profiles/climate-chamber.json")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let sim = ModbusSim::new(profile.clone());
    let server = tokio::spawn(sim.clone().serve(listener));

    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let adapter = Arc::new(
        ModbusAdapter::new(profile, clock.clone())
            .at(addr.to_string())
            .simulated(),
    );
    let report = openmhs_conformance::run(adapter, sim.device(), Options::new(clock)).await;
    server.abort();

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("conformance");
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(
            dir.join("modbus.json"),
            serde_json::to_string_pretty(&report).unwrap(),
        );
    }
    assert!(report.passed, "{}", render(&report));
}
