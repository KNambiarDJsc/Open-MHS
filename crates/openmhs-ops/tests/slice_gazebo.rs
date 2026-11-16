//! The first vertical slice's acceptance list (docs/architecture/deployment-intelligence/08-vertical-slice-and-demos.md
//! §J), on events the Python importer produced from two recorded Gazebo runs (`tests/fixtures`, regenerate with
//! `python -m openmhs.importers.openamr_gazebo`). SIMULATION ONLY.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use openmhs_ops::{CompareSpec, IngestStatus, OutcomeKind, Store};
use serde_json::Value;

fn fixture() -> Vec<Value> {
    include_str!("fixtures/gazebo_bullet1_dart1.jsonl")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn loaded() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    let results = store.ingest(&fixture(), 1).unwrap();
    let rejected: Vec<_> = results
        .iter()
        .filter(|r| matches!(r.status, IngestStatus::Rejected { .. }))
        .collect();
    assert!(rejected.is_empty(), "{rejected:?}");
    store.rebuild().unwrap();
    store
}

#[test]
fn every_manifest_reference_written_in_python_resolves_in_rust() {
    // The importer computes manifest digests in Python; the store recomputes them in Rust. They must agree, or
    // events would point at manifests the store does not have.
    let store = loaded();
    let cmp_bullet = store
        .compare(&CompareSpec::new(
            "lift",
            "engine-bullet",
            "engine-dartsim",
            OutcomeKind::Reported,
        ))
        .unwrap();
    assert_eq!(cmp_bullet.a.manifests.len(), 1);
    assert_eq!(cmp_bullet.b.manifests.len(), 1);
    assert_eq!(
        cmp_bullet.a.attempts, 10,
        "bullet1's ten lift goals are attributed to its manifest"
    );
    assert_eq!(cmp_bullet.b.attempts, 10);
    assert!(
        !cmp_bullet
            .warnings
            .iter()
            .any(|w| w.contains("no known manifest")),
        "{:?}",
        cmp_bullet.warnings
    );
}

#[test]
fn incidents_open_exactly_where_success_was_reported_but_not_verified() {
    let store = loaded();
    let lift: Vec<_> = store.incidents(None, Some("lift"), 100).unwrap();
    let bullet: BTreeSet<String> = lift
        .iter()
        .filter(|i| i.task_id.starts_with("bullet1/"))
        .map(|i| i.task_id.clone())
        .collect();
    let expected: BTreeSet<String> = ["L03", "L04", "L06", "L07", "L09"]
        .iter()
        .map(|t| format!("bullet1/{t}"))
        .collect();
    assert_eq!(
        bullet, expected,
        "the five goals faster than the 0.018 m/s limit (RESULTS_GAZEBO.md)"
    );
    assert!(
        lift.iter()
            .all(|i| i.triggers == vec!["reported_verified_disagree".to_string()])
    );
    assert_eq!(
        lift.iter()
            .filter(|i| i.task_id.starts_with("dart1/"))
            .count(),
        8,
        "under DART the lift never moved"
    );
}

#[test]
fn a_lift_incident_shows_its_goal_its_verdict_its_deployment_and_its_recording() {
    let store = loaded();
    let inc = store
        .incidents(None, Some("lift"), 100)
        .unwrap()
        .into_iter()
        .find(|i| i.task_id == "bullet1/L03")
        .unwrap();
    let d = store.incident(&inc.incident_id).unwrap().unwrap();
    let kinds: Vec<(&str, &str, &str)> = d
        .members
        .iter()
        .map(|m| (m.kind.as_str(), m.basis.as_str(), m.detail.as_str()))
        .collect();
    assert!(
        kinds.contains(&("action.ended", "id_match", "task_id")),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&("outcome.verified", "id_match", "goal_id")),
        "{kinds:?}"
    );
    // The verdict was placed from the recorder's clock with a stated uncertainty.
    let verdict = d
        .members
        .iter()
        .find(|m| m.kind == "outcome.verified")
        .unwrap();
    assert_eq!(verdict.uncertainty_ms, Some(50.5));
    assert_eq!(verdict.verified.as_deref(), Some("violated"));
    // No odometry verdict (component base) joined a lift task.
    assert!(
        d.members
            .iter()
            .all(|m| m.kind != "outcome.verified" || m.basis == "id_match")
    );
    // The deployment: the lift controller's recorded goal tolerance of zero is in the manifest.
    let manifest = d.manifest.as_ref().unwrap().to_string();
    assert!(manifest.contains("\"goal_tolerance_m\":0.0"), "{manifest}");
    assert!(manifest.contains("bullet-featherstone"));
    // The recording, by reference, cut to the incident's window.
    assert_eq!(d.artifacts.len(), 1);
    assert!(d.artifacts[0].uri.ends_with("bullet1/bag/bag_0.mcap"));
    assert!(d.artifacts[0].window.0 < d.artifacts[0].window.1);
    // Same fingerprint elsewhere: the other four in bullet1 and the eight in dart1.
    assert_eq!(d.similar.len(), 12);
}

