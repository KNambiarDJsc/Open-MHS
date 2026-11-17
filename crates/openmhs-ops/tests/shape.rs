//! The shape of an incident (ADR-0035; rules shape-2, ADR-0036), on events built by hand so every number is known.

#![allow(clippy::unwrap_used)]

use openmhs_events::{Manifest, format_ns};
use openmhs_ops::{IncidentDetail, Store};
use serde_json::{Value, json};

const S: i64 = 1_000_000_000;
const CAUSAL: &[&str] = &[
    "because",
    "cause",
    "due to",
    "led to",
    "root",
    "therefore",
    "resulted",
];

struct E {
    id: &'static str,
    kind: &'static str,
    t: i64,
    task: Option<&'static str>,
    goal: Option<&'static str>,
    component: Option<&'static str>,
    reported: Option<&'static str>,
    verified: Option<(&'static str, Value)>,
    code: Option<&'static str>,
    clock: &'static str,
    uncertainty_ms: Option<f64>,
    manifest: Option<String>,
}

impl Default for E {
    fn default() -> Self {
        Self {
            id: "",
            kind: "marker",
            t: 0,
            task: None,
            goal: None,
            component: None,
            reported: None,
            verified: None,
            code: None,
            clock: "robot",
            uncertainty_ms: None,
            manifest: None,
        }
    }
}

fn ev(e: E) -> Value {
    let mut time = json!({"observed": format_ns(e.t), "clock_domain": e.clock});
    if let Some(u) = e.uncertainty_ms {
        time["uncertainty_ms"] = json!(u);
    }
    let mut v = json!({
        "schema": "openmhs.event/0.1", "event_id": e.id, "type": e.kind,
        "source": {"id": "test", "kind": "sim"}, "time": time,
        "scope": {"tenant": "lab", "site": "east", "robot": "arm-1"}, "provenance": "observed"
    });
    let mut subject = serde_json::Map::new();
    if let Some(t) = e.task {
        subject.insert("task_id".into(), json!(t));
        subject.insert("task_type".into(), json!("pick"));
    }
    if let Some(g) = e.goal {
        subject.insert("goal_id".into(), json!(g));
    }
    if !subject.is_empty() {
        v["subject"] = Value::Object(subject);
    }
    let mut attributes = serde_json::Map::new();
    if let Some(c) = e.component {
        attributes.insert("component".into(), json!(c));
    }
    if let Some(c) = e.code {
        attributes.insert("code".into(), json!(c));
    }
    if !attributes.is_empty() {
        v["attributes"] = Value::Object(attributes);
    }
    if let Some(r) = e.reported {
        v["outcome"] = json!({"reported": {"value": r}});
    }
    if let Some((value, conditions)) = e.verified {
        let mut verified = json!({"value": value, "contract": "place-at-target"});
        if !conditions.is_null() {
            verified["conditions"] = conditions;
        }
        v["outcome"] = json!({"verified": verified});
    }
    if let Some(m) = e.manifest {
        v["deployment"] = json!({"manifest": m});
    }
    v
}

fn m(x: f64) -> Value {
    json!({"value": x, "unit": "m"})
}

/// x within 0.20 m of 5.0 m, judged on `observed`.
fn x_condition(result: &str, observed: &[f64]) -> Value {
    json!([{"name": "x within 0.20 m", "evidence": "x", "op": "between", "min": m(4.8), "max": m(5.2),
            "result": result, "observed": observed.iter().map(|x| m(*x)).collect::<Vec<_>>()}])
}

/// A task from `t0` to `t0 + 10 s` with a goto goal per (offset, verdict, observed x), each reported succeeded.
fn task(
    id: &'static str,
    t0: i64,
    goals: &[(&'static str, i64, &'static str, &[f64])],
) -> Vec<Value> {
    let mut out = vec![ev(E {
        id: Box::leak(format!("{id}-start").into_boxed_str()),
        kind: "task.started",
        t: t0,
        task: Some(id),
        ..Default::default()
    })];
    for (goal, at, verdict, xs) in goals {
        let result = match *verdict {
            "violated" => "false",
            "unknown" => "undetermined",
            _ => "true",
        };
        out.push(ev(E {
            id: Box::leak(format!("{goal}-end").into_boxed_str()),
            kind: "action.ended",
            t: t0 + at * S,
            task: Some(id),
            goal: Some(goal),
            component: Some("goto"),
            reported: Some("succeeded"),
            ..Default::default()
        }));
        out.push(ev(E {
            id: Box::leak(format!("{goal}-verdict").into_boxed_str()),
            kind: "outcome.verified",
            t: t0 + at * S,
            goal: Some(goal),
            component: Some("goto"),
            verified: Some((verdict, x_condition(result, xs))),
            ..Default::default()
        }));
    }
    out.push(ev(E {
        id: Box::leak(format!("{id}-end").into_boxed_str()),
        kind: "task.ended",
        t: t0 + 10 * S,
        task: Some(id),
        reported: Some("succeeded"),
        ..Default::default()
    }));
    out
}

fn load(records: &[Value]) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    let results = store.ingest(records, 1).unwrap();
    assert!(
        results
            .iter()
            .all(|r| !format!("{:?}", r.status).contains("Rejected")),
        "{results:?}"
    );
    store.rebuild().unwrap();
    store
}

