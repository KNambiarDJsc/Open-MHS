//! What the layer finds in three REAL flights: the CoreSense Inspection Testbed ROSbag Dataset (three Crazyflie drones
//! under OptiTrack motion capture, Zenodo 10.5281/zenodo.23243295, CC BY 4.0). The events in `tests/fixtures` were
//! produced from the recordings by `python -m openmhs.importers.coresense_aerostack2`.
//!
//! The failures in these flights were INDUCED by the experimenters. These tests pin what was found, not what a field
//! deployment would produce.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use openmhs_ops::{CompareSpec, IngestStatus, OutcomeKind, Store};
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

#[test]
fn a_comparison_counts_the_verified_goals_per_behavior_and_gives_no_interval_for_them() {
    let store = loaded_verified();
    let release = "aerostack2-unversioned+thresholds";
    let c = store
        .compare(&CompareSpec::new(
            "coverage_mission",
            release,
            release,
            OutcomeKind::Reported,
        ))
        .unwrap();
    let g = &c.a.goal_verdicts;
    // The tally of the evaluation: GoTo 4 satisfied / 96 violated / 7 unknown, Takeoff 9 satisfied, Land 6 / 2.
    assert_eq!(g.get("goto"), Some(&(4, 96, 7)), "{g:?}");
    assert_eq!(g.get("takeoff"), Some(&(9, 0, 0)), "{g:?}");
    assert_eq!(g.get("land"), Some(&(6, 2, 0)), "{g:?}");
    assert_eq!(c.a.goal_verdicts, c.b.goal_verdicts);
}

#[test]
fn an_arm_without_verified_goals_reports_none() {
    let store = loaded();
    let c = store
        .compare(&CompareSpec::new(
            "coverage_mission",
            "aerostack2-unversioned",
            "aerostack2-unversioned",
            OutcomeKind::Reported,
        ))
        .unwrap();
    assert!(c.a.goal_verdicts.is_empty());
}

// ------------------------------------------------------------------------------- the shape (ADR-0035, shape-1)

fn shapes(store: &Store) -> Vec<(openmhs_ops::IncidentSummary, openmhs_ops::Shape)> {
    store
        .incidents(None, None, 100)
        .unwrap()
        .into_iter()
        .map(|i| {
            let d = store.incident(&i.incident_id).unwrap().unwrap();
            (i, d.shape.expect("every incident has a shape"))
        })
        .collect()
}

#[test]
fn the_shapes_add_up_to_the_evaluations_tally() {
    let all = shapes(&loaded_verified());
    assert_eq!(all.len(), 9);
    let mut per: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
    for (_, sh) in &all {
        for (c, n) in &sh.goals.per_component {
            let e = per.entry(c.clone()).or_default();
            *e = (e.0 + n.satisfied, e.1 + n.violated, e.2 + n.unknown);
        }
    }
    assert_eq!(per.get("goto"), Some(&(4, 96, 7)));
    assert_eq!(per.get("takeoff"), Some(&(9, 0, 0)));
    assert_eq!(per.get("land"), Some(&(6, 2, 0)));
    let disagree: u64 = all
        .iter()
        .map(|(_, sh)| sh.disagreements.reported_succeeded_verified_violated)
        .sum();
    assert_eq!(
        disagree, 98,
        "every violated goal had reported succeeded: 96 GoTo, 2 Land"
    );
}

