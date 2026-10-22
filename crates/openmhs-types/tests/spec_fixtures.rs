//! M1 acceptance: every valid example passes, every invalid example fails with exactly
//! its expected error code, and the other schemas accept their examples.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use openmhs_types::schema::{self, Schema};
use openmhs_types::validate_descriptor;
use serde::Deserialize;

fn spec_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec")
}

fn read_json(path: &Path) -> serde_json::Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("list {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    files
}

#[test]
fn valid_descriptors_pass() {
    let files = json_files(&spec_dir().join("examples/valid"));
    assert!(
        files.len() >= 4,
        "expected the three device examples plus the fixture base"
    );
    for path in files {
        if let Err(issues) = validate_descriptor(&read_json(&path)) {
            let lines: Vec<String> = issues.iter().map(ToString::to_string).collect();
            panic!(
                "{} should be valid:\n  {}",
                path.display(),
                lines.join("\n  ")
            );
        }
    }
}

#[test]
fn valid_descriptors_round_trip_through_types() {
    for path in json_files(&spec_dir().join("examples/valid")) {
        let parsed = validate_descriptor(&read_json(&path)).expect("valid");
        let reserialized = serde_json::to_value(&parsed).expect("serialize");
        assert!(
            schema::validate(Schema::Descriptor, &reserialized).is_empty(),
            "{} no longer matches the schema after a round trip",
            path.display()
        );
        assert_eq!(
            validate_descriptor(&reserialized).expect("still valid"),
            parsed
        );
    }
}

#[derive(Deserialize)]
struct Expected {
    code: String,
    pointer: Option<String>,
}

#[test]
fn invalid_descriptors_fail_with_exactly_the_expected_code() {
    let dir = spec_dir().join("examples/invalid");
    let expected: BTreeMap<String, Expected> =
        serde_json::from_value(read_json(&dir.join("expected.json"))).expect("expected.json shape");

    let fixtures: BTreeSet<String> = json_files(&dir)
        .iter()
        .map(|p| p.file_name().expect("name").to_string_lossy().into_owned())
        .filter(|n| n != "expected.json")
        .collect();
    let listed: BTreeSet<String> = expected.keys().cloned().collect();
    assert_eq!(
        fixtures, listed,
        "every invalid fixture must be listed in expected.json and vice versa"
    );
    assert!(fixtures.len() >= 10);

    let mut failures = Vec::new();
    for (file, exp) in &expected {
        let issues = match validate_descriptor(&read_json(&dir.join(file))) {
            Ok(_) => {
                failures.push(format!("{file}: accepted, expected {}", exp.code));
                continue;
            }
            Err(issues) => issues,
        };
        let codes: BTreeSet<&str> = issues.iter().map(|i| i.code.as_str()).collect();
        if codes != BTreeSet::from([exp.code.as_str()]) {
            failures.push(format!(
                "{file}: expected only {}, got {codes:?}: {issues:?}",
                exp.code
            ));
            continue;
        }
        if let Some(ptr) = &exp.pointer {
            if !issues.iter().any(|i| i.pointer.starts_with(ptr.as_str())) {
                failures.push(format!("{file}: no issue at {ptr}: {issues:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn documented_descriptor_codes_match_fixtures() {
    // Keeps spec/errors.md honest: each descriptor.* code it documents must be produced
    // by some fixture, and each code a fixture produces must be documented.
    let doc = fs::read_to_string(spec_dir().join("errors.md")).expect("errors.md");
    let documented: BTreeSet<&str> = doc
        .split('`')
        .filter(|s| s.starts_with("descriptor.") && !s.contains(' '))
        .collect();
    let dir = spec_dir().join("examples/invalid");
    let expected: BTreeMap<String, Expected> =
        serde_json::from_value(read_json(&dir.join("expected.json"))).expect("expected.json shape");
    let exercised: BTreeSet<&str> = expected.values().map(|e| e.code.as_str()).collect();
    assert_eq!(documented, exercised);
}

#[test]
fn command_examples() {
    for path in json_files(&spec_dir().join("examples/commands/valid")) {
        let issues = schema::validate(Schema::Command, &read_json(&path));
        assert!(issues.is_empty(), "{}: {issues:?}", path.display());
    }
    for path in json_files(&spec_dir().join("examples/commands/invalid")) {
        let issues = schema::validate(Schema::Command, &read_json(&path));
        assert!(!issues.is_empty(), "{} should be rejected", path.display());
        assert!(issues.iter().all(|i| i.code == "command.schema"));
    }
}

#[test]
fn receipt_and_event_examples() {
    for (sub, schema) in [("receipts", Schema::Receipt), ("events", Schema::Event)] {
        let files = json_files(&spec_dir().join("examples").join(sub));
        assert!(!files.is_empty());
        for path in files {
            let issues = schema::validate(schema, &read_json(&path));
            assert!(issues.is_empty(), "{}: {issues:?}", path.display());
        }
    }
}

#[test]
fn all_schemas_compile() {
    for s in Schema::ALL {
        // An empty object is invalid for every schema except it must not panic.
        let _ = schema::validate(s, &serde_json::json!({}));
    }
}
