//! `openmhs outcome`: check a declared physical outcome against evidence (RFC 0004). Offline: never opens a
//! gateway, a journal or a device. The verdict comes from deterministic rules, never from a model.

use std::path::{Path, PathBuf};

use openmhs_outcome::{ConditionResult, Contract, Record, Verdict, audit, parse_evidence, verify};

use crate::exit;

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn bundle_records(path: &Path) -> Result<(Vec<Record>, Vec<String>), String> {
    let bundle: serde_json::Value =
        serde_json::from_str(&read(path)?).map_err(|e| format!("{}: {e}", path.display()))?;
    audit::records_from_bundle(&bundle).map_err(|e| format!("{}: {e}", path.display()))
}

/// `outcome verify`. Exits 0 SATISFIED, 3 VIOLATED, 4 UNKNOWN, 1 if the inputs cannot be checked at all.
pub fn verify_cmd(
    json: bool,
    contract: &Path,
    evidence: &[PathBuf],
    audit_bundles: &[PathBuf],
) -> u8 {
    let run = || -> Result<openmhs_outcome::Report, String> {
        let contract: Contract = serde_json::from_str(&read(contract)?)
            .map_err(|e| format!("{}: {e}", contract.display()))?;
        let mut records = Vec::new();
        for path in evidence {
            records.extend(
                parse_evidence(&read(path)?).map_err(|e| format!("{}: {e}", path.display()))?,
            );
        }
        for path in audit_bundles {
            records.extend(bundle_records(path)?.0);
        }
        verify(&contract, &records).map_err(|e| e.to_string())
    };
    let report = match run() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::ERROR;
        }
    };
    if json {
        println!("{}", String::from_utf8_lossy(&report.to_canonical_json()));
    } else {
        println!(
            "{} {}@{}",
            match report.verdict {
                Verdict::Satisfied => "SATISFIED",
                Verdict::Violated => "VIOLATED",
                Verdict::Unknown => "UNKNOWN",
            },
            report.contract["name"].as_str().unwrap_or("?"),
            report.contract["version"].as_str().unwrap_or("?"),
        );
        for c in &report.conditions {
            let mark = match c.result {
                ConditionResult::True => "true        ",
                ConditionResult::False => "FALSE       ",
                ConditionResult::Undetermined => "undetermined",
            };
            let label = c
                .name
                .clone()
                .unwrap_or_else(|| format!("{} {:?}", c.evidence, c.op));
            println!("  {mark} {label}  [{}]", c.records.join(", "));
        }
        for r in &report.reasons {
            let at = r
                .record
                .as_deref()
                .map(|id| format!(" ({id})"))
                .unwrap_or_default();
            println!("  because {}{at}: {}", r.code, r.detail);
        }
        for (name, item) in &report.items {
            if item.provenance.iter().any(|p| p == "simulator") {
                println!("  note: evidence '{name}' says it came from a simulator");
            }
        }
        println!(
            "  contract sha256 {}, evidence sha256 {}",
            report.contract["sha256"].as_str().unwrap_or("?"),
            report.evidence["sha256"].as_str().unwrap_or("?"),
        );
    }
    match report.verdict {
        Verdict::Satisfied => exit::OK,
        Verdict::Violated => exit::FAILED,
        Verdict::Unknown => exit::UNCERTAIN,
    }
}

/// `outcome from-audit`: an audit export as evidence records, one per line, on stdout. What could not become
/// a record is said on stderr, never made up.
pub fn from_audit_cmd(bundle: &Path) -> u8 {
    match bundle_records(bundle) {
        Ok((records, skipped)) => {
            for r in &records {
                match serde_json::to_string(r) {
                    Ok(line) => println!("{line}"),
                    Err(e) => {
                        eprintln!("error: {e}");
                        return exit::ERROR;
                    }
                }
            }
            for s in &skipped {
                eprintln!("skipped: {s}");
            }
            exit::OK
        }
        Err(e) => {
            eprintln!("error: {e}");
            exit::ERROR
        }
    }
}
