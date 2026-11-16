//! `openmhs ops`: deployment intelligence over recorded events (RFC 0005). Works directly on a local store file;
//! never contacts a gateway, a robot or a device.

use std::io::Read;
use std::path::{Path, PathBuf};

use openmhs_events::format_ns;
use openmhs_ops::{CompareSpec, IncidentDetail, IngestStatus, OutcomeKind, Store};
use serde_json::Value;

use crate::exit;

fn fail(message: impl std::fmt::Display) -> u8 {
    eprintln!("error: {message}");
    exit::ERROR
}

fn open(store: &Path) -> Result<Store, String> {
    Store::open(store).map_err(|e| format!("{}: {e}", store.display()))
}

fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// JSON Lines, or one JSON array. `-` reads standard input.
fn read_records(path: &Path) -> Result<Vec<Value>, String> {
    let mut text = String::new();
    if path == Path::new("-") {
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| format!("stdin: {e}"))?;
    } else {
        text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let trimmed = text.trim_start();
    if trimmed.starts_with('[') {
        return serde_json::from_str(trimmed).map_err(|e| format!("{}: {e}", path.display()));
    }
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| {
            serde_json::from_str(l).map_err(|e| format!("{}:{}: {e}", path.display(), i + 1))
        })
        .collect()
}

pub fn ingest(json: bool, store: &Path, files: &[PathBuf], rebuild: bool) -> u8 {
    let run = || -> Result<u8, String> {
        let mut s = open(store)?;
        let mut totals = std::collections::BTreeMap::<String, u64>::new();
        let mut rejected = Vec::new();
        for file in files {
            let records = read_records(file)?;
            for r in s.ingest(&records, now_ns()).map_err(|e| e.to_string())? {
                let key = match &r.status {
                    IngestStatus::Accepted => "accepted",
                    IngestStatus::Duplicate => "duplicate",
                    IngestStatus::Conflict { .. } => "conflict",
                    IngestStatus::Rejected { .. } => "rejected",
                };
                *totals.entry(key.into()).or_default() += 1;
                if let IngestStatus::Rejected { code, message } = &r.status {
                    rejected.push(format!("{} {}: {code}: {message}", r.record, r.id));
                }
            }
        }
        let report = if rebuild {
            Some(s.rebuild().map_err(|e| e.to_string())?)
        } else {
            None
        };
        if json {
            println!(
                "{}",
                serde_json::json!({"ingested": totals, "rejected": rejected, "rebuild": report})
            );
        } else {
            let parts: Vec<String> = totals.iter().map(|(k, v)| format!("{v} {k}")).collect();
            println!("ingested: {}", parts.join(", "));
            for r in rejected.iter().take(20) {
                println!("  rejected {r}");
            }
            if let Some(r) = report {
                println!(
                    "rebuilt ({}): {} task attempts, {} incidents, {} rollouts",
                    r.rules_version, r.task_attempts, r.incidents, r.rollouts
                );
            }
        }
        Ok(if rejected.is_empty() {
            exit::OK
        } else {
            exit::FAILED
        })
    };
    run().unwrap_or_else(fail)
}

pub fn rebuild(json: bool, store: &Path) -> u8 {
    match open(store).and_then(|mut s| s.rebuild().map_err(|e| e.to_string())) {
        Ok(r) if json => {
            println!("{}", serde_json::to_string(&r).unwrap_or_default());
            exit::OK
        }
        Ok(r) => {
            println!(
                "rebuilt ({}): {} task attempts, {} incidents ({} members), {} rollouts, {} rollout conflicts",
                r.rules_version,
                r.task_attempts,
                r.incidents,
                r.members,
                r.rollouts,
                r.rollout_conflicts
            );
            exit::OK
        }
        Err(e) => fail(e),
    }
}

pub fn status(json: bool, store: &Path) -> u8 {
    match open(store).and_then(|s| s.counts().map_err(|e| e.to_string())) {
        Ok(c) if json => {
            let map: serde_json::Map<String, Value> =
                c.into_iter().map(|(k, v)| (k, v.into())).collect();
            println!("{}", Value::Object(map));
            exit::OK
        }
        Ok(c) => {
            for (k, v) in c {
                println!("{k:<18} {v}");
            }
            exit::OK
        }
        Err(e) => fail(e),
    }
}

fn short(digest: Option<&str>) -> String {
    digest.map_or_else(
        || "-".into(),
        |d| d.trim_start_matches("sha256:").chars().take(12).collect(),
    )
}

