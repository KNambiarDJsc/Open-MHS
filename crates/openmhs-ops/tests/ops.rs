#![allow(clippy::unwrap_used)]

use openmhs_events::{Manifest, format_ns};
use openmhs_ops::{CompareSpec, IngestStatus, OutcomeKind, Store, Verdict};
use proptest::prelude::*;
use serde_json::{Value, json};

const S: i64 = 1_000_000_000;

fn manifest(policy: &str, release: &str) -> Value {
    json!({"schema": "openmhs.deployment/0.1",
           "components": [{"kind": "software", "name": "stack", "version": "1.0"},
                          {"kind": "model", "name": "policy", "revision": policy}],
           "labels": {"release": release}})
}

fn digest(v: &Value) -> String {
    Manifest::parse(v).unwrap().digest
}

#[derive(Default)]
struct Ev {
    id: String,
    kind: &'static str,
    t: i64,
    robot: &'static str,
    site: &'static str,
    task: Option<String>,
    task_type: Option<&'static str>,
    goal: Option<String>,
    manifest: Option<String>,
    reported: Option<&'static str>,
    verified: Option<&'static str>,
    category: Option<&'static str>,
}

fn ev(e: Ev) -> Value {
    let mut v = json!({
        "schema": "openmhs.event/0.1", "event_id": e.id, "type": e.kind,
        "source": {"id": "test", "kind": "sim"},
        "time": {"observed": format_ns(e.t), "clock_domain": "sim:test", "uncertainty_ms": 0},
        "scope": {"tenant": "lab", "site": e.site, "robot": e.robot},
        "provenance": "observed"
    });
    let mut subject = serde_json::Map::new();
    if let Some(t) = e.task {
        subject.insert("task_id".into(), json!(t));
    }
    if let Some(t) = e.task_type {
        subject.insert("task_type".into(), json!(t));
    }
    if let Some(g) = e.goal {
        subject.insert("goal_id".into(), json!(g));
    }
    if !subject.is_empty() {
        v["subject"] = Value::Object(subject);
    }
    if let Some(m) = e.manifest {
        v["deployment"] = json!({"manifest": m});
    }
    if let Some(r) = e.reported {
        v["outcome"] = json!({"reported": {"value": r}});
    }
    if let Some(x) = e.verified {
        v["outcome"] = json!({"verified": {"value": x, "contract": "c"}});
    }
    if let Some(c) = e.category {
        v["attributes"] = json!({"category": c, "severity": "error"});
    }
    v
}

/// One task: started, an action with a goal, ended with `reported`.
fn task(
    id: &str,
    robot: &'static str,
    site: &'static str,
    t: i64,
    m: Option<&str>,
    reported: &'static str,
) -> Vec<Value> {
    vec![
        ev(Ev {
            id: format!("{id}-s"),
            kind: "task.started",
            t,
            robot,
            site,
            task: Some(id.into()),
            task_type: Some("pick"),
            manifest: m.map(String::from),
            ..Default::default()
        }),
        ev(Ev {
            id: format!("{id}-a"),
            kind: "action.ended",
            t: t + S,
            robot,
            site,
            task: Some(id.into()),
            goal: Some(format!("{id}-goal")),
            reported: Some(reported),
            ..Default::default()
        }),
        ev(Ev {
            id: format!("{id}-e"),
            kind: "task.ended",
            t: t + 2 * S,
            robot,
            site,
            task: Some(id.into()),
            reported: Some(reported),
            ..Default::default()
        }),
    ]
}

