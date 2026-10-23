//! Conformance: run the adapter through the OpenMHS suite against the fake lamp.
//!
//! `cargo test` fails with the reasons if the adapter stops reaching the level its
//! descriptor declares, and writes the full report to `target/tmp/conformance/`.

use std::sync::Arc;
use std::time::Duration;

use openmhs_adapter::TokioClock;
use openmhs_adapter_template::LampAdapter;
use openmhs_adapter_template::fixture::FakeLamp;
use openmhs_adapter_template::link::TcpLink;
use openmhs_conformance::{Options, Report, render};
use openmhs_types::Clock;

fn save(name: &str, report: &Report) {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("conformance");
    if std::fs::create_dir_all(&dir).is_ok()
        && let Ok(json) = serde_json::to_string_pretty(report)
    {
        let _ = std::fs::write(dir.join(format!("{name}.json")), json);
    }
}

/// In memory, on paused time: fast and deterministic.
#[tokio::test(start_paused = true)]
async fn reaches_its_declared_level() {
    let clock: Arc<dyn Clock> = Arc::new(TokioClock::new());
    let lamp = FakeLamp::new();
    let adapter = Arc::new(LampAdapter::new(lamp.clone(), clock.clone()));
    let report = openmhs_conformance::run(adapter, lamp, Options::new(clock)).await;
    save("in-memory", &report);
    assert!(report.passed, "{}", render(&report));
}

/// Over a real socket, so the TCP link's error handling is tested too.
#[tokio::test]
async fn reaches_its_declared_level_over_tcp() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let lamp = FakeLamp::new();
    let server = tokio::spawn(lamp.clone().serve(listener));

    let clock: Arc<dyn Clock> = Arc::new(openmhs_types::SystemClock);
    let link = Arc::new(TcpLink::new(addr.to_string()).simulated());
    let adapter = Arc::new(
        LampAdapter::new(link, clock.clone()).with_reply_timeout(Duration::from_millis(300)),
    );
    let mut options = Options::new(clock);
    options.fault_deadline_ms = 600;
    let report = openmhs_conformance::run(adapter, lamp, options).await;
    server.abort();
    save("tcp", &report);
    assert!(report.passed, "{}", render(&report));
}