fn only(store: &Store) -> IncidentDetail {
    let list = store.incidents(None, None, 100).unwrap();
    assert_eq!(list.len(), 1, "{list:?}");
    store.incident(&list[0].incident_id).unwrap().unwrap()
}

fn no_causal_word(text: &str) {
    let lower = text.to_lowercase();
    for w in CAUSAL {
        assert!(!lower.contains(w), "{w:?} in {text:?}");
    }
}

#[test]
fn the_first_miss_its_place_its_size_and_what_came_after() {
    let mut records = task(
        "t1",
        100 * S,
        &[
            ("g1", 2, "satisfied", &[5.05]),
            ("g2", 3, "violated", &[5.55, 5.5, 5.6]),
            ("g3", 7, "violated", &[4.6]),
        ],
    );
    records.push(ev(E {
        id: "fault-1",
        kind: "fault.raised",
        t: 105 * S,
        task: Some("t1"),
        component: Some("arm"),
        code: Some("gripper_slip"),
        ..Default::default()
    }));
    let d = only(&load(&records));
    let sh = d.shape.unwrap();
    assert_eq!(sh.rules, "shape-2");

    let kinds: Vec<&str> = sh.points.iter().map(|p| p.kind.as_str()).collect();
    assert_eq!(kinds, ["verified_violated", "fault", "verified_violated"]);
    let first = &sh.points[0];
    assert_eq!(first.event_id, "g2-verdict");
    assert_eq!(first.offset_s, Some(3.0));
    assert_eq!(first.fraction, Some(0.3));
    assert!(sh.first_unordered_with.is_empty(), "one clock, 2 s apart");
    let miss = &first.misses[0];
    assert_eq!(miss.side.as_deref(), Some("above"));
    assert!(
        (miss.at_least.unwrap() - 0.3).abs() < 1e-9,
        "the closest record, 5.5 m"
    );

    assert_eq!(sh.goals.verified, 3);
    assert_eq!(sh.goals.violated, 2);
    assert_eq!(sh.goals.spread, "some");
    assert_eq!(sh.goals.first_violated, Some((2, 3)));

    assert_eq!(sh.misses.len(), 1);
    let s = &sh.misses[0];
    assert_eq!((s.goals, s.above, s.below, s.measured), (2, 1, 1, 2));
    let (lo, mid, hi) = s.at_least.unwrap();
    assert!((lo - 0.2).abs() < 1e-9 && (mid - 0.25).abs() < 1e-9 && (hi - 0.3).abs() < 1e-9);
    assert_eq!(s.unit.as_deref(), Some("m"));

    assert_eq!(sh.disagreements.reported_succeeded_verified_violated, 2);
    assert_eq!(
        sh.signature,
        "pick/verified_violated/goto/place-at-target:x"
    );
    assert_eq!(
        sh.headline,
        "first at 3.0 s (30 % of the task): goto goal verified violated (2 of 3 verified goals), x within 0.20 m: \
         above the bound by at least 0.300 m; violated goals: goto 2 of 3; fault gripper_slip (arm) 2.0 s after the \
         first point; 2 goal(s) reported succeeded and verified violated"
    );
    no_causal_word(&sh.headline);
    assert!(sh.notes.is_empty(), "{:?}", sh.notes);
}

#[test]
fn points_closer_than_the_clock_uncertainty_have_no_known_order() {
    let mut records = task("t1", 100 * S, &[("g1", 3, "violated", &[5.5])]);
    records.push(ev(E {
        id: "fault-1",
        kind: "fault.raised",
        t: 103 * S + 200_000_000,
        task: Some("t1"),
        component: Some("arm"),
        code: Some("gripper_slip"),
        uncertainty_ms: Some(250.0),
        ..Default::default()
    }));
    let sh = only(&load(&records)).shape.unwrap();
    assert_eq!(sh.first_unordered_with, ["fault-1"]);
    assert!(
        sh.headline
            .starts_with("first, in an order not known against 1 other point(s),"),
        "{}",
        sh.headline
    );
    assert!(
        sh.headline.contains("at the same time as the first point"),
        "{}",
        sh.headline
    );
    assert_eq!(
        sh.signature, "pick/fault|verified_violated/arm|goto/place-at-target:x",
        "the signature names both, not an arbitrary one"
    );
    assert!(
        sh.notes
            .iter()
            .any(|n| n.contains("not known within clock uncertainty"))
    );
}

