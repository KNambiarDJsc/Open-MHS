//! The virtual lab passes the levels its descriptors declare (M5 acceptance).

use std::sync::Arc;

use openmhs_adapter::TokioClock;
use openmhs_conformance::{ConformanceLevel, Options, Report, Status, render, sim};
use openmhs_types::schema::Schema;
use openmhs_types::{Clock, Timestamp};

async fn reports() -> Vec<Report> {
    let clock: Arc<dyn Clock> = Arc::new(TokioClock::starting_at(Timestamp::from_millis(
        1_790_000_000_000,
    )));
    let mut out = Vec::new();
    for device in openmhs_sim::virtual_lab(clock.clone(), 42) {
        out.push(sim::run(&device, Options::new(clock.clone())).await);
    }
    out
}

#[tokio::test(start_paused = true)]
async fn every_virtual_device_reaches_its_declared_level() {
    for report in reports().await {
        println!("{}", render(&report));
        assert!(report.passed, "{}", render(&report));
        let expected = match report.subject.device.as_deref() {
            Some("sim.thermo-01") => ConformanceLevel::L1,
            Some("sim.psu-01" | "sim.axis-01") => ConformanceLevel::L3,
            other => panic!("unexpected device {other:?}"),
        };
        assert_eq!(report.declared, Some(expected));
        assert_eq!(report.achieved, Some(expected), "{}", render(&report));
        assert_eq!(report.summary.fail, 0, "{}", render(&report));
        assert!(report.fixture.counts_deliveries || expected == ConformanceLevel::L1);
    }
}

#[tokio::test(start_paused = true)]
async fn reports_match_the_schema_and_round_trip() {
    for report in reports().await {
        let json = serde_json::to_value(&report).unwrap();
        let issues = openmhs_types::schema::validate(Schema::ConformanceReport, &json);
        assert!(issues.is_empty(), "{issues:?}");
        let back: Report = serde_json::from_value(json).unwrap();
        assert_eq!(back, report);
    }
}

#[tokio::test(start_paused = true)]
async fn every_state_changing_capability_is_exercised_under_every_fault() {
    for report in reports().await {
        for c in report
            .checks
            .iter()
            .filter(|c| c.level <= ConformanceLevel::L3)
        {
            assert_ne!(
                c.status,
                Status::Untested,
                "{} {:?}: {}",
                c.id,
                c.capability,
                c.message
            );
        }
        let device = report.subject.device.clone().unwrap_or_default();
        let ids = |id: &str| report.checks.iter().filter(|c| c.id == id).count();
        match device.as_str() {
            "sim.psu-01" => {
                // Four state-changing capabilities, each under four faults.
                for id in [
                    "L2.command",
                    "L2.reject",
                    "L2.malformed",
                    "L2.timeout",
                    "L2.lost_ack",
                ] {
                    assert_eq!(ids(id), 4, "{id}");
                }
                // Six readable properties.
                assert_eq!(ids("L1.read"), 6);
            }
            "sim.axis-01" => assert_eq!(ids("L2.lost_ack"), 3),
            _ => assert_eq!(ids("L2.command"), 1),
        }
    }
}
