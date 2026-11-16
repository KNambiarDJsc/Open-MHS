//! Reads: incident lists and timelines, and release comparisons (ADR-0032). Every answer names what it is based on.

use std::collections::{BTreeMap, BTreeSet};

use openmhs_events::{ComponentChange, Manifest, diff};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;

use crate::derive::{RULES_VERSION, WINDOW_AFTER_NS, WINDOW_BEFORE_NS};
use crate::stats::{
    PairedCounts, StratumCounts, mantel_haenszel_rd, mcnemar_exact, newcombe, newcombe_paired,
    wilson,
};
use crate::{OpsError, Store};

#[derive(Debug, Clone, Serialize)]
pub struct IncidentSummary {
    pub incident_id: String,
    pub robot: String,
    pub site: Option<String>,
    pub task_id: String,
    pub task_type: String,
    pub opened_ns: Option<i64>,
    pub triggers: Vec<String>,
    pub fingerprint: String,
    pub reported: Option<String>,
    pub verified: Option<String>,
    pub manifest: Option<String>,
    pub release: Option<String>,
    pub members: i64,
    pub status: String,
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Member {
    pub event_id: String,
    pub kind: String,
    pub observed_ns: Option<i64>,
    pub uncertainty_ms: Option<f64>,
    pub clock_domain: String,
    pub basis: String,
    pub detail: String,
    pub reported: Option<String>,
    pub verified: Option<String>,
    pub goal_id: Option<String>,
    pub action_id: Option<String>,
    pub code: Option<String>,
    pub provenance: String,
    /// Events whose time, widened by both uncertainties, overlaps this one's: their order is not known.
    pub concurrent_with: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactLink {
    pub artifact_id: String,
    pub uri: String,
    pub media_type: Option<String>,
    pub start_ns: Option<i64>,
    pub end_ns: Option<i64>,
    /// The part of the artifact that covers the incident's window.
    pub window: (i64, i64),
}

#[derive(Debug, Clone, Serialize)]
pub struct LastSuccess {
    pub task_id: String,
    pub started_ns: Option<i64>,
    pub manifest: Option<String>,
    pub changes: Vec<ComponentChange>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IncidentDetail {
    pub summary: IncidentSummary,
    pub members: Vec<Member>,
    pub manifest: Option<Value>,
    /// The last attempt of the same task type on the same robot that reported success and was not verified
    /// violated, and what differs between its manifest and this incident's.
    pub since_last_success: Option<LastSuccess>,
    pub artifacts: Vec<ArtifactLink>,
    pub similar: Vec<String>,
    pub rules_version: &'static str,
}

fn summary_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<IncidentSummary> {
    let triggers: String = r.get(9)?;
    Ok(IncidentSummary {
        incident_id: r.get(0)?,
        robot: r.get(1)?,
        site: r.get(2)?,
        task_id: r.get(3)?,
        task_type: r.get(4)?,
        opened_ns: r.get(5)?,
        fingerprint: r.get(6)?,
        reported: r.get(7)?,
        verified: r.get(8)?,
        triggers: serde_json::from_str(&triggers).unwrap_or_default(),
        manifest: r.get(10)?,
        release: r.get(11)?,
        members: r.get(12)?,
        status: r
            .get::<_, Option<String>>(13)?
            .unwrap_or_else(|| "open".into()),
        owner: r.get(14)?,
    })
}

const SUMMARY_SQL: &str = "SELECT i.incident_id, i.robot, i.site, i.task_id, i.task_type, i.opened_ns, i.fingerprint,
        i.reported, i.verified, i.triggers, i.manifest, m.release,
        (SELECT COUNT(*) FROM incident_members im WHERE im.tenant = i.tenant AND im.incident_id = i.incident_id),
        s.status, s.owner
    FROM incidents i
    LEFT JOIN manifests m ON m.digest = i.manifest
    LEFT JOIN incident_state s ON s.tenant = i.tenant AND s.incident_id = i.incident_id";

impl Store {
    pub fn incidents(
        &self,
        robot: Option<&str>,
        task_type: Option<&str>,
        limit: usize,
    ) -> Result<Vec<IncidentSummary>, OpsError> {
        let sql = format!(
            "{SUMMARY_SQL} WHERE (?1 IS NULL OR i.robot = ?1) AND (?2 IS NULL OR i.task_type = ?2)
             ORDER BY i.opened_ns DESC, i.incident_id LIMIT ?3"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params![robot, task_type, limit as i64], summary_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn incident(&self, incident_id: &str) -> Result<Option<IncidentDetail>, OpsError> {
        let sql = format!("{SUMMARY_SQL} WHERE i.incident_id = ?1");
        let Some(summary) = self
            .conn
            .query_row(&sql, [incident_id], summary_row)
            .optional()?
        else {
            return Ok(None);
        };
        let tenant: String = self.conn.query_row(
            "SELECT tenant FROM incidents WHERE incident_id = ?1",
            [incident_id],
            |r| r.get(0),
        )?;

        let mut stmt = self.conn.prepare(
            "SELECT e.event_id, e.type, e.observed_ns, e.uncertainty_ms, e.clock_domain, m.basis, m.detail,
                    e.reported, e.verified, e.goal_id, e.action_id, e.code, e.provenance, e.seq
             FROM incident_members m JOIN events e ON e.tenant = m.tenant AND e.event_id = m.event_id
             WHERE m.tenant = ?1 AND m.incident_id = ?2
             ORDER BY e.observed_ns IS NULL, e.observed_ns, e.seq, e.event_id",
        )?;
        let members = stmt
            .query_map(params![tenant, incident_id], |r| {
                Ok(Member {
                    event_id: r.get(0)?,
                    kind: r.get(1)?,
                    observed_ns: r.get(2)?,
                    uncertainty_ms: r.get(3)?,
                    clock_domain: r.get(4)?,
                    basis: r.get(5)?,
                    detail: r.get(6)?,
                    reported: r.get(7)?,
                    verified: r.get(8)?,
                    goal_id: r.get(9)?,
                    action_id: r.get(10)?,
                    code: r.get(11)?,
                    provenance: r.get(12)?,
                    concurrent_with: Vec::new(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut members = members;
        mark_concurrent(&mut members);

        let manifest_body: Option<String> = match &summary.manifest {
            Some(d) => self
                .conn
                .query_row("SELECT body FROM manifests WHERE digest = ?1", [d], |r| {
                    r.get(0)
                })
                .optional()?,
            None => None,
        };
        let manifest = manifest_body
            .as_deref()
            .and_then(|b| serde_json::from_str(b).ok());

        let since_last_success = self.last_success(&tenant, &summary, manifest_body.as_deref())?;

        let mut artifacts = Vec::new();
        if let Some(open) = summary.opened_ns {
            let close: Option<i64> = self.conn.query_row(
                "SELECT closed_ns FROM incidents WHERE incident_id = ?1",
                [incident_id],
                |r| r.get(0),
            )?;
            let lo = open - WINDOW_BEFORE_NS;
            let hi = close.unwrap_or(open) + WINDOW_AFTER_NS;
            let mut stmt = self.conn.prepare(
                "SELECT artifact_id, uri, media_type, start_ns, end_ns FROM artifacts
                 WHERE tenant = ?1 AND (robot IS NULL OR robot = ?2)
                   AND (start_ns IS NULL OR start_ns <= ?4) AND (end_ns IS NULL OR end_ns >= ?3)
                 ORDER BY artifact_id",
            )?;
            artifacts = stmt
                .query_map(params![tenant, summary.robot, lo, hi], |r| {
                    let start: Option<i64> = r.get(3)?;
                    let end: Option<i64> = r.get(4)?;
                    Ok(ArtifactLink {
                        artifact_id: r.get(0)?,
                        uri: r.get(1)?,
                        media_type: r.get(2)?,
                        start_ns: start,
                        end_ns: end,
                        window: (
                            start.map_or(lo, |s| s.max(lo)),
                            end.map_or(hi, |e| e.min(hi)),
                        ),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
        }

        let mut stmt = self.conn.prepare(
            "SELECT incident_id FROM incidents WHERE tenant = ?1 AND fingerprint = ?2 AND incident_id <> ?3
             ORDER BY opened_ns DESC, incident_id LIMIT 20",
        )?;
        let similar = stmt
            .query_map(params![tenant, summary.fingerprint, incident_id], |r| {
                r.get(0)
            })?
            .collect::<Result<Vec<String>, _>>()?;

        Ok(Some(IncidentDetail {
            summary,
            members,
            manifest,
            since_last_success,
            artifacts,
            similar,
            rules_version: RULES_VERSION,
        }))
    }

    fn last_success(
        &self,
        tenant: &str,
        s: &IncidentSummary,
        manifest_body: Option<&str>,
    ) -> Result<Option<LastSuccess>, OpsError> {
        let Some(opened) = s.opened_ns else {
            return Ok(None);
        };
        let row: Option<(String, Option<i64>, Option<String>)> = self
            .conn
            .query_row(
                "SELECT task_id, started_ns, manifest FROM task_attempts
                 WHERE tenant = ?1 AND robot = ?2 AND task_type = ?3 AND started_ns < ?4
                   AND reported = 'succeeded' AND (verified IS NULL OR verified <> 'violated')
                 ORDER BY started_ns DESC LIMIT 1",
                params![tenant, s.robot, s.task_type, opened],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((task_id, started_ns, manifest)) = row else {
            return Ok(None);
        };
        let before: Option<String> = match &manifest {
            Some(d) => self
                .conn
                .query_row("SELECT body FROM manifests WHERE digest = ?1", [d], |r| {
                    r.get(0)
                })
                .optional()?,
            None => None,
        };
        let parse = |b: &str| {
            serde_json::from_str::<Value>(b)
                .ok()
                .and_then(|v| Manifest::parse(&v).ok())
        };
        let changes = match (
            before.as_deref().and_then(parse),
            manifest_body.and_then(parse),
        ) {
            (Some(b), Some(a)) => diff(&b, &a),
            _ => Vec::new(),
        };
        Ok(Some(LastSuccess {
            task_id,
            started_ns,
            manifest,
            changes,
        }))
    }
}

/// Two events are concurrent when their times, each widened by its own uncertainty, overlap and they share a clock
/// domain, or when they are on different clocks without stated uncertainties on both (their order cannot be known).
fn mark_concurrent(members: &mut [Member]) {
    let n = members.len();
    for i in 0..n {
        for j in 0..n {
            if i == j {
                continue;
            }
            let (a, b) = (&members[i], &members[j]);
            let (Some(ta), Some(tb)) = (a.observed_ns, b.observed_ns) else {
                continue;
            };
            let unc = |m: &Member| m.uncertainty_ms.map(|u| (u * 1e6) as i64);
            let overlap = match (a.clock_domain == b.clock_domain, unc(a), unc(b)) {
                (true, ua, ub) => (ta - tb).abs() <= ua.unwrap_or(0) + ub.unwrap_or(0),
                (false, Some(ua), Some(ub)) => (ta - tb).abs() <= ua + ub,
                (false, _, _) => true,
            };
            if overlap {
                let id = members[j].event_id.clone();
                members[i].concurrent_with.push(id);
            }
        }
    }
}

// ------------------------------------------------------------------------------------------ release comparison

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    /// What the executing system said.
    Reported,
    /// RFC 0004 verdicts. Attempts without one count as unknown.
    Verified,
}

#[derive(Debug, Clone)]
pub struct CompareSpec {
    pub task_type: String,
    /// A release label (`labels.release`), a manifest digest, or a digest prefix of at least 8 hex digits.
    pub a: String,
    pub b: String,
    pub outcome: OutcomeKind,
    /// Stratify by site (default); a stratum present in only one arm is reported, not pooled.
    pub by_site: bool,
    /// Smallest change in failure rate, as a fraction, that counts as a regression or an improvement.
    pub margin: f64,
    /// Minimum attempts with a known outcome per arm before any verdict other than `insufficient_evidence`.
    pub min_known: u64,
    /// Minimum failures across both arms.
    pub min_failures: u64,
    /// Count scenarios, not attempts, when attempts repeat a scenario (`subject.scenario_id`). Default on.
    pub by_scenario: bool,
    /// Match scenarios across the two arms and compare each scenario with itself (McNemar's exact test, Newcombe's
    /// paired interval). Needs `subject.scenario_id` on the attempts, and the same scenarios in both arms.
    pub paired: bool,
}

impl CompareSpec {
    pub fn new(task_type: &str, a: &str, b: &str, outcome: OutcomeKind) -> Self {
        Self {
            task_type: task_type.into(),
            a: a.into(),
            b: b.into(),
            outcome,
            by_site: true,
            margin: 0.01,
            min_known: 30,
            min_failures: 5,
            by_scenario: true,
            paired: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Arm {
    pub selector: String,
    pub manifests: Vec<String>,
    /// Task attempts in this arm, and their outcomes counted per attempt: (succeeded, failed, unknown).
    pub attempts: u64,
    pub attempt_outcomes: (u64, u64, u64),
    /// Distinct scenario ids among the attempts (0 when none carry one).
    pub scenarios: u64,
    /// What `succeeded`, `failed` and `unknown` count: `attempt`, or `scenario` when attempts repeat scenarios
    /// (each scenario judged by the majority of its attempts; a tie is unknown).
    pub unit: &'static str,
    pub succeeded: u64,
    pub failed: u64,
    pub unknown: u64,
    pub excluded: BTreeMap<String, u64>,
    /// failed / (succeeded + failed).
    pub rate: Option<f64>,
    pub ci95: Option<(f64, f64)>,
    /// The rate with every unknown counted as a success, then as a failure.
    pub unknown_bounds: Option<(f64, f64)>,
    pub sites: BTreeMap<String, u64>,
    pub incidents: Vec<String>,
    /// Seconds from task start to task end over attempts with both: (median, 90th percentile, n).
    pub duration_s: Option<(f64, f64, u64)>,
}

/// One attempt as read for a comparison.
struct Row {
    site: String,
    class: &'static str,
    unit_key: String,
    scenario: Option<String>,
    duration_ns: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stratum {
    pub site: String,
    pub a: (u64, u64),
    pub b: (u64, u64),
    pub difference: Option<(f64, f64, f64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// b's failure rate is higher than a's by more than the margin, with 95 % confidence.
    Regression,
    /// b's failure rate is lower by more than the margin.
    Improvement,
    /// The interval lies within ± margin.
    NoChange,
    InsufficientEvidence,
}

/// Scenarios present in both arms, compared with themselves.
#[derive(Debug, Clone, Serialize)]
pub struct Paired {
    /// Scenarios with a known outcome in both arms (the 2 x 2 table below sums to this).
    pub pairs: u64,
    pub both_fail: u64,
    pub a_only_fails: u64,
    pub b_only_fails: u64,
    pub both_ok: u64,
    /// Scenarios in both arms where either arm has no known outcome.
    pub unknown_pairs: u64,
    pub unmatched_a: u64,
    pub unmatched_b: u64,
    /// b - a in failure rate, Newcombe's paired interval: (estimate, low, high).
    pub difference: Option<(f64, f64, f64)>,
    /// Exact two-sided McNemar p-value over the scenarios that differ; none when no scenario differs.
    pub mcnemar_p: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Comparison {
    pub task_type: String,
    pub outcome: OutcomeKind,
    pub a: Arm,
    pub b: Arm,
    /// b − a in failure rate: (estimate, low, high).
    pub difference: Option<(f64, f64, f64)>,
    pub difference_method: &'static str,
    /// The difference with unknowns counted against b and for a, then the reverse.
    pub unknown_bounds: Option<(f64, f64)>,
    pub strata: Vec<Stratum>,
    pub paired: Option<Paired>,
    pub verdict: Verdict,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
    pub rules_version: &'static str,
}

/// One scenario's outcome in one arm: the majority of its attempts, a tie being unknown. Attempts without a scenario id
/// cannot be matched across arms and are left out.
fn scenario_outcomes(rows: &[Row]) -> BTreeMap<&str, &'static str> {
    let mut counts: BTreeMap<&str, (u64, u64, u64)> = BTreeMap::new();
    for r in rows {
        let Some(sc) = r.scenario.as_deref() else {
            continue;
        };
        let e = counts.entry(sc).or_default();
        match r.class {
            "success" => e.0 += 1,
            "failure" => e.1 += 1,
            "unknown" => e.2 += 1,
            _ => {}
        }
    }
    counts
        .into_iter()
        .map(|(k, (ok, failed, unknown))| {
            let class = if failed > ok + unknown {
                "failure"
            } else if ok > failed + unknown {
                "success"
            } else {
                "unknown"
            };
            (k, class)
        })
        .collect()
}

fn paired_comparison(rows_a: &[Row], rows_b: &[Row]) -> Paired {
    let (a, b) = (scenario_outcomes(rows_a), scenario_outcomes(rows_b));
    let mut p = Paired {
        pairs: 0,
        both_fail: 0,
        a_only_fails: 0,
        b_only_fails: 0,
        both_ok: 0,
        unknown_pairs: 0,
        unmatched_a: 0,
        unmatched_b: 0,
        difference: None,
        mcnemar_p: None,
    };
    for (k, oa) in &a {
        match b.get(k) {
            None => p.unmatched_a += 1,
            Some(&ob) if *oa == "unknown" || ob == "unknown" => p.unknown_pairs += 1,
            Some(&ob) => {
                p.pairs += 1;
                match (*oa == "failure", ob == "failure") {
                    (true, true) => p.both_fail += 1,
                    (true, false) => p.a_only_fails += 1,
                    (false, true) => p.b_only_fails += 1,
                    (false, false) => p.both_ok += 1,
                }
            }
        }
    }
    p.unmatched_b = b.keys().filter(|k| !a.contains_key(*k)).count() as u64;
    let counts = PairedCounts {
        both: p.both_fail,
        only_a: p.a_only_fails,
        only_b: p.b_only_fails,
        neither: p.both_ok,
    };
    p.difference = newcombe_paired(counts);
    p.mcnemar_p = mcnemar_exact(p.a_only_fails, p.b_only_fails);
    p
}

/// Per site: (failures, attempts with a known outcome).
type SiteCounts = BTreeMap<String, (u64, u64)>;

impl Store {
    fn resolve(&self, selector: &str) -> Result<Vec<String>, OpsError> {
        let mut stmt = self
            .conn
            .prepare("SELECT digest, release FROM manifests ORDER BY digest")?;
        let rows: Vec<(String, Option<String>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        let hex = selector.strip_prefix("sha256:").unwrap_or(selector);
        let by_release: Vec<String> = rows
            .iter()
            .filter(|(_, rel)| rel.as_deref() == Some(selector))
            .map(|(d, _)| d.clone())
            .collect();
        if !by_release.is_empty() {
            return Ok(by_release);
        }
        if hex.len() >= 8 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            let want = format!("sha256:{}", hex.to_ascii_lowercase());
            return Ok(rows
                .into_iter()
                .filter(|(d, _)| d.starts_with(&want))
                .map(|(d, _)| d)
                .collect());
        }
        Ok(Vec::new())
    }

    fn rows(&self, spec: &CompareSpec, selector: &str) -> Result<(Arm, Vec<Row>), OpsError> {
        let manifests = self.resolve(selector)?;
        let mut arm = Arm {
            selector: selector.into(),
            manifests: manifests.clone(),
            unit: "attempt",
            ..Default::default()
        };
        let mut rows = Vec::new();
        for digest in &manifests {
            let mut stmt = self.conn.prepare(
                "SELECT site, reported, verified, scenario_id, task_id, started_ns, ended_ns FROM task_attempts
                 WHERE task_type = ?1 AND manifest = ?2 ORDER BY task_id",
            )?;
            type Raw = (
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
                String,
                Option<i64>,
                Option<i64>,
            );
            let raw: Vec<Raw> = stmt
                .query_map(params![spec.task_type, digest], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                })?
                .collect::<Result<_, _>>()?;
            for (site, reported, verified, scenario, task_id, started, ended) in raw {
                let class = match spec.outcome {
                    OutcomeKind::Reported => match reported.as_deref() {
                        Some("succeeded") => "success",
                        Some("failed" | "aborted" | "timeout") => "failure",
                        Some("canceled") => "excluded:canceled",
                        _ => "unknown",
                    },
                    OutcomeKind::Verified => match verified.as_deref() {
                        Some("satisfied") => "success",
                        Some("violated") => "failure",
                        _ => "unknown",
                    },
                };
                rows.push(Row {
                    site: site.unwrap_or_else(|| "(no site)".into()),
                    class,
                    unit_key: scenario
                        .clone()
                        .unwrap_or_else(|| format!("task:{task_id}")),
                    scenario,
                    duration_ns: match (started, ended) {
                        (Some(s), Some(e)) if e >= s => Some(e - s),
                        _ => None,
                    },
                });
            }
            let mut stmt = self.conn.prepare(
                "SELECT incident_id FROM incidents WHERE task_type = ?1 AND manifest = ?2
                 ORDER BY opened_ns DESC, incident_id LIMIT 5",
            )?;
            arm.incidents.extend(
                stmt.query_map(params![spec.task_type, digest], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        Ok((arm, rows))
    }

    /// Count an arm per attempt, or per scenario (majority of its attempts; a tie is unknown).
    fn tally(arm: &mut Arm, rows: &[Row], by_scenario: bool) -> SiteCounts {
        let mut by_site: SiteCounts = BTreeMap::new();
        let mut durations: Vec<i64> = Vec::new();
        // unit key -> (site, successes, failures, unknowns)
        let mut units: BTreeMap<&str, (String, u64, u64, u64)> = BTreeMap::new();
        let mut scenarios: BTreeSet<&str> = BTreeSet::new();
        for r in rows {
            arm.attempts += 1;
            *arm.sites.entry(r.site.clone()).or_default() += 1;
            if let Some(d) = r.duration_ns {
                durations.push(d);
            }
            if let Some(sc) = &r.scenario {
                scenarios.insert(sc);
            }
            match r.class {
                "success" => arm.attempt_outcomes.0 += 1,
                "failure" => arm.attempt_outcomes.1 += 1,
                "unknown" => arm.attempt_outcomes.2 += 1,
                other => {
                    *arm.excluded
                        .entry(other.trim_start_matches("excluded:").to_owned())
                        .or_default() += 1;
                    continue;
                }
            }
            if by_scenario {
                let unit = units
                    .entry(r.unit_key.as_str())
                    .or_insert_with(|| (r.site.clone(), 0, 0, 0));
                match r.class {
                    "success" => unit.1 += 1,
                    "failure" => unit.2 += 1,
                    _ => unit.3 += 1,
                }
            } else {
                let e = by_site.entry(r.site.clone()).or_default();
                match r.class {
                    "success" => {
                        arm.succeeded += 1;
                        e.1 += 1;
                    }
                    "failure" => {
                        arm.failed += 1;
                        e.0 += 1;
                        e.1 += 1;
                    }
                    _ => arm.unknown += 1,
                }
            }
        }
        arm.scenarios = scenarios.len() as u64;
        if by_scenario {
            arm.unit = "scenario";
            for (site, ok, failed, unknown) in units.into_values() {
                let e = by_site.entry(site).or_default();
                if failed > ok + unknown {
                    arm.failed += 1;
                    e.0 += 1;
                    e.1 += 1;
                } else if ok > failed + unknown {
                    arm.succeeded += 1;
                    e.1 += 1;
                } else {
                    arm.unknown += 1;
                }
            }
        }
        let known = arm.succeeded + arm.failed;
        if known > 0 {
            arm.rate = Some(arm.failed as f64 / known as f64);
            arm.ci95 = wilson(arm.failed, known);
        }
        let all = known + arm.unknown;
        if all > 0 {
            arm.unknown_bounds = Some((
                arm.failed as f64 / all as f64,
                (arm.failed + arm.unknown) as f64 / all as f64,
            ));
        }
        if !durations.is_empty() {
            durations.sort_unstable();
            let at = |q: f64| {
                let i = ((durations.len() - 1) as f64 * q).round() as usize;
                durations[i] as f64 / 1e9
            };
            arm.duration_s = Some((at(0.5), at(0.9), durations.len() as u64));
        }
        by_site
    }

    pub fn compare(&self, spec: &CompareSpec) -> Result<Comparison, OpsError> {
        let (mut a, rows_a) = self.rows(spec, &spec.a)?;
        let (mut b, rows_b) = self.rows(spec, &spec.b)?;
        let mut warnings = Vec::new();
        let mut reasons = Vec::new();
        // Replicates of one scenario are not independent trials. Count scenarios in both arms if either repeats one.
        let repeats = |rows: &[Row]| {
            let keys: BTreeSet<&str> = rows.iter().map(|r| r.unit_key.as_str()).collect();
            rows.iter().any(|r| r.scenario.is_some()) && keys.len() < rows.len()
        };
        let by_scenario = spec.by_scenario && (repeats(&rows_a) || repeats(&rows_b));
        let sites_a = Self::tally(&mut a, &rows_a, by_scenario);
        let sites_b = Self::tally(&mut b, &rows_b, by_scenario);
        if by_scenario {
            warnings.push(format!(
                "attempts repeat scenarios: counted per scenario ({} in a from {} attempts, {} in b from {} attempts), each judged by the majority of its attempts",
                a.succeeded + a.failed + a.unknown,
                a.attempts,
                b.succeeded + b.failed + b.unknown,
                b.attempts
            ));
        }
        for arm in [&a, &b] {
            if arm.manifests.is_empty() {
                warnings.push(format!(
                    "{:?} matches no manifest (a release label, or a digest prefix)",
                    arm.selector
                ));
            }
            if arm.attempts > 0 && arm.unknown * 2 > arm.attempts {
                warnings.push(format!(
                    "{}: {} of {} attempts have no known {:?} outcome",
                    arm.selector, arm.unknown, arm.attempts, spec.outcome
                ));
            }
        }
        let overlap: BTreeSet<&String> = a
            .manifests
            .iter()
            .filter(|m| b.manifests.contains(m))
            .collect();
        if !overlap.is_empty() {
            warnings.push("the two selectors share manifests: the arms are not separate".into());
        }
        let no_manifest: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM task_attempts WHERE task_type = ?1 AND manifest IS NULL",
            [&spec.task_type],
            |r| r.get(0),
        )?;
        if no_manifest > 0 {
            warnings.push(format!(
                "{no_manifest} attempts of this task type have no known manifest and are left out"
            ));
        }

        let (ka, na) = (a.failed, a.failed + a.succeeded);
        let (kb, nb) = (b.failed, b.failed + b.succeeded);
        let plain = newcombe(ka, na, kb, nb);

        let mut strata = Vec::new();
        let mut counts = Vec::new();
        let sites: BTreeSet<&String> = sites_a.keys().chain(sites_b.keys()).collect();
        for site in &sites {
            let sa = sites_a.get(*site).copied().unwrap_or_default();
            let sb = sites_b.get(*site).copied().unwrap_or_default();
            if sa.1 == 0 || sb.1 == 0 {
                warnings.push(format!(
                    "site {site} appears only in {}: not comparable within the site",
                    if sa.1 == 0 { "b" } else { "a" }
                ));
            } else {
                counts.push(StratumCounts {
                    k_a: sa.0,
                    n_a: sa.1,
                    k_b: sb.0,
                    n_b: sb.1,
                });
            }
            strata.push(Stratum {
                site: (*site).clone(),
                a: sa,
                b: sb,
                difference: newcombe(sa.0, sa.1, sb.0, sb.1),
            });
        }
        let stratified = spec.by_site && sites.len() > 1;
        let (difference, method) = if stratified {
            (mantel_haenszel_rd(&counts), "mantel_haenszel_by_site")
        } else {
            (plain, "newcombe")
        };
        let paired = spec.paired.then(|| paired_comparison(&rows_a, &rows_b));
        let (difference, method) = match &paired {
            Some(p) => (p.difference, "newcombe_paired"),
            None => (difference, method),
        };
        if let Some(p) = &paired {
            if p.unmatched_a + p.unmatched_b > 0 {
                warnings.push(format!(
                    "{} scenarios appear only in a and {} only in b: they are left out of the paired comparison",
                    p.unmatched_a, p.unmatched_b
                ));
            }
            if p.unknown_pairs > 0 {
                warnings.push(format!(
                    "{} scenarios have no known outcome in one of the arms and are left out of the pairs",
                    p.unknown_pairs
                ));
            }
            if stratified {
                warnings.push("a paired comparison is not stratified by site".into());
            }
        }
        let (n_a_eff, n_b_eff, failures_eff) = match &paired {
            Some(p) => (
                p.pairs,
                p.pairs,
                2 * p.both_fail + p.a_only_fails + p.b_only_fails,
            ),
            None => (na, nb, ka + kb),
        };
        if stratified
            && let (Some(p), Some(s)) = (plain, difference)
            && (p.0 - s.0).abs() > spec.margin
        {
            warnings.push(format!(
                "pooled and site-stratified differences disagree ({:+.3} vs {:+.3}): the site mix differs between arms",
                p.0, s.0
            ));
        }

        let unknown_bounds = {
            let (all_a, all_b) = (na + a.unknown, nb + b.unknown);
            (all_a > 0 && all_b > 0).then(|| {
                let lo = kb as f64 / all_b as f64 - (ka + a.unknown) as f64 / all_a as f64;
                let hi = (kb + b.unknown) as f64 / all_b as f64 - ka as f64 / all_a as f64;
                (lo, hi)
            })
        };

        let verdict = if n_a_eff < spec.min_known || n_b_eff < spec.min_known {
            reasons.push(match &paired {
                Some(p) => format!(
                    "fewer than {} matched scenario pairs with a known outcome ({})",
                    spec.min_known, p.pairs
                ),
                None => format!(
                    "fewer than {} {}s with a known outcome in an arm (a {na}, b {nb})",
                    spec.min_known, a.unit
                ),
            });
            Verdict::InsufficientEvidence
        } else if failures_eff < spec.min_failures {
            reasons.push(format!(
                "fewer than {} failures in both arms together ({failures_eff})",
                spec.min_failures
            ));
            Verdict::InsufficientEvidence
        } else {
            match difference {
                None => {
                    reasons.push("no stratum has both arms".into());
                    Verdict::InsufficientEvidence
                }
                Some((_, lo, _)) if lo > spec.margin => {
                    reasons.push(format!(
                        "the whole 95 % interval is above +{:.3}",
                        spec.margin
                    ));
                    Verdict::Regression
                }
                Some((_, _, hi)) if hi < -spec.margin => {
                    reasons.push(format!(
                        "the whole 95 % interval is below -{:.3}",
                        spec.margin
                    ));
                    Verdict::Improvement
                }
                Some((_, lo, hi)) if lo >= -spec.margin && hi <= spec.margin => {
                    reasons.push(format!(
                        "the whole 95 % interval is within ±{:.3}",
                        spec.margin
                    ));
                    Verdict::NoChange
                }
                Some(_) => {
                    reasons.push(format!(
                        "the 95 % interval spans both no change and ±{:.3}",
                        spec.margin
                    ));
                    Verdict::InsufficientEvidence
                }
            }
        };

        Ok(Comparison {
            task_type: spec.task_type.clone(),
            outcome: spec.outcome,
            a,
            b,
            difference,
            difference_method: method,
            unknown_bounds,
            strata,
            paired,
            verdict,
            reasons,
            warnings,
            rules_version: RULES_VERSION,
        })
    }
}