#[test]
fn two_clocks_without_stated_uncertainty_cannot_be_ordered() {
    let mut records = task("t1", 100 * S, &[("g1", 3, "violated", &[5.5])]);
    records.push(ev(E {
        id: "fault-1",
        kind: "fault.raised",
        t: 108 * S,
        task: Some("t1"),
        code: Some("e_stop"),
        clock: "plc",
        ..Default::default()
    }));
    let sh = only(&load(&records)).shape.unwrap();
    assert_eq!(
        sh.first_unordered_with,
        ["fault-1"],
        "5 s apart, on clocks that cannot be compared"
    );
}

#[test]
fn a_verdict_without_measured_values_says_so() {
    let mut records = task("t1", 100 * S, &[]);
    records.push(ev(E {
        id: "g1-end",
        kind: "action.ended",
        t: 104 * S,
        task: Some("t1"),
        goal: Some("g1"),
        component: Some("goto"),
        reported: Some("succeeded"),
        ..Default::default()
    }));
    records.push(ev(E {
        id: "g1-verdict",
        kind: "outcome.verified",
        t: 104 * S,
        goal: Some("g1"),
        verified: Some(("violated", Value::Null)),
        ..Default::default()
    }));
    let sh = only(&load(&records)).shape.unwrap();
    assert!(sh.points[0].misses.is_empty());
    assert_eq!(
        sh.points[0].component.as_deref(),
        Some("goto"),
        "the component of the goal's action"
    );
    assert!(
        sh.notes
            .contains(&"1 violated verdict(s) carry no measured values".to_owned()),
        "{:?}",
        sh.notes
    );
    assert!(
        sh.headline
            .ends_with("1 violated verdict(s) carry no measured values"),
        "{}",
        sh.headline
    );
    assert!(sh.misses.is_empty());
    assert_eq!(sh.goals.spread, "all");
}

#[test]
fn a_failed_task_without_goals_has_task_level_points_only() {
    let records = vec![
        ev(E {
            id: "s",
            kind: "task.started",
            t: 100 * S,
            task: Some("t1"),
            ..Default::default()
        }),
        ev(E {
            id: "e",
            kind: "task.ended",
            t: 110 * S,
            task: Some("t1"),
            reported: Some("failed"),
            ..Default::default()
        }),
    ];
    let sh = only(&load(&records)).shape.unwrap();
    assert_eq!(sh.points.len(), 1);
    assert_eq!(
        sh.headline,
        "first at 10.0 s (100 % of the task): task ended failed"
    );
    assert_eq!(sh.goals.verified, 0);
    assert_eq!(sh.goals.spread, "none");
    assert_eq!(sh.signature, "pick/task_failed/-/-");
}

#[test]
fn an_incident_with_nothing_the_events_can_place_says_so() {
    // Opened by an unknown report alone: the task never ended.
    let records = vec![ev(E {
        id: "s",
        kind: "task.started",
        t: 100 * S,
        task: Some("t1"),
        ..Default::default()
    })];
    let sh = only(&load(&records)).shape.unwrap();
    assert!(sh.points.is_empty());
    assert_eq!(sh.headline, "nothing went wrong that the events can place");
    assert_eq!(sh.signature, "pick/none");
}

#[test]
fn the_same_shape_is_counted_per_release_and_first_seen_is_named() {
    let manifest = |rev: &str, release: &str| {
        json!({"schema": "openmhs.deployment/0.1",
               "components": [{"kind": "model", "name": "policy", "revision": rev}],
               "labels": {"release": release}})
    };
    let (m1, m2) = (manifest("a1", "r1"), manifest("b2", "r2"));
    let (d1, d2) = (
        Manifest::parse(&m1).unwrap().digest,
        Manifest::parse(&m2).unwrap().digest,
    );
    let mut records = vec![m1, m2];
    for (id, goal, t0, d) in [
        ("t1", "g1", 100, &d1),
        ("t2", "g2", 200, &d2),
        ("t3", "g3", 300, &d2),
    ] {
        let mut task_records = task(id, t0 * S, &[(goal, 3, "violated", &[5.5])]);
        task_records[0]["deployment"] = json!({"manifest": d});
        records.extend(task_records);
    }
    let store = load(&records);
    let list = store.incidents(None, None, 10).unwrap();
    assert_eq!(list.len(), 3);
    assert!(list.iter().all(|i| i.signature == list[0].signature));
    let d = store.incident(&list[0].incident_id).unwrap().unwrap();
    let r = d.recurrence.unwrap();
    assert_eq!(r.incidents, 3);
    assert_eq!(r.first_release.as_deref(), Some("r1"));
    assert_eq!(r.per_release.get("r1"), Some(&1));
    assert_eq!(r.per_release.get("r2"), Some(&2));
    let first = list.iter().find(|i| i.task_id == "t1").unwrap();
    assert_eq!(r.first_seen, first.incident_id);
}

