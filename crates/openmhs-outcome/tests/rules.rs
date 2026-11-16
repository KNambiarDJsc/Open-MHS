//! The edges the corpus does not reach: inputs that cannot be checked at all, the anchor, operators,
//! and reading an OpenMHS audit export.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

use openmhs_outcome::{InputError, Verdict, audit, parse_evidence, verify};
use serde_json::json;

fn contract(v: serde_json::Value) -> openmhs_outcome::Contract {
    serde_json::from_value(v).unwrap()
}

fn base() -> serde_json::Value {
    json!({
        "contract": "c", "version": "0.1",
        "timing": { "anchor": "a", "default_window_ms": [0, 1000] },
        "evidence": {
            "a": { "signal": "act", "kind": "controller" },
            "s": { "signal": "s", "kind": "device_state" }
        },
        "conditions": [ { "evidence": "s", "op": "eq", "value": 1 } ]
    })
}

fn rec(
    id: &str,
    signal: &str,
    kind: &str,
    value: serde_json::Value,
    at: &str,
) -> serde_json::Value {
    json!({ "id": id, "signal": signal, "value": value, "source": { "kind": kind },
            "time": { "source": at, "clock_domain": "h" } })
}

fn records(v: &[serde_json::Value]) -> Vec<openmhs_outcome::Record> {
    v.iter()
        .map(|r| serde_json::from_value(r.clone()).unwrap())
        .collect()
}

const T0: &str = "2026-11-10T09:00:00.000Z";
const T1: &str = "2026-11-10T09:00:00.500Z";

type Change = Box<dyn Fn(&mut serde_json::Value)>;

#[test]
fn a_contract_that_cannot_be_checked_is_an_error_not_a_verdict() {
    let cases: Vec<(&str, Change)> = vec![
        (
            "anchor is not an item",
            Box::new(|c| c["timing"]["anchor"] = json!("nope")),
        ),
        (
            "condition names no item",
            Box::new(|c| c["conditions"][0]["evidence"] = json!("nope")),
        ),
        (
            "operand missing",
            Box::new(|c| {
                c["conditions"][0].as_object_mut().unwrap().remove("value");
            }),
        ),
        (
            "empty window",
            Box::new(|c| c["timing"]["default_window_ms"] = json!([10, 0])),
        ),
        ("no conditions", Box::new(|c| c["conditions"] = json!([]))),
        (
            "window without timing",
            Box::new(|c| {
                c.as_object_mut().unwrap().remove("timing");
                c["evidence"]["s"]["window_ms"] = json!([0, 1]);
            }),
        ),
        (
            "negative uncertainty",
            Box::new(|c| c["timing"]["max_clock_uncertainty_ms"] = json!(-1)),
        ),
    ];
    for (what, change) in cases {
        let mut c = base();
        change(&mut c);
        let r = verify(&contract(c), &[]);
        assert!(matches!(r, Err(InputError::Contract(_))), "{what}: {r:?}");
    }
}

