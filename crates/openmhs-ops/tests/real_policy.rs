//! What the failure shape finds in the policy demo: a pretrained LeRobot diffusion policy on PushT, two samplers, seeds
//! 1000 to 1019. The events in `tests/fixtures` were produced from the recorded results by
//! `python -m openmhs.importers.policy_rollouts`.
//!
//! SIMULATION. The outcome and the value behind it are the simulator's own judgement (ADR-0036), never a verification.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use openmhs_ops::{IngestStatus, Store};
use serde_json::Value;

fn loaded() -> Store {
    let records: Vec<Value> = include_str!("fixtures/policy_pusht_rollouts.jsonl")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let mut store = Store::open_in_memory().unwrap();
    let results = store.ingest(&records, 1).unwrap();
    let rejected: Vec<_> = results
        .iter()
        .filter(|r| matches!(r.status, IngestStatus::Rejected { .. }))
        .collect();
    assert!(rejected.is_empty(), "{rejected:?}");
    store.rebuild().unwrap();
    store
}

#[test]
fn each_failed_episode_says_how_far_it_got_as_the_simulator_reported_it() {
    let store = loaded();
    let list = store.incidents(None, None, 100).unwrap();
    assert_eq!(list.len(), 13, "the 13 failed episodes");

    let mut misses: Vec<(f64, String)> = Vec::new();
    for i in &list {
        let d = store.incident(&i.incident_id).unwrap().unwrap();
        let sh = d.shape.unwrap();
        assert_eq!(sh.rules, "shape-2");
        assert_eq!(sh.points.len(), 1, "{}", i.incident_id);
        let m = &sh.points[0].misses[0];
        assert_eq!(
            (m.source.as_str(), m.side.as_deref()),
            ("reported", Some("below"))
        );
        assert_eq!(sh.goals.verified, 0, "nothing here is verified");
        assert!(sh.headline.contains("(reported peak reward reaches 1:"));
        misses.push((m.at_least.unwrap(), i.task_id.clone()));
    }
    misses.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Peak rewards from the result files: 1 - peak, smallest first.
    let expected = [
        0.0003, 0.0007, 0.0029, 0.0060, 0.0068, 0.0120, 0.0218, 0.0275, 0.0292, 0.0385, 0.0386,
        0.2903, 0.9037,
    ];
    for ((got, task), want) in misses.iter().zip(expected) {
        assert!((got - want).abs() < 1e-9, "{task}: {got} against {want}");
    }
    // Eleven near misses; one stopped at 0.71 of the threshold; one barely moved the block.
    assert_eq!(misses.iter().filter(|(d, _)| *d < 0.04).count(), 11);
    assert_eq!(misses[11].1, "ddim-10/seed-1014");
    assert_eq!(misses[12].1, "ddpm-100/seed-1001");
}

#[test]
fn one_signature_and_its_count_per_release_the_distance_is_not_in_the_signature() {
    let store = loaded();
    let list = store.incidents(None, None, 100).unwrap();
    let signatures: BTreeMap<_, usize> = list.iter().fold(BTreeMap::new(), |mut acc, i| {
        *acc.entry(i.signature.clone().unwrap()).or_default() += 1;
        acc
    });
    assert_eq!(
        signatures,
        BTreeMap::from([("pusht/task_failed/-/reported:reward".to_owned(), 13)])
    );
    let r = store
        .incident(&list[0].incident_id)
        .unwrap()
        .unwrap()
        .recurrence
        .unwrap();
    assert_eq!(r.incidents, 13);
    assert_eq!(r.per_release.get("ddim-10"), Some(&8));
    assert_eq!(r.per_release.get("ddpm-100"), Some(&5));
}