#[test]
fn the_shape_is_rebuilt_the_same_whatever_the_order_of_arrival() {
    let mut records = task(
        "t1",
        100 * S,
        &[
            ("g1", 2, "satisfied", &[5.05]),
            ("g2", 3, "violated", &[5.55]),
            ("g3", 7, "violated", &[4.6]),
        ],
    );
    let a = only(&load(&records)).shape.unwrap();
    records.reverse();
    let b = only(&load(&records)).shape.unwrap();
    assert_eq!(a, b);
}

/// A failed task's own report, with the value it judged on (ADR-0036).
fn reported_failed_with(observed: f64) -> Value {
    json!({"reported": {"value": "failed", "conditions": [
        {"name": "coverage over 0.95", "evidence": "coverage", "op": "gt", "value": 0.95, "result": "false",
         "observed": [observed]}]}})
}

#[test]
fn a_reported_miss_is_measured_and_said_to_be_the_reporters_own() {
    let mut records = task("t1", 100 * S, &[]);
    let end = records.last_mut().unwrap();
    end["outcome"] = reported_failed_with(0.9);
    let sh = only(&load(&records)).shape.unwrap();
    assert_eq!(sh.points.len(), 1);
    let miss = &sh.points[0].misses[0];
    assert_eq!(miss.source, "reported");
    assert_eq!(miss.contract, None);
    assert_eq!(miss.side.as_deref(), Some("below"));
    assert!((miss.at_least.unwrap() - 0.05).abs() < 1e-9);
    assert_eq!(
        sh.headline,
        "first at 10.0 s (100 % of the task): task ended failed (reported coverage over 0.95: below the bound by at least 0.050)"
    );
    assert_eq!(sh.signature, "pick/task_failed/-/reported:coverage");
    assert_eq!(sh.misses.len(), 1);
    assert_eq!(
        (sh.misses[0].source.as_str(), sh.misses[0].goals),
        ("reported", 1)
    );
    // a self-judgement is not a verdict: no goals, no spread, no disagreement
    assert_eq!((sh.goals.verified, sh.goals.spread.as_str()), (0, "none"));
    assert_eq!(sh.disagreements.reported_succeeded_verified_violated, 0);
    no_causal_word(&sh.headline);
}

#[test]
fn reported_and_verified_misses_are_never_pooled() {
    let mut records = task("t1", 100 * S, &[("g1", 3, "violated", &[5.5])]);
    records.last_mut().unwrap()["outcome"] = reported_failed_with(0.9);
    let sh = only(&load(&records)).shape.unwrap();
    let kinds: Vec<&str> = sh.points.iter().map(|p| p.kind.as_str()).collect();
    assert_eq!(kinds, ["verified_violated", "task_failed"]);
    assert_eq!(sh.points[0].misses[0].source, "verified");
    assert_eq!(sh.points[1].misses[0].source, "reported");
    let sources: Vec<(&str, u64)> = sh
        .misses
        .iter()
        .map(|m| (m.source.as_str(), m.goals))
        .collect();
    assert_eq!(sources, [("reported", 1), ("verified", 1)]);
    assert_eq!((sh.goals.verified, sh.goals.violated), (1, 1));
    assert_eq!(
        sh.signature, "pick/verified_violated/goto/place-at-target:x",
        "a shape-1 signature where the first point is a verdict"
    );
}

#[test]
fn a_tiny_miss_never_prints_as_zero() {
    let mut records = task("t1", 100 * S, &[]);
    records.last_mut().unwrap()["outcome"] = reported_failed_with(0.9497);
    let sh = only(&load(&records)).shape.unwrap();
    assert!(
        sh.headline
            .ends_with("below the bound by at least 0.00030)"),
        "{}",
        sh.headline
    );
}

#[test]
fn a_success_reported_with_a_failing_condition_is_refused() {
    let mut records = task("t1", 100 * S, &[]);
    let mut outcome = reported_failed_with(0.9);
    outcome["reported"]["value"] = json!("succeeded");
    records.last_mut().unwrap()["outcome"] = outcome;
    let mut store = Store::open_in_memory().unwrap();
    let results = store.ingest(&records, 1).unwrap();
    assert!(
        format!("{:?}", results.last().unwrap().status).contains("Rejected"),
        "{results:?}"
    );
}