#[test]
fn every_flight_first_misses_a_waypoint_by_centimetres_and_the_order_is_known() {
    let all = shapes(&loaded_verified());
    let mut by_axis: BTreeMap<String, (u64, f64)> = BTreeMap::new();
    for (_, sh) in &all {
        let first = &sh.points[0];
        assert_eq!(
            (first.kind.as_str(), first.component.as_deref()),
            ("verified_violated", Some("goto"))
        );
        assert!(
            sh.first_unordered_with.is_empty(),
            "one recorder clock, distinct instants"
        );
        for p in &sh.points {
            for m in &p.misses {
                if p.component.as_deref() == Some("goto") {
                    assert_eq!(m.unit.as_deref(), Some("m"));
                    let axis = m.condition.split(' ').next().unwrap().to_owned();
                    let e = by_axis.entry(axis).or_default();
                    e.0 += 1;
                    e.1 = e.1.max(m.at_least.unwrap());
                }
            }
        }
    }
    // Each violated GoTo failed one axis, never both. Against the ASSUMED 0.20 m threshold, at the instant of the
    // success report, by at most 7.3 cm: the early report the evaluation found, measured goal by goal.
    assert_eq!(by_axis["x"].0 + by_axis["y"].0, 96);
    assert_eq!((by_axis["x"].0, by_axis["y"].0), (35, 61));
    assert!(by_axis.values().all(|(_, max)| *max < 0.074), "{by_axis:?}");
}

#[test]
fn the_failed_drones_shape_places_the_fault_after_the_first_miss() {
    let all = shapes(&loaded_verified());
    let (_, sh) = all
        .iter()
        .find(|(i, _)| i.reported.as_deref() == Some("failed"))
        .unwrap();
    assert_eq!(
        sh.headline,
        "first at 17.5 s (39 % of the task): goto goal verified violated (3 of 8 verified goals), y within 0.20 m: \
         above the bound by at least 0.039 m; violated goals: goto 6 of 7, takeoff 0 of 1; fault drone_status_failed \
         (mission) 22.3 s after the first point; 6 goal(s) reported succeeded and verified violated"
    );
    let kinds: Vec<&str> = sh.points.iter().map(|p| p.kind.as_str()).collect();
    assert!(
        kinds.contains(&"action_canceled")
            && kinds.contains(&"fault")
            && kinds.contains(&"task_failed")
    );
}

#[test]
fn the_landings_reported_done_were_above_the_ground_limit() {
    let all = shapes(&loaded_verified());
    let land: Vec<&openmhs_ops::shape::MissSummary> = all
        .iter()
        .flat_map(|(_, sh)| sh.misses.iter())
        .filter(|m| m.contract.as_deref() == Some("land-on-ground"))
        .collect();
    assert_eq!(land.len(), 2);
    assert!(
        land.iter()
            .all(|m| m.above == 1 && m.unit.as_deref() == Some("m"))
    );
}

#[test]
fn headlines_have_no_causal_word_and_the_signature_splits_by_axis() {
    let all = shapes(&loaded_verified());
    for (_, sh) in &all {
        let h = sh.headline.to_lowercase();
        for w in [
            "because",
            "cause",
            "due to",
            "led to",
            "root",
            "therefore",
            "resulted",
        ] {
            assert!(!h.contains(w), "{w:?} in {h}");
        }
    }
    // shape-1 keys the signature on the first point's failing condition, so one pattern (early success reports)
    // appears as two signatures, by which axis missed first. Recorded as found; a change belongs in a new rule version.
    let mut sigs: BTreeMap<String, u64> = BTreeMap::new();
    for (i, _) in &all {
        *sigs.entry(i.signature.clone().unwrap()).or_default() += 1;
    }
    assert_eq!(
        sigs,
        BTreeMap::from([
            (
                "coverage_mission/verified_violated/goto/goto-arrived-at-waypoint:x".to_owned(),
                4
            ),
            (
                "coverage_mission/verified_violated/goto/goto-arrived-at-waypoint:y".to_owned(),
                5
            ),
        ])
    );
}

#[test]
fn flights_imported_without_verification_have_shapes_without_measured_misses() {
    let all = shapes(&loaded());
    assert_eq!(all.len(), 1, "only the drone that reported failure");
    let sh = &all[0].1;
    assert!(sh.misses.is_empty());
    assert_eq!(sh.goals.verified, 0);
    assert_eq!(sh.points[0].kind, "action_canceled", "{:?}", sh.points);
}
