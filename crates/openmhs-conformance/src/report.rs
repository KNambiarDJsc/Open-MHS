//! The machine-readable conformance report (`spec/schemas/conformance-report.schema.json`).

use openmhs_types::descriptor::ConformanceLevel;
use openmhs_types::{Source, Timestamp};
use serde::{Deserialize, Serialize};

use crate::fixture::Fault;

/// Version of the report format.
pub const REPORT_VERSION: &str = "0.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Fail,
    /// Could not be tested, e.g. the fixture cannot inject the fault it needs. Blocks the
    /// level like a failure: nothing is assumed.
    Untested,
    /// Does not apply to this device, e.g. no-retry checks for an idempotent capability.
    NotApplicable,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Fail => "fail",
            Status::Untested => "untested",
            Status::NotApplicable => "not_applicable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckResult {
    /// Requirement id from spec/conformance.md, e.g. `L2.timeout`.
    pub id: String,
    pub level: ConformanceLevel,
    /// The capability the check exercised, for per-capability requirements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    pub status: Status,
    /// What happened, and for failures, what to fix.
    pub message: String,
    /// Observations backing the result: states, error codes, readings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<serde_json::Value>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    /// What the adapter calls itself (`adapter_id`).
    pub adapter: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// SHA-256 of the descriptor as described, so a report can be tied to one version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor_sha256: Option<String>,
    /// Sources the readings named. `simulator` means a simulator was tested, not hardware.
    #[serde(default)]
    pub sources: Vec<Source>,
}

/// Where the suite ran, so a report can be read years later without guessing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    /// Version of the OpenMHS runtime that ran the suite (the suite ships with it).
    pub runtime_version: String,
    /// Operating system, as Rust names it: `windows`, `linux`, `macos`.
    pub os: String,
    /// CPU architecture: `x86_64`, `aarch64`.
    pub arch: String,
}

impl Environment {
    /// The machine running now.
    pub fn current() -> Self {
        Self {
            runtime_version: env!("CARGO_PKG_VERSION").into(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureInfo {
    /// Faults the fixture could inject.
    pub faults: Vec<Fault>,
    pub counts_deliveries: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub pass: usize,
    pub fail: usize,
    pub untested: usize,
    pub not_applicable: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub openmhs_conformance: String,
    pub suite_version: String,
    pub subject: Subject,
    pub fixture: FixtureInfo,
    /// Absent from reports written before M10.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<Environment>,
    /// The level the descriptor claims (`adapter.conformance`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared: Option<ConformanceLevel>,
    /// The highest level reached, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub achieved: Option<ConformanceLevel>,
    /// The declared level was reached (or, with none declared, at least L0).
    pub passed: bool,
    pub started_at: Timestamp,
    pub duration_ms: u64,
    pub summary: Summary,
    pub checks: Vec<CheckResult>,
}

impl Report {
    /// Checks that keep the subject from reaching `level`: failed or untested checks at
    /// that level or below.
    pub fn blockers(&self, level: ConformanceLevel) -> Vec<&CheckResult> {
        self.checks
            .iter()
            .filter(|c| c.level <= level && matches!(c.status, Status::Fail | Status::Untested))
            .collect()
    }

    /// What the report is judged against: the declared level, or the next one up.
    pub fn target(&self) -> ConformanceLevel {
        self.declared.unwrap_or(match self.achieved {
            None => ConformanceLevel::L0,
            Some(ConformanceLevel::L0) => ConformanceLevel::L1,
            Some(ConformanceLevel::L1) => ConformanceLevel::L2,
            Some(_) => ConformanceLevel::L3,
        })
    }
}

/// A level is reached when it and every level below it have no failed or untested
/// checks, and at least one check at it passed. Levels are cumulative.
pub fn achieved_level(checks: &[CheckResult]) -> Option<ConformanceLevel> {
    let mut achieved = None;
    for level in ConformanceLevel::ALL {
        let at = || checks.iter().filter(move |c| c.level == level);
        let clean = at().all(|c| matches!(c.status, Status::Pass | Status::NotApplicable));
        let evidence = at().any(|c| c.status == Status::Pass);
        if !(clean && evidence) {
            break;
        }
        achieved = Some(level);
    }
    achieved
}

pub fn summarize(checks: &[CheckResult]) -> Summary {
    let mut s = Summary::default();
    for c in checks {
        match c.status {
            Status::Pass => s.pass += 1,
            Status::Fail => s.fail += 1,
            Status::Untested => s.untested += 1,
            Status::NotApplicable => s.not_applicable += 1,
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use ConformanceLevel::*;

    fn check(level: ConformanceLevel, status: Status) -> CheckResult {
        CheckResult {
            id: format!("{level}.x"),
            level,
            capability: None,
            status,
            message: String::new(),
            evidence: None,
            duration_ms: 0,
        }
    }

    #[test]
    fn levels_are_cumulative_and_need_evidence() {
        use Status::*;
        assert_eq!(achieved_level(&[]), None);
        assert_eq!(achieved_level(&[check(L0, Pass)]), Some(L0));
        // Nothing passed at L1: not reached, even with nothing failing.
        assert_eq!(
            achieved_level(&[check(L0, Pass), check(L1, NotApplicable)]),
            Some(L0)
        );
        // Untested blocks like a failure.
        assert_eq!(
            achieved_level(&[check(L0, Pass), check(L1, Pass), check(L1, Untested)]),
            Some(L0)
        );
        // A failure at L0 caps everything, whatever passes above it.
        assert_eq!(
            achieved_level(&[check(L0, Fail), check(L1, Pass), check(L2, Pass)]),
            None
        );
        assert_eq!(
            achieved_level(&[
                check(L0, Pass),
                check(L1, Pass),
                check(L2, Pass),
                check(L3, Untested)
            ]),
            Some(L2)
        );
    }
}