#[test]
fn drive_tasks_join_odometry_verdicts_by_window_only() {
    let store = loaded();
    let drives = store.incidents(None, Some("drive"), 100).unwrap();
    assert!(!drives.is_empty());
    for inc in &drives {
        assert!(
            inc.triggers
                .contains(&"window_verified_violated".to_string()),
            "{inc:?}"
        );
        let d = store.incident(&inc.incident_id).unwrap().unwrap();
        let verdicts: Vec<_> = d
            .members
            .iter()
            .filter(|m| m.kind == "outcome.verified")
            .collect();
        assert!(!verdicts.is_empty());
        assert!(verdicts.iter().all(|m| m.basis == "window"));
    }
}

#[test]
fn reported_and_verified_outcomes_tell_different_stories_about_the_same_runs() {
    let store = loaded();
    let reported = store
        .compare(&CompareSpec::new(
            "lift",
            "engine-bullet",
            "engine-dartsim",
            OutcomeKind::Reported,
        ))
        .unwrap();
    let verified = store
        .compare(&CompareSpec::new(
            "lift",
            "engine-bullet",
            "engine-dartsim",
            OutcomeKind::Verified,
        ))
        .unwrap();
    assert_eq!(
        (reported.a.failed, reported.b.failed),
        (0, 0),
        "the action reported success every time"
    );
    assert_eq!((verified.a.failed, verified.b.failed), (5, 8));
    // Ten attempts per arm: honest about it.
    assert_eq!(verified.verdict, openmhs_ops::Verdict::InsufficientEvidence);
}

#[test]
fn importing_twice_changes_nothing() {
    let mut store = loaded();
    let before = store.counts().unwrap();
    let again = store.ingest(&fixture(), 2).unwrap();
    assert!(again.iter().all(|r| r.status == IngestStatus::Duplicate));
    store.rebuild().unwrap();
    assert_eq!(store.counts().unwrap(), before);
}

/// Release A: the stack as shipped (the task client sends no tolerance; the controller checks none).
/// Release B: the task client sends a 0.01 m goal tolerance. Three recorded runs each, same task list.
fn ab() -> Store {
    let records: Vec<Value> = include_str!("fixtures/gazebo_ab.jsonl")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let mut store = Store::open_in_memory().unwrap();
    store.ingest(&records, 1).unwrap();
    store.rebuild().unwrap();
    store
}

#[test]
fn the_reported_status_cannot_see_the_fix_the_verified_outcome_can() {
    let store = ab();
    let mut spec = CompareSpec::new("lift", "engine-bullet", "tol-0.01", OutcomeKind::Reported);
    let reported = store.compare(&spec).unwrap();
    assert_eq!(
        reported.a.manifests.len(),
        1,
        "three identical runs, one manifest"
    );
    assert_eq!(reported.b.manifests.len(), 1);
    assert_eq!((reported.a.attempts, reported.b.attempts), (30, 30));
    assert_eq!(
        (reported.a.failed, reported.b.failed),
        (0, 0),
        "SUCCEEDED every time in both releases"
    );
    assert_eq!(
        reported.verdict,
        openmhs_ops::Verdict::InsufficientEvidence,
        "no failures to compare"
    );

    // The 30 attempts are 10 scenarios run three times each. By default they are counted as 10 scenarios.
    spec.outcome = OutcomeKind::Verified;
    let verified = store.compare(&spec).unwrap();
    assert_eq!(verified.a.unit, "scenario");
    assert_eq!((verified.a.scenarios, verified.b.scenarios), (10, 10));
    assert_eq!(
        (verified.a.attempt_outcomes.1, verified.a.attempts),
        (15, 30)
    );
    assert_eq!((verified.a.failed, verified.a.succeeded), (5, 5));
    assert_eq!((verified.b.failed, verified.b.succeeded), (0, 10));
    assert_eq!(
        verified.verdict,
        openmhs_ops::Verdict::InsufficientEvidence,
        "{:?}",
        verified.reasons
    );
    assert!(
        verified.reasons[0].contains("scenarios"),
        "{:?}",
        verified.reasons
    );
    assert!(
        verified
            .warnings
            .iter()
            .any(|w| w.contains("repeat scenarios")),
        "{:?}",
        verified.warnings
    );

    // Counting every attempt gives the earlier, overconfident answer: replicates passed off as independent trials.
    spec.by_scenario = false;
    let per_attempt = store.compare(&spec).unwrap();
    assert_eq!(per_attempt.a.unit, "attempt");
    assert_eq!((per_attempt.a.failed, per_attempt.a.attempts), (15, 30));
    assert_eq!((per_attempt.b.failed, per_attempt.b.attempts), (0, 30));
    assert_eq!(
        per_attempt.verdict,
        openmhs_ops::Verdict::Improvement,
        "{:?}",
        per_attempt.reasons
    );
    let (d, _, hi) = per_attempt.difference.unwrap();
    assert!((d + 0.5).abs() < 1e-9 && hi < -0.01);
}