fn fixture() -> Vec<Value> {
    let m1 = manifest("aaaa111", "r1");
    let m2 = manifest("bbbb222", "r2");
    let (d1, d2) = (digest(&m1), digest(&m2));
    let mut out = vec![m1, m2];
    // t1 succeeds under r1; t2 reports success under r2 but its goal was verified violated; a fault on the robot
    // without a task falls in t2's window; t3 failed; t4 never ended.
    out.extend(task("t1", "r-1", "east", 100 * S, Some(&d1), "succeeded"));
    out.extend(task("t2", "r-1", "east", 200 * S, Some(&d2), "succeeded"));
    out.push(ev(Ev {
        id: "v2".into(),
        kind: "outcome.verified",
        t: 201 * S,
        robot: "r-1",
        site: "east",
        goal: Some("t2-goal".into()),
        verified: Some("violated"),
        ..Default::default()
    }));
    out.push(ev(Ev {
        id: "f2".into(),
        kind: "fault.raised",
        t: 195 * S,
        robot: "r-1",
        site: "east",
        category: Some("perception.depth"),
        ..Default::default()
    }));
    out.extend(task("t3", "r-1", "east", 300 * S, Some(&d2), "aborted"));
    out.push(ev(Ev {
        id: "t4-s".into(),
        kind: "task.started",
        t: 400 * S,
        robot: "r-1",
        site: "east",
        task: Some("t4".into()),
        task_type: Some("pick"),
        ..Default::default()
    }));
    // A rollout on robot r-2, and a task there that names no manifest.
    out.push(ev(Ev {
        id: "dep".into(),
        kind: "deployment.applied",
        t: 50 * S,
        robot: "r-2",
        site: "west",
        manifest: Some(d1.clone()),
        ..Default::default()
    }));
    out.extend(task("t5", "r-2", "west", 500 * S, None, "succeeded"));
    out
}

fn dump(store: &Store) -> String {
    // Everything derived, in a fixed order.
    let mut s = String::new();
    for inc in store.incidents(None, None, 1000).unwrap() {
        let d = store.incident(&inc.incident_id).unwrap().unwrap();
        s.push_str(&serde_json::to_string(&d).unwrap());
    }
    s.push_str(&format!("{:?}", store.counts().unwrap()));
    s
}

fn loaded(records: &[Value]) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store.ingest(records, 1).unwrap();
    store.rebuild().unwrap();
    store
}

#[test]
fn every_record_gets_its_own_answer_and_a_bad_one_fails_nothing() {
    let mut store = Store::open_in_memory().unwrap();
    let mut records = fixture();
    let first = store.ingest(&records, 1).unwrap();
    assert!(
        first.iter().all(|r| r.status == IngestStatus::Accepted),
        "{first:?}"
    );

    let mut changed = records[2].clone();
    changed["outcome"] = json!({"reported": {"value": "failed"}});
    records.push(changed);
    records.push(json!({"schema": "openmhs.event/0.1", "event_id": "bad"}));
    records.push(json!({"hello": 1}));
    let second = store.ingest(&records, 2).unwrap();
    let n = second.len();
    assert!(
        second[..n - 3]
            .iter()
            .all(|r| r.status == IngestStatus::Duplicate)
    );
    assert!(matches!(
        second[n - 3].status,
        IngestStatus::Conflict { .. }
    ));
    assert!(
        matches!(&second[n - 2].status, IngestStatus::Rejected { code, .. } if code == "event.field")
    );
    assert!(
        matches!(&second[n - 1].status, IngestStatus::Rejected { code, .. } if code == "record.schema")
    );
    let counts = store.counts().unwrap();
    assert!(counts.contains(&("event_conflicts".into(), 1)));
}

