//! Plain-text rendering of a report, for terminals and test failure messages.

use std::fmt::Write;

use openmhs_types::descriptor::ConformanceLevel;

use crate::report::{Report, Status};

/// A short human-readable summary: the verdict, counts per level, and every check that
/// keeps the subject from its target level, with the reason.
pub fn render(report: &Report) -> String {
    let mut out = String::new();
    let s = &report.subject;
    let who = s.device.as_deref().unwrap_or("(no device)");
    let version = s
        .adapter_version
        .as_deref()
        .map(|v| format!(" {v}"))
        .unwrap_or_default();
    let level =
        |l: Option<ConformanceLevel>| l.map_or_else(|| "none".to_owned(), |l| l.to_string());
    let _ = writeln!(
        out,
        "{who} ({}{version}): reached {}, declared {} -> {}",
        s.adapter,
        level(report.achieved),
        level(report.declared),
        if report.passed { "PASS" } else { "FAIL" }
    );
    let faults: Vec<&str> = report.fixture.faults.iter().map(|f| f.name()).collect();
    let _ = writeln!(
        out,
        "  fixture faults: {}; counts deliveries: {}",
        if faults.is_empty() {
            "none".to_owned()
        } else {
            faults.join(", ")
        },
        if report.fixture.counts_deliveries {
            "yes"
        } else {
            "no"
        }
    );
    let m = &report.summary;
    let _ = writeln!(
        out,
        "  {} pass, {} fail, {} untested, {} not applicable, in {} ms",
        m.pass, m.fail, m.untested, m.not_applicable, report.duration_ms
    );
    for l in ConformanceLevel::ALL {
        let at: Vec<_> = report.checks.iter().filter(|c| c.level == l).collect();
        let count = |st: Status| at.iter().filter(|c| c.status == st).count();
        let _ = writeln!(
            out,
            "  {l} {:<13} {:>3} pass {:>3} fail {:>3} untested {:>3} n/a",
            l.name(),
            count(Status::Pass),
            count(Status::Fail),
            count(Status::Untested),
            count(Status::NotApplicable)
        );
    }
    let target = report.target();
    let blockers = report.blockers(target);
    if !blockers.is_empty() {
        let _ = writeln!(out, "  keeping it from {target}:");
        for c in blockers {
            let cap = c
                .capability
                .as_deref()
                .map(|c| format!(" [{c}]"))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "    {} {}{cap}: {}",
                c.status.name().to_uppercase(),
                c.id,
                c.message
            );
        }
    }
    out
}
