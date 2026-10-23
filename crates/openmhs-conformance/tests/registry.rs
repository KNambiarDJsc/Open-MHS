//! spec/conformance.md and the suite's requirement registry say the same thing.

use openmhs_conformance::{ConformanceLevel, Fault, REQUIREMENTS};

const SPEC: &str = include_str!("../../../spec/conformance.md");

fn level(s: &str) -> ConformanceLevel {
    match s {
        "L0" => ConformanceLevel::L0,
        "L1" => ConformanceLevel::L1,
        "L2" => ConformanceLevel::L2,
        "L3" => ConformanceLevel::L3,
        other => panic!("unknown level {other}"),
    }
}

fn fault(s: &str) -> Option<Fault> {
    let s = s.trim_matches('`');
    if s.is_empty() {
        return None;
    }
    Some(
        Fault::ALL
            .into_iter()
            .find(|f| f.name() == s)
            .unwrap_or_else(|| panic!("unknown fault {s}")),
    )
}

#[test]
fn the_spec_table_matches_the_registry() {
    let rows: Vec<(String, ConformanceLevel, Option<Fault>, String)> = SPEC
        .lines()
        .filter(|l| l.starts_with("| `L"))
        .map(|l| {
            let cells: Vec<&str> = l
                .trim()
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            assert_eq!(cells.len(), 4, "{l}");
            (
                cells[0].trim_matches('`').to_owned(),
                level(cells[1]),
                fault(cells[2]),
                cells[3].to_owned(),
            )
        })
        .collect();
    let registry: Vec<(String, ConformanceLevel, Option<Fault>, String)> = REQUIREMENTS
        .iter()
        .map(|r| (r.id.to_owned(), r.level, r.fault, r.text.to_owned()))
        .collect();
    assert_eq!(rows, registry);
}

#[test]
fn the_report_schema_knows_every_fault_and_level() {
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../spec/schemas/conformance-report.schema.json"
    ))
    .unwrap();
    let names = |def: &str| -> Vec<String> {
        schema["$defs"][def]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    };
    let faults: Vec<String> = Fault::ALL.iter().map(|f| f.name().to_owned()).collect();
    assert_eq!(names("fault"), faults);
    let levels: Vec<String> = ConformanceLevel::ALL
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(names("level"), levels);
}
