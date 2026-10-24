//! Acceptance: a Python adapter passes the same conformance suite as a Rust adapter.
//!
//! The Python PSU simulator (adapters/python/psu-sim) models the same device, with the
//! same descriptor, as the Rust one in openmhs-sim. Both go through the unchanged suite;
//! the Python one through the adapter host, over a real child process.

mod common;

use std::sync::Arc;

use openmhs_adapter::HardwareAdapter;
use openmhs_conformance::sim::SimFixture;
use openmhs_conformance::{ConformanceLevel, Options, render};
use openmhs_host::{HostOptions, ProcessAdapter};
use openmhs_sim::VirtualPsu;
use openmhs_types::{Clock, SystemClock};

#[tokio::test(flavor = "multi_thread")]
async fn python_psu_reaches_the_same_level_as_the_rust_psu() {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let (psu, handle) = VirtualPsu::new(clock.clone(), 42);
    let rust = openmhs_conformance::run(
        Arc::new(psu),
        Arc::new(SimFixture::new(Arc::new(handle))),
        Options::new(clock.clone()),
    )
    .await;
    assert!(rust.passed, "{}", render(&rust));

    let adapter = ProcessAdapter::start(common::psu_sim(), HostOptions::unsigned_fixture())
        .await
        .expect("start the Python PSU");
    let fixture = adapter.fixture().expect("fixture mode");
    let adapter = Arc::new(adapter);
    let python = openmhs_conformance::run(
        adapter.clone(),
        Arc::new(fixture),
        Options::new(clock.clone()),
    )
    .await;
    adapter.shutdown().await.unwrap();

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("conformance");
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(
            dir.join("python-psu.json"),
            serde_json::to_string_pretty(&python).unwrap(),
        );
    }
    assert!(python.passed, "{}", render(&python));
    assert_eq!(python.achieved, Some(ConformanceLevel::L3));
    assert_eq!(python.achieved, rust.achieved);
    // The same checks, with the same results.
    let outcome = |r: &openmhs_conformance::Report| {
        r.checks
            .iter()
            .map(|c| (c.id.clone(), c.capability.clone(), c.status))
            .collect::<Vec<_>>()
    };
    assert_eq!(outcome(&python), outcome(&rust));
}