#[test]
fn evidence_that_cannot_be_read_is_an_error() {
    let dup = records(&[
        rec("x", "s", "device_state", json!(1), T0),
        rec("x", "s", "device_state", json!(1), T1),
    ]);
    assert!(matches!(
        verify(&contract(base()), &dup),
        Err(InputError::Evidence(_))
    ));
    // A misspelt field is refused, not ignored.
    assert!(
        parse_evidence(
            r#"{"id":"a","signal":"s","value":1,"source":{"kind":"controller"},"qualty":"BAD"}"#
        )
        .is_err()
    );
    assert!(
        parse_evidence(r#"{"id":"a","signal":"s","value":1,"source":{"kind":"robot"}}"#).is_err()
    );
    let not_cal = json!({ "id": "c", "signal": "s", "value": 1, "source": { "kind": "device_state" },
        "calibration": { "name": "n", "applies_to": "d", "valid_from": T0, "valid_until": T1 } });
    assert!(matches!(
        verify(&contract(base()), &records(&[not_cal])),
        Err(InputError::Evidence(_))
    ));
}

#[test]
fn two_anchors_at_different_times_place_no_window() {
    let r = records(&[
        rec("a1", "act", "controller", json!("SUCCEEDED"), T0),
        rec("a2", "act", "controller", json!("SUCCEEDED"), T1),
        rec("s1", "s", "device_state", json!(1), T1),
    ]);
    let report = verify(&contract(base()), &r).unwrap();
    assert_eq!(report.verdict, Verdict::Unknown);
    assert!(
        report
            .reasons
            .iter()
            .any(|r| r.code == "timing.anchor_ambiguous"),
        "{:?}",
        report.reasons
    );
}

#[test]
fn a_time_that_is_not_a_time_rejects_the_record() {
    let r = records(&[
        rec("a1", "act", "controller", json!("SUCCEEDED"), T0),
        rec("s1", "s", "device_state", json!(1), "yesterday"),
    ]);
    let report = verify(&contract(base()), &r).unwrap();
    assert_eq!(report.verdict, Verdict::Unknown);
    assert_eq!(report.items["s"].rejected[0].code, "time.invalid");
}

#[test]
fn operators() {
    let check = |cond: serde_json::Value, value: serde_json::Value| {
        let mut c = base();
        c["conditions"] = json!([cond]);
        let r = records(&[
            rec("a1", "act", "controller", json!("OK"), T0),
            rec("s1", "s", "device_state", value, T1),
        ]);
        verify(&contract(c), &r).unwrap().verdict
    };
    let s = |op: &str, v| json!({ "evidence": "s", "op": op, "value": v });
    assert_eq!(check(s("lt", json!(2)), json!(1)), Verdict::Satisfied);
    assert_eq!(check(s("ge", json!(2)), json!(1)), Verdict::Violated);
    assert_eq!(
        check(s("ne", json!("idle")), json!("busy")),
        Verdict::Satisfied
    );
    assert_eq!(
        check(
            json!({ "evidence": "s", "op": "in", "values": ["a", "b"] }),
            json!("b")
        ),
        Verdict::Satisfied
    );
    assert_eq!(
        check(
            json!({ "evidence": "s", "op": "in", "values": ["a", "b"] }),
            json!("c")
        ),
        Verdict::Violated
    );
    // A string compared with a number is not false: it cannot be compared.
    assert_eq!(check(s("eq", json!(1)), json!("1")), Verdict::Unknown);
    // Ordering a string is not possible either.
    assert_eq!(check(s("lt", json!("b")), json!("a")), Verdict::Unknown);
    // A bare number is not a quantity, and the other way round.
    assert_eq!(
        check(s("lt", json!({"value": 2, "unit": "V"})), json!(1)),
        Verdict::Unknown
    );
    assert_eq!(
        check(s("lt", json!(2)), json!({"value": 1, "unit": "V"})),
        Verdict::Unknown
    );
}

#[test]
fn no_timing_means_no_windows() {
    let mut c = base();
    c.as_object_mut().unwrap().remove("timing");
    let r = records(&[
        json!({ "id": "s1", "signal": "s", "value": 1, "source": { "kind": "device_state" } }),
    ]);
    assert_eq!(
        verify(&contract(c), &r).unwrap().verdict,
        Verdict::Satisfied
    );
}

#[test]
fn an_audit_export_becomes_evidence_without_inventing_values() {
    let read = |seq: i64, data: serde_json::Value| {
        json!({
            "seq": seq, "at_ms": 1_793_000_000_000i64, "kind": "read", "subject": "sim.psu-01",
            "data": data.to_string(), "prev_hash": "", "hash": ""
        })
    };
    let bundle = json!({
        "version": 1,
        "audit": [
            read(1, json!({ "capability": "power.output.voltage.measured",
                "state": { "value": { "value": 2.0, "unit": "V" }, "timestamp": T1, "source": "simulator", "quality": "GOOD" } })),
            read(2, json!({ "capability": "power.output.voltage.measured", "error": "adapter.disconnected", "message": "x" })),
            read(3, json!({ "capability": "power.output.voltage.measured",
                "state": { "value": 1.0, "timestamp": T1, "source": "device", "quality": "BAD" } })),
            { "seq": 4, "at_ms": 0, "kind": "token.created", "subject": "t", "data": "{}", "prev_hash": "", "hash": "" }
        ],
        "receipts": [ {
            "command_id": "c1", "correlation_id": "attempt-1",
            "request": { "target": { "device": "sim.psu-01", "capability": "power.output.voltage.setpoint" } },
            "state": "VERIFIED",
            "transitions": [ { "state": "CREATED", "at": T0 }, { "state": "VERIFIED", "at": T1 } ]
        } ],
        "checkpoint": {}
    });
    let (records, skipped) = audit::records_from_bundle(&bundle).unwrap();
    assert_eq!(records.len(), 3, "{records:#?}");
    assert_eq!(skipped.len(), 1, "the failed read: {skipped:?}");
    assert!(skipped[0].contains("failed"));
    let sim = records.iter().find(|r| r.id == "audit-1").unwrap();
    assert_eq!(
        sim.source.provenance,
        Some(openmhs_outcome::Provenance::Simulator)
    );
    assert_eq!(
        sim.time.as_ref().unwrap().clock_domain.as_deref(),
        Some(audit::GATEWAY_CLOCK)
    );
    let dev = records.iter().find(|r| r.id == "audit-3").unwrap();
    assert_eq!(dev.quality, openmhs_outcome::Quality::Bad);
    assert_eq!(
        dev.time.as_ref().unwrap().clock_domain.as_deref(),
        Some("device:sim.psu-01")
    );
    let cmd = records.iter().find(|r| r.id == "command-c1").unwrap();
    assert_eq!(cmd.value, json!("VERIFIED"));
    assert_eq!(cmd.refs["correlation"], "attempt-1");
    assert_eq!(
        cmd.time.as_ref().unwrap().source.as_deref(),
        Some(T1),
        "the time it reached its state"
    );
    assert!(audit::records_from_bundle(&json!({ "audit": [] })).is_err());
}

#[test]
fn a_record_in_another_frame_is_not_evidence_and_is_never_transformed() {
    let mut c = base();
    c["evidence"]["s"]["frame"] = json!("map");
    let mut in_odom = rec("s1", "s", "device_state", json!(1), T1);
    in_odom["frame"] = json!("odom");
    let mut no_frame = rec("s2", "s", "device_state", json!(1), T1);
    no_frame.as_object_mut().unwrap().remove("frame");
    let anchor = rec("a1", "act", "controller", json!("OK"), T0);
    let report = verify(
        &contract(c.clone()),
        &records(&[anchor.clone(), in_odom, no_frame]),
    )
    .unwrap();
    assert_eq!(report.verdict, Verdict::Unknown);
    let codes: Vec<&str> = report.items["s"]
        .rejected
        .iter()
        .map(|r| r.code.as_str())
        .collect();
    assert_eq!(codes, ["evidence.frame", "evidence.frame"]);
    let mut in_map = rec("s3", "s", "device_state", json!(1), T1);
    in_map["frame"] = json!("map");
    let report = verify(&contract(c), &records(&[anchor, in_map])).unwrap();
    assert_eq!(report.verdict, Verdict::Satisfied);
}

#[test]
fn when_the_anchor_is_discarded_the_report_says_why() {
    let mut c = base();
    c["evidence"]["a"]["calibration"] = json!(["cal"]);
    let mut anchor = rec("a1", "act", "controller", json!("OK"), T1);
    anchor["source"]["device"] = json!("d");
    let cal = json!({ "id": "c1", "signal": "calibration", "value": "cal",
        "source": { "kind": "calibration", "device": "d" },
        "calibration": { "name": "cal", "applies_to": "d", "valid_from": "2026-01-01T00:00:00Z", "valid_until": T0 } });
    let r = records(&[anchor, cal, rec("s1", "s", "device_state", json!(1), T1)]);
    let report = verify(&contract(c), &r).unwrap();
    assert_eq!(report.verdict, Verdict::Unknown);
    let codes: Vec<&str> = report.reasons.iter().map(|r| r.code.as_str()).collect();
    assert!(codes.contains(&"timing.anchor_missing"), "{codes:?}");
    assert!(codes.contains(&"calibration.expired"), "{codes:?}");
}

/// A sensor sampled every 10 ms for `n` samples from `start_ms`, holding `value` until `flip_at` (sample index), then
/// a different one, with fresh timestamps throughout.
fn feed(start_ms: u32, n: u32, flip_at: u32) -> Vec<serde_json::Value> {
    (0..n)
        .map(|i| {
            let at = format!(
                "2026-11-10T09:00:{:02}.{:03}Z",
                (start_ms + i * 10) / 1000,
                (start_ms + i * 10) % 1000
            );
            let value = if i < flip_at { 1 } else { 2 };
            rec(&format!("s{i:03}"), "s", "device_state", json!(value), &at)
        })
        .collect()
}

#[test]
fn a_value_repeated_for_longer_than_the_item_allows_is_stale_however_fresh_its_timestamp() {
    // The sensor reads 1 for 600 ms, and the claim comes at the end of that stretch.
    let mut c = base();
    c["timing"]["default_window_ms"] = json!([-50, 50]);
    c["evidence"]["s"]["bound"] = json!(false);
    let anchor = rec(
        "a1",
        "act",
        "controller",
        json!("OK"),
        "2026-11-10T09:00:00.550Z",
    );
    let mut all = feed(0, 61, u32::MAX);
    all.push(anchor.clone());

    // Without the opt-in the repeated value satisfies the contract.
    let plain = verify(&contract(c.clone()), &records(&all)).unwrap();
    assert_eq!(plain.verdict, Verdict::Satisfied);

    // With it, the same evidence is not evidence.
    c["evidence"]["s"]["max_unchanged_ms"] = json!(200);
    let report = verify(&contract(c), &records(&all)).unwrap();
    assert_eq!(report.verdict, Verdict::Unknown);
    let stale: Vec<_> = report.items["s"]
        .rejected
        .iter()
        .filter(|r| r.code == "evidence.stale")
        .collect();
    // Samples at 210 ms and later are more than 200 ms into the run: 21 to 60, forty records.
    assert_eq!(stale.len(), 40);
    assert!(
        stale[0].detail.contains("unchanged for"),
        "{}",
        stale[0].detail
    );
    assert!(report.items["s"].accepted.is_empty());
}

#[test]
fn a_live_sensor_whose_value_changes_is_never_stale() {
    // Changes every 100 ms: no run reaches 200 ms.
    let mut c = base();
    c["timing"]["default_window_ms"] = json!([-50, 50]);
    c["evidence"]["s"]["bound"] = json!(false);
    c["evidence"]["s"]["max_unchanged_ms"] = json!(200);
    c["conditions"] = json!([{ "evidence": "s", "op": "ge", "value": 1 }]);
    let mut all: Vec<_> = (0..40u32)
        .map(|i| {
            let ms = i * 25;
            let at = format!("2026-11-10T09:00:00.{ms:03}Z");
            rec(
                &format!("s{i:03}"),
                "s",
                "device_state",
                json!(1 + (i / 4) % 2),
                &at,
            )
        })
        .collect();
    all.push(rec(
        "a1",
        "act",
        "controller",
        json!("OK"),
        "2026-11-10T09:00:00.500Z",
    ));
    let report = verify(&contract(c), &records(&all)).unwrap();
    assert_eq!(
        report.verdict,
        Verdict::Satisfied,
        "{:?}",
        report.items["s"].rejected
    );
    assert!(
        report.items["s"]
            .rejected
            .iter()
            .all(|r| r.code != "evidence.stale")
    );
}

#[test]
fn staleness_is_judged_where_the_value_last_changed_not_from_the_start_of_the_evidence() {
    // 1 for 300 ms, then 2 for 300 ms: at the claim (t = 550) the run of 2 is 250 ms old, and the window (500 to 600 ms)
    // holds records 200 to 300 ms into it.
    let mut c = base();
    c["timing"]["default_window_ms"] = json!([-50, 50]);
    c["evidence"]["s"]["bound"] = json!(false);
    c["conditions"] = json!([{ "evidence": "s", "op": "ge", "value": 1 }]);
    let mut all = feed(0, 61, 30);
    all.push(rec(
        "a1",
        "act",
        "controller",
        json!("OK"),
        "2026-11-10T09:00:00.550Z",
    ));
    c["evidence"]["s"]["max_unchanged_ms"] = json!(400);
    assert_eq!(
        verify(&contract(c.clone()), &records(&all))
            .unwrap()
            .verdict,
        Verdict::Satisfied
    );
    c["evidence"]["s"]["max_unchanged_ms"] = json!(100);
    assert_eq!(
        verify(&contract(c), &records(&all)).unwrap().verdict,
        Verdict::Unknown
    );
}

#[test]
fn a_zero_staleness_limit_is_a_contract_error() {
    let mut c = base();
    c["evidence"]["s"]["max_unchanged_ms"] = json!(0);
    let err = serde_json::from_value::<openmhs_outcome::Contract>(c)
        .unwrap()
        .check()
        .unwrap_err();
    assert!(err.to_string().contains("max_unchanged_ms"), "{err}");
}