pub fn incidents(
    json: bool,
    store: &Path,
    robot: Option<&str>,
    task_type: Option<&str>,
    limit: usize,
) -> u8 {
    match open(store).and_then(|s| {
        s.incidents(robot, task_type, limit)
            .map_err(|e| e.to_string())
    }) {
        Ok(list) if json => {
            println!("{}", serde_json::to_string(&list).unwrap_or_default());
            exit::OK
        }
        Ok(list) => {
            if list.is_empty() {
                println!("no incidents");
            }
            for i in list {
                println!(
                    "{}  {}  {:<6} {:<14} reported {:<9} verified {:<9} {:<28} {}  release {}",
                    i.incident_id,
                    i.opened_ns.map(format_ns).unwrap_or_default(),
                    i.task_type,
                    i.task_id,
                    i.reported.as_deref().unwrap_or("-"),
                    i.verified.as_deref().unwrap_or("-"),
                    i.triggers.join(","),
                    i.robot,
                    i.release.as_deref().unwrap_or("-"),
                );
            }
            exit::OK
        }
        Err(e) => fail(e),
    }
}

fn print_detail(d: &IncidentDetail) {
    let s = &d.summary;
    println!(
        "{}  robot {}  site {}  task {} ({})  {}",
        s.incident_id,
        s.robot,
        s.site.as_deref().unwrap_or("-"),
        s.task_id,
        s.task_type,
        s.status
    );
    println!(
        "reported {}  verified {}  triggers {}",
        s.reported.as_deref().unwrap_or("-"),
        s.verified.as_deref().unwrap_or("none"),
        s.triggers.join(", ")
    );
    println!(
        "deployment {}  release {}",
        short(s.manifest.as_deref()),
        s.release.as_deref().unwrap_or("-")
    );
    match &d.since_last_success {
        Some(l) if l.changes.is_empty() => println!(
            "last success on this robot: {} with the same manifest (nothing in the deployment changed)",
            l.task_id
        ),
        Some(l) => {
            println!(
                "changed since the last success on this robot ({}):",
                l.task_id
            );
            for c in &l.changes {
                println!(
                    "  {}/{}: {} -> {}",
                    c.kind,
                    c.name,
                    c.before.as_deref().unwrap_or("(absent)"),
                    c.after.as_deref().unwrap_or("(absent)")
                );
            }
        }
        None => println!("no earlier success of this task type on this robot"),
    }
    println!(
        "timeline ({} events, rules {}):",
        d.members.len(),
        d.rules_version
    );
    for m in &d.members {
        let time = m
            .observed_ns
            .map(format_ns)
            .unwrap_or_else(|| "(no time)".into());
        let unc = m
            .uncertainty_ms
            .map(|u| format!(" ±{u} ms"))
            .unwrap_or_default();
        let concurrent = if m.concurrent_with.is_empty() {
            String::new()
        } else {
            format!(
                "  concurrent with {} other event(s)",
                m.concurrent_with.len()
            )
        };
        let mut what = Vec::new();
        if let Some(r) = &m.reported {
            what.push(format!("reported {r}"));
        }
        if let Some(v) = &m.verified {
            what.push(format!("verified {v}"));
        }
        if let Some(c) = &m.code {
            what.push(c.clone());
        }
        if let Some(g) = &m.goal_id {
            what.push(format!("goal {}", &g[..g.len().min(8)]));
        }
        println!(
            "  {time}{unc}  {:<20} {:<32} {} ({}){concurrent}",
            m.kind,
            what.join("  "),
            m.basis,
            m.detail
        );
    }
    for a in &d.artifacts {
        println!(
            "evidence {}  {}  window {} .. {}",
            a.uri,
            a.media_type.as_deref().unwrap_or(""),
            format_ns(a.window.0),
            format_ns(a.window.1)
        );
    }
    if !d.similar.is_empty() {
        println!(
            "same fingerprint ({}): {} other incidents",
            s.fingerprint,
            d.similar.len()
        );
    }
}

pub fn show(json: bool, store: &Path, id: &str) -> u8 {
    match open(store).and_then(|s| s.incident(id).map_err(|e| e.to_string())) {
        Ok(Some(d)) if json => {
            println!("{}", serde_json::to_string(&d).unwrap_or_default());
            exit::OK
        }
        Ok(Some(d)) => {
            print_detail(&d);
            exit::OK
        }
        Ok(None) => fail(format!("no incident {id}")),
        Err(e) => fail(e),
    }
}

fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

fn interval(ci: Option<(f64, f64)>) -> String {
    ci.map_or_else(|| "-".into(), |(l, h)| format!("{}-{}", pct(l), pct(h)))
}