#[test]
fn incidents_open_for_the_right_tasks_and_say_why_each_event_belongs() {
    let store = loaded(&fixture());
    let incidents = store.incidents(None, None, 100).unwrap();
    let by_task: std::collections::BTreeMap<_, _> = incidents
        .iter()
        .map(|i| (i.task_id.as_str(), i.triggers.clone()))
        .collect();
    assert_eq!(
        by_task.get("t2"),
        Some(&vec!["reported_verified_disagree".to_string()])
    );
    assert_eq!(
        by_task.get("t3"),
        Some(&vec!["reported_aborted".to_string()])
    );
    assert_eq!(
        by_task.get("t4"),
        Some(&vec!["reported_unknown".to_string()])
    );
    assert!(
        !by_task.contains_key("t1") && !by_task.contains_key("t5"),
        "{by_task:?}"
    );

    let t2 = incidents.iter().find(|i| i.task_id == "t2").unwrap();
    let d = store.incident(&t2.incident_id).unwrap().unwrap();
    let basis = |id: &str| {
        d.members
            .iter()
            .find(|m| m.event_id == id)
            .map(|m| (m.basis.clone(), m.detail.clone()))
    };
    assert_eq!(basis("t2-s"), Some(("id_match".into(), "task_id".into())));
    assert_eq!(basis("v2").map(|b| b.1), Some("goal_id".into()));
    assert_eq!(basis("f2").map(|b| b.0), Some("window".into()));
    assert_eq!(d.summary.release.as_deref(), Some("r2"));
    // t1 was the last success on this robot; only the policy changed since.
    let last = d.since_last_success.unwrap();
    assert_eq!(last.task_id, "t1");
    assert_eq!(last.changes.len(), 1);
    assert_eq!(last.changes[0].name, "policy");
}

#[test]
fn a_task_without_a_manifest_takes_the_rollout_in_force_and_says_so() {
    let store = loaded(&fixture());
    let cmp = store
        .compare(&CompareSpec::new("pick", "r1", "r2", OutcomeKind::Reported))
        .unwrap();
    // r1: t1 (event) and t5 (rollout interval on r-2).
    assert_eq!(cmp.a.attempts, 2);
    assert_eq!(cmp.a.sites.get("west"), Some(&1));
}

#[test]
fn incident_ids_are_stable_across_rebuilds() {
    let mut store = loaded(&fixture());
    let before: Vec<String> = store
        .incidents(None, None, 100)
        .unwrap()
        .into_iter()
        .map(|i| i.incident_id)
        .collect();
    store.rebuild().unwrap();
    let after: Vec<String> = store
        .incidents(None, None, 100)
        .unwrap()
        .into_iter()
        .map(|i| i.incident_id)
        .collect();
    assert_eq!(before, after);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]
    /// Any order, any repetition of the same records: the same derived tables, byte for byte.
    #[test]
    fn derived_tables_do_not_depend_on_arrival_order_or_duplicates(
        perm in Just(fixture()).prop_shuffle(),
        dups in proptest::collection::vec(0usize..18, 0..6),
    ) {
        let reference = dump(&loaded(&fixture()));
        let mut records = perm.clone();
        for i in dups {
            records.push(perm[i % perm.len()].clone());
        }
        let mut store = Store::open_in_memory().unwrap();
        // Split across two batches as well.
        let mid = records.len() / 2;
        store.ingest(&records[..mid], 1).unwrap();
        store.ingest(&records[mid..], 2).unwrap();
        store.rebuild().unwrap();
        prop_assert_eq!(dump(&store), reference);
    }
}

fn fleet(
    release: &str,
    policy: &str,
    site: &'static str,
    n: usize,
    failures: usize,
    start: i64,
) -> Vec<Value> {
    let m = manifest(policy, release);
    let d = digest(&m);
    let mut out = vec![m];
    for i in 0..n {
        let reported = if i < failures { "failed" } else { "succeeded" };
        out.extend(task(
            &format!("{release}-{site}-{i}"),
            "r-1",
            site,
            start + (i as i64) * 10 * S,
            Some(&d),
            reported,
        ));
    }
    out
}

#[test]
fn a_clear_regression_is_reported_with_its_interval() {
    let mut records = fleet("v1", "p1", "east", 200, 4, 0);
    records.extend(fleet("v2", "p2", "east", 200, 40, 100_000 * S));
    let store = loaded(&records);
    let c = store
        .compare(&CompareSpec::new("pick", "v1", "v2", OutcomeKind::Reported))
        .unwrap();
    assert_eq!(
        (c.a.failed, c.a.attempts, c.b.failed, c.b.attempts),
        (4, 200, 40, 200)
    );
    assert_eq!(c.verdict, Verdict::Regression, "{c:?}");
    let (d, lo, _) = c.difference.unwrap();
    assert!((d - 0.18).abs() < 1e-9 && lo > 0.01);
}