#[test]
fn release_b_takes_longer_and_the_comparison_says_so() {
    let store = ab();
    let c = store
        .compare(&CompareSpec::new(
            "lift",
            "engine-bullet",
            "tol-0.01",
            OutcomeKind::Verified,
        ))
        .unwrap();
    let (median_a, _, n_a) = c.a.duration_s.unwrap();
    let (median_b, p90_b, n_b) = c.b.duration_s.unwrap();
    assert_eq!((n_a, n_b), (30, 30));
    assert!(median_b >= median_a, "{median_a} vs {median_b}");
    assert!(
        p90_b > 4.0,
        "the slowest B goals waited for the lift: {p90_b}"
    );
}

#[test]
fn events_whose_order_is_not_known_are_marked_concurrent() {
    let store = loaded();
    let inc = store
        .incidents(None, Some("lift"), 100)
        .unwrap()
        .into_iter()
        .find(|i| i.task_id == "bullet1/L03")
        .unwrap();
    let d = store.incident(&inc.incident_id).unwrap().unwrap();
    // action.ended and task.ended carry the same time and ±50 ms: nothing says which came first.
    let ended = d.members.iter().find(|m| m.kind == "action.ended").unwrap();
    assert!(
        ended
            .concurrent_with
            .iter()
            .any(|id| id.ends_with("/task.ended")),
        "{:?}",
        ended.concurrent_with
    );
    // The verdict is 40 ms from the action's end, and both are uncertain by about 50 ms: concurrent too.
    let verdict = d
        .members
        .iter()
        .find(|m| m.kind == "outcome.verified")
        .unwrap();
    assert!(
        verdict
            .concurrent_with
            .iter()
            .any(|id| id.ends_with("/action.ended"))
    );
    // The deployment event is 19 seconds earlier: ordered.
    let deployment = d
        .members
        .iter()
        .find(|m| m.kind == "deployment.applied")
        .unwrap();
    assert!(deployment.concurrent_with.is_empty());
}

#[test]
fn pairing_by_scenario_matches_each_lift_goal_with_itself() {
    let store = ab();
    let mut spec = CompareSpec::new("lift", "engine-bullet", "tol-0.01", OutcomeKind::Verified);
    spec.paired = true;
    let c = store.compare(&spec).unwrap();
    let p = c.paired.as_ref().unwrap();
    // The ten lift goals appear in both releases. Five failed under A and none under B.
    assert_eq!(
        (p.pairs, p.unmatched_a, p.unmatched_b, p.unknown_pairs),
        (10, 0, 0, 0)
    );
    assert_eq!(
        (p.both_fail, p.a_only_fails, p.b_only_fails, p.both_ok),
        (0, 5, 0, 5)
    );
    // Five discordant pairs, all one way: exact two-sided p = 2 / 2^5.
    assert!((p.mcnemar_p.unwrap() - 0.0625).abs() < 1e-12);
    assert_eq!(c.difference_method, "newcombe_paired");
    let (d, lo, hi) = c.difference.unwrap();
    assert!((d + 0.5).abs() < 1e-12);
    // Here B never fails, so no scenario fails under both and the pairs carry no correlation: the paired interval is the
    // unpaired one, never wider. (The synthetic fleet in tests/ops.rs shows the narrowing when failures overlap.)
    spec.paired = false;
    let (_, ulo, uhi) = store.compare(&spec).unwrap().difference.unwrap();
    assert!(
        hi - lo <= uhi - ulo + 1e-12,
        "paired {lo}..{hi}, unpaired {ulo}..{uhi}"
    );
    // Ten pairs are still too few to call it, said plainly.
    assert_eq!(c.verdict, openmhs_ops::Verdict::InsufficientEvidence);
    assert!(
        c.reasons[0].contains("matched scenario pairs"),
        "{:?}",
        c.reasons
    );
}