#[allow(clippy::too_many_arguments)]
pub fn compare(
    json: bool,
    store: &Path,
    task_type: &str,
    a: &str,
    b: &str,
    verified: bool,
    no_strata: bool,
    per_attempt: bool,
    paired: bool,
    min_known: u64,
    margin: f64,
) -> u8 {
    let outcome = if verified {
        OutcomeKind::Verified
    } else {
        OutcomeKind::Reported
    };
    let mut spec = CompareSpec::new(task_type, a, b, outcome);
    spec.by_site = !no_strata;
    spec.by_scenario = !per_attempt;
    spec.paired = paired;
    spec.min_known = min_known;
    spec.margin = margin;
    let c = match open(store).and_then(|s| s.compare(&spec).map_err(|e| e.to_string())) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    if json {
        println!("{}", serde_json::to_string(&c).unwrap_or_default());
        return exit::OK;
    }
    println!(
        "{task_type}: {a} -> {b}   outcome: {}   (rules {})",
        if verified {
            "verified (RFC 0004 verdicts)"
        } else {
            "reported by the executing system"
        },
        c.rules_version
    );
    println!(
        "        counted as    n    ok  failed  unknown  failure rate (95% CI)        unknowns as ok / as failed"
    );
    for (name, arm) in [("a", &c.a), ("b", &c.b)] {
        println!(
            "{name} {:<6} {:<10} {:>4} {:>5} {:>7} {:>8}  {:<8} ({:<13})       {}",
            arm.selector,
            arm.unit,
            arm.succeeded + arm.failed + arm.unknown,
            arm.succeeded,
            arm.failed,
            arm.unknown,
            arm.rate.map(pct).unwrap_or_else(|| "-".into()),
            interval(arm.ci95),
            arm.unknown_bounds
                .map(|(l, h)| format!("{} / {}", pct(l), pct(h)))
                .unwrap_or_else(|| "-".into())
        );
    }
    if let Some((d, lo, hi)) = c.difference {
        println!(
            "difference b - a: {:+.1} points (95% CI {:+.1} to {:+.1}, {})",
            d * 100.0,
            lo * 100.0,
            hi * 100.0,
            c.difference_method
        );
    }
    if let Some(p) = &c.paired {
        println!(
            "paired by scenario: {} pairs; both fail {}, only a fails {}, only b fails {}, neither {}; \
             McNemar exact p = {}; left out: {} unknown, {} only in a, {} only in b",
            p.pairs,
            p.both_fail,
            p.a_only_fails,
            p.b_only_fails,
            p.both_ok,
            p.mcnemar_p
                .map_or_else(|| "n/a".into(), |x| format!("{x:.4}")),
            p.unknown_pairs,
            p.unmatched_a,
            p.unmatched_b
        );
    }
    if let Some((lo, hi)) = c.unknown_bounds {
        println!(
            "  with unknowns counted both ways: {:+.1} to {:+.1} points",
            lo * 100.0,
            hi * 100.0
        );
    }
    for (name, arm) in [("a", &c.a), ("b", &c.b)] {
        if let Some((median, p90, n)) = arm.duration_s {
            println!(
                "duration {name}: median {median:.1} s, 90th percentile {p90:.1} s over {n} attempts ({} attempts, {} scenarios)",
                arm.attempts, arm.scenarios
            );
        }
    }
    if c.strata.len() > 1 {
        for s in &c.strata {
            println!(
                "  site {}: a {}/{}  b {}/{}",
                s.site, s.a.0, s.a.1, s.b.0, s.b.1
            );
        }
    }
    println!("verdict: {:?}  ({})", c.verdict, c.reasons.join("; "));
    for (name, arm) in [("a", &c.a), ("b", &c.b)] {
        if let Some((median, p90, n)) = arm.wall_s {
            println!(
                "wall time {name}: median {median:.1} s, 90th percentile {p90:.1} s over {n} attempts, as reported by the \
                 producer (comparable only on the same hardware and load)"
            );
        }
    }
    for w in &c.warnings {
        println!("warning: {w}");
    }
    for (name, arm) in [("a", &c.a), ("b", &c.b)] {
        if !arm.incidents.is_empty() {
            // The query returns at most the newest five; say so, so a short list is not read as a complete one.
            let note = if arm.incidents.len() >= 5 {
                " (newest 5; `ops incidents` lists them all)"
            } else {
                ""
            };
            println!("incidents in {name}{note}: {}", arm.incidents.join(" "));
        }
    }
    exit::OK
}