#[test]
fn small_samples_answer_insufficient_evidence() {
    let mut records = fleet("v1", "p1", "east", 10, 1, 0);
    records.extend(fleet("v2", "p2", "east", 10, 6, 100_000 * S));
    let store = loaded(&records);
    let c = store
        .compare(&CompareSpec::new("pick", "v1", "v2", OutcomeKind::Reported))
        .unwrap();
    assert_eq!(c.verdict, Verdict::InsufficientEvidence);
    assert!(c.reasons[0].contains("fewer than 30"), "{:?}", c.reasons);
}

#[test]
fn a_site_confound_is_removed_by_stratification_and_named() {
    // Within each site the two releases fail equally often; v2 ran mostly at the hard site.
    let mut records = fleet("v1", "p1", "easy", 300, 6, 0);
    records.extend(fleet("v1", "p1", "hard", 60, 6, 10_000 * S));
    records.extend(fleet("v2", "p2", "easy", 60, 1, 100_000 * S));
    records.extend(fleet("v2", "p2", "hard", 300, 30, 200_000 * S));
    let store = loaded(&records);
    let mut spec = CompareSpec::new("pick", "v1", "v2", OutcomeKind::Reported);
    let c = store.compare(&spec).unwrap();
    assert_eq!(c.difference_method, "mantel_haenszel_by_site");
    assert!(c.difference.unwrap().0.abs() < 0.01, "{:?}", c.difference);
    assert!(
        c.warnings.iter().any(|w| w.contains("site mix")),
        "{:?}",
        c.warnings
    );
    spec.by_site = false;
    let pooled = store.compare(&spec).unwrap();
    assert_eq!(
        pooled.verdict,
        Verdict::Regression,
        "pooling alone would call it a regression"
    );
}

#[test]
fn verified_and_reported_outcomes_are_compared_separately() {
    let store = loaded(&fixture());
    let reported = store
        .compare(&CompareSpec::new("pick", "r1", "r2", OutcomeKind::Reported))
        .unwrap();
    let verified = store
        .compare(&CompareSpec::new("pick", "r1", "r2", OutcomeKind::Verified))
        .unwrap();
    // r2: t2 reported success but was verified violated; t3 reported aborted and has no verdict.
    assert_eq!((reported.b.succeeded, reported.b.failed), (1, 1));
    assert_eq!((verified.b.failed, verified.b.unknown), (1, 1));
}

#[test]
fn a_window_join_never_compares_two_clocks_or_two_components() {
    let mut records = task("t9", "r-1", "east", 100 * S, None, "aborted");
    // Same instant, but another clock (a second simulation run restarting at zero, say): never joined.
    let mut other_clock = ev(Ev {
        id: "x-clock".into(),
        kind: "fault.raised",
        t: 101 * S,
        robot: "r-1",
        site: "east",
        ..Default::default()
    });
    other_clock["time"]["clock_domain"] = json!("sim:another-run");
    // Same clock and time, but about the base while the task is about the lift: not joined.
    let mut other_component = ev(Ev {
        id: "x-comp".into(),
        kind: "fault.raised",
        t: 101 * S,
        robot: "r-1",
        site: "east",
        ..Default::default()
    });
    other_component["attributes"] = json!({"component": "base"});
    // Same clock, no component: joined by window.
    let plain = ev(Ev {
        id: "x-plain".into(),
        kind: "fault.raised",
        t: 101 * S,
        robot: "r-1",
        site: "east",
        ..Default::default()
    });
    records[0]["attributes"] = json!({"component": "lift"});
    records.extend([other_clock, other_component, plain]);
    let store = loaded(&records);
    let inc = &store.incidents(None, None, 10).unwrap()[0];
    let d = store.incident(&inc.incident_id).unwrap().unwrap();
    let ids: Vec<&str> = d.members.iter().map(|m| m.event_id.as_str()).collect();
    assert!(ids.contains(&"x-plain"), "{ids:?}");
    assert!(
        !ids.contains(&"x-clock") && !ids.contains(&"x-comp"),
        "{ids:?}"
    );
}

