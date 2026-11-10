//! The fault corpus in research/outcome-verifier (EVALUATION.md, Q1 to Q3): every case gets its expected
//! verdict and reason, reports are deterministic, and nothing is SATISFIED that a rule should have stopped.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use openmhs_outcome::{Contract, Record, Verdict, parse_evidence, verify};

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../research/outcome-verifier/corpus/cases")
}

struct Case {
    name: String,
    contract: Contract,
    records: Vec<Record>,
    verdict: Verdict,
    reason: Option<String>,
}

fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    for domain in std::fs::read_dir(corpus()).expect("the corpus exists") {
        let domain = domain.unwrap().path();
        for case in std::fs::read_dir(&domain).unwrap() {
            let dir = case.unwrap().path();
            let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap();
            let expected: serde_json::Value = serde_json::from_str(&read("expected.json")).unwrap();
            out.push(Case {
                name: format!(
                    "{}/{}",
                    domain.file_name().unwrap().to_string_lossy(),
                    dir.file_name().unwrap().to_string_lossy()
                ),
                contract: serde_json::from_str(&read("contract.json")).unwrap(),
                records: parse_evidence(&read("evidence.jsonl")).unwrap(),
                verdict: serde_json::from_value(expected["verdict"].clone()).unwrap(),
                reason: expected["reason"].as_str().map(str::to_owned),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[test]
fn every_case_gets_its_expected_verdict_and_reason() {
    let cases = cases();
    assert_eq!(cases.len(), 27, "the corpus EVALUATION.md describes");
    let mut wrong = Vec::new();
    for c in &cases {
        let report = verify(&c.contract, &c.records).unwrap();
        let codes: Vec<&str> = report.reasons.iter().map(|r| r.code.as_str()).collect();
        let reason_ok = c.reason.as_deref().is_none_or(|want| codes.contains(&want));
        if report.verdict != c.verdict || !reason_ok {
            wrong.push(format!(
                "{}: got {:?} {codes:?}, expected {:?} {:?}",
                c.name, report.verdict, c.verdict, c.reason
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn nothing_is_satisfied_unless_expected() {
    for c in cases() {
        let report = verify(&c.contract, &c.records).unwrap();
        if report.verdict == Verdict::Satisfied {
            assert_eq!(c.verdict, Verdict::Satisfied, "false SATISFIED: {}", c.name);
        }
    }
}

#[test]
fn reports_are_identical_whatever_order_the_records_arrive_in() {
    for c in cases() {
        let first = verify(&c.contract, &c.records).unwrap().to_canonical_json();
        let mut records = c.records.clone();
        for round in 0..100 {
            // A deterministic shuffle, different each round.
            let n = records.len();
            for i in 0..n {
                records.swap(i, (i * 7 + round * 13 + 5) % n);
            }
            let again = verify(&c.contract, &records).unwrap().to_canonical_json();
            assert_eq!(first, again, "{} differs on round {round}", c.name);
        }
    }
}

#[test]
fn a_simulator_is_never_reported_as_a_device() {
    let c = cases()
        .into_iter()
        .find(|c| c.name == "pick_place/camera_is_a_simulator")
        .unwrap();
    let report = verify(&c.contract, &c.records).unwrap();
    assert_eq!(report.items["x"].provenance, vec!["simulator".to_owned()]);
}
