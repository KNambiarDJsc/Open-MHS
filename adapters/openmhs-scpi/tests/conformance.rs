//! The SCPI adapter against the simulated instrument, through the real protocol code:
//! TCP sockets everywhere, and a pseudo-terminal serial pair on Unix.

use std::sync::Arc;

use openmhs_conformance::{Options, Report, render};
use openmhs_scpi::link::StreamLink;
use openmhs_scpi::sim::ScpiSim;
use openmhs_scpi::{ScpiAdapter, load};
use openmhs_types::{Clock, SystemClock};

const PROFILE: &str = include_str!("../profiles/bench-psu.json");

fn save(name: &str, report: &Report) {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("conformance");
    if std::fs::create_dir_all(&dir).is_ok()
        && let Ok(json) = serde_json::to_string_pretty(report)
    {
        let _ = std::fs::write(dir.join(format!("scpi-{name}.json")), json);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn bench_psu_over_tcp_reaches_its_declared_level() {
    let profile = load(PROFILE).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let sim = ScpiSim::new(profile.clone());
    let server = tokio::spawn(sim.clone().serve_tcp(listener));

    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let link = Arc::new(
        StreamLink::tcp(addr.to_string())
            .synced_by(profile.connection.identify.clone())
            .simulated(),
    );
    let adapter = Arc::new(ScpiAdapter::with_link(profile, link, clock.clone()));
    let report = openmhs_conformance::run(adapter, sim.device(), Options::new(clock)).await;
    server.abort();
    save("tcp", &report);
    assert!(report.passed, "{}", render(&report));
    assert_eq!(report.subject.sources, [openmhs_types::Source::Simulator]);
}

/// The serial transport over a real pseudo-terminal pair: the same code path as a USB
/// serial adapter, without hardware. Unix only; Windows has no built-in virtual ports.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn bench_psu_over_a_serial_line_reaches_its_declared_level() {
    use std::sync::Mutex;

    let profile = load(PROFILE).unwrap();
    let (instrument_end, adapter_end) = tokio_serial::SerialStream::pair().unwrap();
    let sim = ScpiSim::new(profile.clone());
    let server = tokio::spawn(sim.clone().serve_stream(instrument_end));

    let port = Arc::new(Mutex::new(Some(adapter_end)));
    let link = StreamLink::new(move || {
        let taken = port.lock().unwrap().take();
        async move { taken.ok_or_else(|| std::io::Error::other("the port is already open")) }
    })
    .synced_by(profile.connection.identify.clone())
    .simulated();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let adapter = Arc::new(ScpiAdapter::with_link(
        profile,
        Arc::new(link),
        clock.clone(),
    ));
    // A hang here once cost six hours of CI on macOS (an unbounded write on a full
    // pseudo-terminal). Fail in minutes instead, saying so.
    let report = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        openmhs_conformance::run(adapter, sim.device(), Options::new(clock)),
    )
    .await
    .expect("the serial conformance run did not finish within 5 minutes");
    server.abort();
    save("serial", &report);
    assert!(report.passed, "{}", render(&report));
}