#[test]
fn a_store_from_a_newer_program_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ops.sqlite");
    drop(Store::open(&path).unwrap());
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
    }
    let err = Store::open(&path).err().unwrap().to_string();
    assert!(err.contains("newer"), "{err}");
}

#[test]
fn an_edited_migration_is_noticed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ops.sqlite");
    drop(Store::open(&path).unwrap());
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE schema_migrations SET checksum = 'sha256:x' WHERE version = 1",
            [],
        )
        .unwrap();
    }
    let err = Store::open(&path).err().unwrap().to_string();
    assert!(err.contains("edited"), "{err}");
    // Reopening an untouched store is fine.
    let other = dir.path().join("ok.sqlite");
    drop(Store::open(&other).unwrap());
    assert!(Store::open(&other).is_ok());
}

#[test]
fn a_store_from_the_previous_schema_version_upgrades_in_place_and_keeps_its_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ops.sqlite");
    // Build a version-1 store by hand, exactly as the previous release would have left it.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        let sql = include_str!("../src/migrations/0001_init.sql");
        conn.execute_batch(sql).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, checksum TEXT NOT NULL, applied_ns INTEGER NOT NULL)",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO schema_migrations VALUES (1, ?1, 0)",
            [openmhs_events::sha256_digest(&sql.replace('\r', ""))],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        let body =
            r#"{"schema":"openmhs.event/0.1","subject":{"scenario_id":"s-1","task_id":"t"}}"#;
        conn.execute(
            "INSERT INTO events (tenant, event_id, type, source_id, clock_domain, ingested_ns, robot, provenance,
                 content_hash, body) VALUES ('lab', 'old', 'task.started', 's', 'c', 1, 'r', 'observed', 'sha256:x', ?1)",
            [body],
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 4);
    let scenario: Option<String> = conn
        .query_row(
            "SELECT scenario_id FROM events WHERE event_id = 'old'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        scenario.as_deref(),
        Some("s-1"),
        "the new column is filled from the stored body"
    );
    assert!(store.counts().unwrap().contains(&("events".into(), 1)));
}

/// 40 scenarios, each attempted once under release `v1` and once under `v2`. `v1` fails on the first 12; `v2` fails on
/// the first 3 of those. Every scenario is the same situation in both releases.
fn paired_fleet() -> Vec<Value> {
    let m1 = manifest("p1", "v1");
    let m2 = manifest("p2", "v2");
    let (d1, d2) = (digest(&m1), digest(&m2));
    let mut out = vec![m1, m2];
    for (release, d, fails) in [("v1", &d1, 12usize), ("v2", &d2, 3usize)] {
        for i in 0..40usize {
            let id = format!("{release}-{i}");
            let reported = if i < fails { "failed" } else { "succeeded" };
            let t = (i as i64) * 10 * S + if release == "v2" { 100_000 * S } else { 0 };
            for (n, kind) in ["task.started", "task.ended"].iter().enumerate() {
                let mut e = ev(Ev {
                    id: format!("{id}-{n}"),
                    kind,
                    t: t + (n as i64) * S,
                    robot: "r-1",
                    site: "east",
                    task: Some(id.clone()),
                    task_type: Some("pick"),
                    manifest: (n == 0).then(|| d.clone()),
                    reported: (n == 1).then_some(reported),
                    ..Default::default()
                });
                e["subject"]["scenario_id"] = json!(format!("scenario-{i:02}"));
                out.push(e);
            }
        }
    }
    out
}

