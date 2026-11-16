//! What the layer finds in three REAL flights: the CoreSense Inspection Testbed ROSbag Dataset (three Crazyflie drones
//! under OptiTrack motion capture, Zenodo 10.5281/zenodo.23243295, CC BY 4.0). The events in `tests/fixtures` were
//! produced from the recordings by `python -m openmhs.importers.coresense_aerostack2`.
//!
//! The failures in these flights were INDUCED by the experimenters. These tests pin what was found, not what a field
//! deployment would produce.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use openmhs_ops::{IngestStatus, Store};
use serde_json::Value;

fn records() -> Vec<Value> {
    include_str!("fixtures/coresense_real_flights.jsonl")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn loaded() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    let results = store.ingest(&records(), 1).unwrap();
    let rejected: Vec<_> = results
        .iter()
        .filter(|r| matches!(r.status, IngestStatus::Rejected { .. }))
        .collect();
    assert!(rejected.is_empty(), "{rejected:?}");
    store.rebuild().unwrap();
    store
}

#[test]
fn nine_missions_and_one_incident_the_drone_the_swarm_reported_failed() {
    let store = loaded();
    let counts: BTreeMap<_, _> = store.counts().unwrap().into_iter().collect();
    assert_eq!(
        counts["task_attempts"], 9,
        "three drones in each of three flights"
    );
    assert_eq!(
        counts["manifests"], 1,
        "the same software and paths in all three"
    );
    assert_eq!(
        counts["artifacts"], 3,
        "one recording per flight, by reference"
    );

    let incidents = store.incidents(None, None, 100).unwrap();
    assert_eq!(incidents.len(), 1, "{incidents:?}");
    let inc = &incidents[0];
    assert!(
        inc.task_id.ends_with("double_failure/drone0/mission"),
        "{}",
        inc.task_id
    );
    assert_eq!(inc.robot, "crazyflie/drone0");
    assert_eq!(inc.triggers, vec!["reported_failed".to_string()]);
    assert_eq!(inc.reported.as_deref(), Some("failed"));
    assert_eq!(
        inc.verified, None,
        "nothing independent verified these flights"
    );
}

#[test]
fn the_failed_drones_timeline_shows_the_fault_inside_a_goal_that_was_then_canceled() {
    let store = loaded();
    let inc = store.incidents(None, None, 10).unwrap().remove(0);
    let d = store.incident(&inc.incident_id).unwrap().unwrap();

    let fault = d
        .members
        .iter()
        .find(|m| m.kind == "fault.raised")
        .expect("the swarm's own report of the failure");
    assert_eq!(fault.code.as_deref(), Some("drone_status_failed"));
    assert_eq!(fault.provenance, "reported");

    // The goal that was running when the failure was reported ends canceled, after the fault.
    let canceled: Vec<_> = d
        .members
        .iter()
        .filter(|m| m.kind == "action.ended" && m.reported.as_deref() == Some("canceled"))
        .collect();
    assert!(
        canceled.len() >= 2,
        "goals were canceled around the failure: {}",
        canceled.len()
    );
    assert!(
        canceled.iter().any(|m| m.observed_ns > fault.observed_ns),
        "a goal is canceled after the failure was reported"
    );
    // Every member joined by task id: the importer names the mission on every event.
    assert!(d.members.iter().all(|m| m.basis == "id_match"));
    // The recording, by reference, cut to the incident.
    assert_eq!(d.artifacts.len(), 1);
    assert!(d.artifacts[0].uri.ends_with(".db3"));
}

#[test]
fn the_last_success_is_an_earlier_flight_with_the_same_software() {
    let store = loaded();
    let inc = store.incidents(None, None, 10).unwrap().remove(0);
    let d = store.incident(&inc.incident_id).unwrap().unwrap();
    let last = d
        .since_last_success
        .expect("drone0 flew successfully in another flight");
    assert!(last.task_id.ends_with("drone0/mission"));
    assert!(
        last.changes.is_empty(),
        "the manifest is the same in every flight: {:?}",
        last.changes
    );
}

#[test]
fn recoveries_without_a_failure_open_no_incident() {
    // The panel-failure flight had goals canceled on all three drones while an auction reassigned a waypoint, and
    // every drone landed. Cancellation by itself is not a failure, so no incident opens for it (rules corr-1).
    let store = loaded();
    let panel: Vec<_> = store
        .incidents(None, None, 100)
        .unwrap()
        .into_iter()
        .filter(|i| i.task_id.contains("panel_failure"))
        .collect();
    assert!(panel.is_empty());
}

#[test]
fn importing_the_flights_twice_changes_nothing() {
    let mut store = loaded();
    let before = store.counts().unwrap();
    let again = store.ingest(&records(), 2).unwrap();
    assert!(again.iter().all(|r| r.status == IngestStatus::Duplicate));
    store.rebuild().unwrap();
    assert_eq!(store.counts().unwrap(), before);
}

// ---------------------------------------------------------------------------------------------------------------
// The same three flights, with `--verify`: each succeeded Takeoff, GoTo and Land checked against motion capture by the
// deterministic verifier, judged against the thresholds the manifest declares. The GoTo threshold (0.20 m) is an
// ASSUMPTION (the flights' own configuration is not published), and the manifest says so.