#[test]
fn a_paired_comparison_is_tighter_than_two_independent_samples_and_names_who_differs() {
    let store = loaded(&paired_fleet());
    let mut spec = CompareSpec::new("pick", "v1", "v2", OutcomeKind::Reported);
    let unpaired = store.compare(&spec).unwrap();
    spec.paired = true;
    let paired = store.compare(&spec).unwrap();
    let p = paired.paired.as_ref().unwrap();
    assert_eq!(
        (
            p.pairs,
            p.both_fail,
            p.a_only_fails,
            p.b_only_fails,
            p.both_ok
        ),
        (40, 3, 9, 0, 28)
    );
    assert!((p.mcnemar_p.unwrap() - 2.0 * 0.5_f64.powi(9)).abs() < 1e-12);
    let (d, lo, hi) = paired.difference.unwrap();
    assert!((d + 0.225).abs() < 1e-12);
    let (_, ulo, uhi) = unpaired.difference.unwrap();
    assert!(
        hi - lo < uhi - ulo,
        "paired {lo}..{hi}, unpaired {ulo}..{uhi}"
    );
    assert_eq!(paired.verdict, Verdict::Improvement, "{:?}", paired.reasons);
}

#[test]
fn scenarios_missing_from_one_arm_are_named_and_left_out_of_the_pairs() {
    let mut records = paired_fleet();
    // Drop scenario 39 from v2.
    records.retain(|r| {
        r["event_id"]
            .as_str()
            .is_none_or(|id| !id.starts_with("v2-39-"))
    });
    let store = loaded(&records);
    let mut spec = CompareSpec::new("pick", "v1", "v2", OutcomeKind::Reported);
    spec.paired = true;
    let c = store.compare(&spec).unwrap();
    let p = c.paired.as_ref().unwrap();
    assert_eq!((p.pairs, p.unmatched_a, p.unmatched_b), (39, 1, 0));
    assert!(
        c.warnings.iter().any(|w| w.contains("only in a")),
        "{:?}",
        c.warnings
    );
}

#[test]
fn the_wall_clock_cost_of_each_arm_is_reported_beside_the_simulated_duration() {
    // v1 attempts report 100 + i seconds of compute, v2 attempts 20 + i; the events' own clock says 1 s for all of them.
    let mut records = paired_fleet();
    for r in &mut records {
        if r["type"] == "task.ended" {
            let id = r["subject"]["task_id"].as_str().unwrap().to_owned();
            let (release, i) = id.split_once('-').unwrap();
            let i: f64 = i.parse().unwrap();
            let base = if release == "v1" { 100.0 } else { 20.0 };
            r["cost"] = json!({"wall_s": base + i});
        }
    }
    let store = loaded(&records);
    let c = store
        .compare(&CompareSpec::new("pick", "v1", "v2", OutcomeKind::Reported))
        .unwrap();
    // 40 attempts per arm, wall time 100..139 and 20..59: the median is the mean of the 20th and 21st values, and the
    // 90th percentile sits 35.1 ranks up.
    let close = |got: Option<(f64, f64, u64)>, want: (f64, f64, u64)| {
        let g = got.unwrap();
        assert!(
            (g.0 - want.0).abs() < 1e-9 && (g.1 - want.1).abs() < 1e-9 && g.2 == want.2,
            "{g:?} != {want:?}"
        );
    };
    close(c.a.wall_s, (119.5, 135.1, 40));
    close(c.b.wall_s, (39.5, 55.1, 40));
    assert_eq!(
        c.a.duration_s.map(|d| d.0),
        Some(1.0),
        "the simulated duration is a different thing"
    );
}

#[test]
fn an_arm_whose_producer_reports_no_wall_time_shows_none_not_zero() {
    let store = loaded(&paired_fleet());
    let c = store
        .compare(&CompareSpec::new("pick", "v1", "v2", OutcomeKind::Reported))
        .unwrap();
    assert_eq!((c.a.wall_s, c.b.wall_s), (None, None));
}