fn verified_records() -> Vec<Value> {
    include_str!("fixtures/coresense_real_flights_verified.jsonl")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn loaded_verified() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    let results = store.ingest(&verified_records(), 1).unwrap();
    assert!(
        !results
            .iter()
            .any(|r| matches!(r.status, IngestStatus::Rejected { .. })),
        "{results:?}"
    );
    store.rebuild().unwrap();
    store
}

#[test]
fn the_verdicts_are_those_of_the_evaluation_and_join_their_goals_by_id() {
    let mut tally: BTreeMap<(String, String), u32> = BTreeMap::new();
    for r in verified_records() {
        if r["type"] == "outcome.verified" {
            assert!(
                r["subject"]["task_id"].is_null(),
                "a verdict names its goal, not its task"
            );
            *tally
                .entry((
                    r["attributes"]["component"].as_str().unwrap().into(),
                    r["outcome"]["verified"]["value"].as_str().unwrap().into(),
                ))
                .or_default() += 1;
        }
    }
    let t = |c: &str, v: &str| tally.get(&(c.into(), v.into())).copied().unwrap_or(0);
    // 107 GoTo claims, of which only 4 are inside 0.20 m at the instant of the action's success status (the other 7
    // had evidence outside the 50 ms window or in conflict, and the verifier says UNKNOWN rather than guess).
    assert_eq!(
        (
            t("goto", "satisfied"),
            t("goto", "unknown"),
            t("goto", "violated")
        ),
        (4, 7, 96)
    );
    assert_eq!(
        (t("takeoff", "satisfied"), t("takeoff", "violated")),
        (9, 0)
    );
    assert_eq!((t("land", "satisfied"), t("land", "violated")), (6, 2));
}

#[test]
fn with_the_assumed_threshold_every_mission_has_a_verified_violation_and_the_layer_says_so() {
    let store = loaded_verified();
    let incidents = store.incidents(None, None, 100).unwrap();
    assert_eq!(
        incidents.len(),
        9,
        "was 1 without verification: {incidents:?}"
    );
    let disagree = incidents
        .iter()
        .filter(|i| i.triggers == vec!["reported_verified_disagree".to_string()])
        .count();
    assert_eq!(
        disagree, 8,
        "the eight missions that reported success and landed"
    );
    let failed = incidents
        .iter()
        .find(|i| i.task_id.ends_with("double_failure/drone0/mission"))
        .unwrap();
    assert_eq!(
        failed.triggers,
        vec![
            "reported_failed".to_string(),
            "verified_violated".to_string()
        ]
    );
    assert!(
        incidents
            .iter()
            .all(|i| i.verified.as_deref() == Some("violated"))
    );
}

#[test]
fn an_incident_shows_each_goals_verdict_beside_the_report_it_disagrees_with() {
    let store = loaded_verified();
    let inc = store
        .incidents(None, None, 100)
        .unwrap()
        .into_iter()
        .find(|i| i.task_id.ends_with("no_failure/drone0/mission"))
        .unwrap();
    let d = store.incident(&inc.incident_id).unwrap().unwrap();
    let verdicts: Vec<_> = d
        .members
        .iter()
        .filter(|m| m.kind == "outcome.verified")
        .collect();
    assert!(
        verdicts.len() > 10,
        "one per succeeded goal: {}",
        verdicts.len()
    );
    assert!(
        verdicts
            .iter()
            .all(|m| m.basis == "id_match" && m.goal_id.is_some()),
        "joined by goal id"
    );
    assert!(
        verdicts
            .iter()
            .any(|m| m.verified.as_deref() == Some("violated"))
    );
    assert!(
        verdicts
            .iter()
            .any(|m| m.verified.as_deref() == Some("satisfied"))
    );
    // The goal's own report is in the same timeline: success, at the same instant as the verdict that disputes it.
    let goal = verdicts
        .iter()
        .find(|m| m.verified.as_deref() == Some("violated"))
        .unwrap()
        .goal_id
        .clone();
    assert!(d.members.iter().any(|m| m.kind == "action.ended"
        && m.goal_id == goal
        && m.reported.as_deref() == Some("succeeded")));
}

#[test]
fn the_manifest_names_the_thresholds_and_says_the_goto_one_is_assumed() {
    let store = loaded_verified();
    let inc = store.incidents(None, None, 1).unwrap().remove(0);
    let d = store.incident(&inc.incident_id).unwrap().unwrap();
    let manifest = d.manifest.expect("the incident's manifest");
    let th = manifest["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "behavior_thresholds")
        .expect("thresholds are declared, not hidden in code");
    assert_eq!(th["go_to_xy_m"], 0.2);
    assert_eq!(th["takeoff_height_m"], 1.5);
    assert!(th["basis"].as_str().unwrap().contains("assumption"));
    assert_eq!(
        inc.release.as_deref(),
        Some("aerostack2-unversioned+thresholds"),
        "a different assumption is a different release, so it cannot be mistaken for the unverified import"
    );
}

#[test]
fn importing_the_verified_flights_twice_changes_nothing() {
    let mut store = loaded_verified();
    let before = store.counts().unwrap();
    let again = store.ingest(&verified_records(), 2).unwrap();
    assert!(again.iter().all(|r| r.status == IngestStatus::Duplicate));
    store.rebuild().unwrap();
    assert_eq!(store.counts().unwrap(), before);
}
