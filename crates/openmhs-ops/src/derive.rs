//! Derived tables: rollouts, task attempts and incidents (correlation rules, ADR-0030).
//!
//! Every derived row is rebuilt from the events alone, so the same events give the same rows whatever order they
//! arrived in. Membership in an incident always says why: `id_match` (same task or goal id) or `window` (same robot,
//! within a stated time window after clock uncertainty). Nothing here asserts a cause.

use std::collections::{BTreeMap, BTreeSet};

use openmhs_events::sha256_digest;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::Serialize;

use crate::{OpsError, Store};

/// Version of the correlation rules. Every derived row carries it.
pub const RULES_VERSION: &str = "corr-1";
/// Window joins: from this long before a task starts to this long after it ends (rules corr-1).
pub const WINDOW_BEFORE_NS: i64 = 30_000_000_000;
pub const WINDOW_AFTER_NS: i64 = 10_000_000_000;

const FAILED_REPORTS: &[&str] = &["failed", "aborted", "timeout"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RebuildReport {
    pub rules_version: &'static str,
    pub rollouts: usize,
    /// Two `deployment.applied` events for one robot at the same instant with different manifests.
    pub rollout_conflicts: usize,
    pub task_attempts: usize,
    pub incidents: usize,
    pub members: usize,
}

#[derive(Default)]
struct TaskAgg {
    started: Option<(i64, Option<String>)>,
    ended: Option<(i64, Option<String>)>,
    first_ns: Option<i64>,
    last_ns: Option<i64>,
    task_type: Option<String>,
    site: Option<String>,
    any_manifest: Option<String>,
    goals: BTreeSet<String>,
    verdicts: Vec<String>,
    interventions: usize,
    member_ids: Vec<String>,
    clock_domain: Option<String>,
    component: Option<String>,
    scenario: Option<String>,
}

struct Attempt {
    tenant: String,
    robot: String,
    task_id: String,
    task_type: String,
    site: Option<String>,
    manifest: Option<String>,
    manifest_basis: Option<&'static str>,
    started_ns: Option<i64>,
    ended_ns: Option<i64>,
    reported: String,
    verified: Option<String>,
    interventions: usize,
    close_reason: Option<&'static str>,
    goals: BTreeSet<String>,
    member_ids: Vec<String>,
    clock_domain: Option<String>,
    component: Option<String>,
    scenario_id: Option<String>,
    wall_s: Option<f64>,
}

fn aggregate_verdicts(verdicts: &[String]) -> Option<String> {
    if verdicts.is_empty() {
        None
    } else if verdicts.iter().any(|v| v == "violated") {
        Some("violated".into())
    } else if verdicts.iter().any(|v| v == "unknown") {
        Some("unknown".into())
    } else {
        Some("satisfied".into())
    }
}

fn min_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, None) | (None, x) => x,
    }
}

fn max_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) | (None, x) => x,
    }
}

impl Store {
    /// Rebuild every derived table from the stored events. Human state (owners, statuses, annotations) is keyed by
    /// incident id, which is stable across rebuilds, and is never touched here.
    pub fn rebuild(&mut self) -> Result<RebuildReport, OpsError> {
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "DELETE FROM rollouts; DELETE FROM task_attempts; DELETE FROM incidents; DELETE FROM incident_members;",
        )?;
        let mut report = RebuildReport {
            rules_version: RULES_VERSION,
            ..Default::default()
        };
        rollouts(&tx, &mut report)?;
        let attempts = task_attempts(&tx)?;
        report.task_attempts = attempts.len();
        for a in &attempts {
            tx.execute(
                "INSERT INTO task_attempts (tenant, robot, task_id, task_type, site, manifest, manifest_basis,
                     started_ns, ended_ns, reported, verified, interventions, close_reason, rules_version,
                     scenario_id, wall_s)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    a.tenant, a.robot, a.task_id, a.task_type, a.site, a.manifest, a.manifest_basis, a.started_ns,
                    a.ended_ns, a.reported, a.verified, a.interventions as i64, a.close_reason, RULES_VERSION,
                    a.scenario_id, a.wall_s
                ],
            )?;
            incident(&tx, a, &mut report)?;
        }
        tx.commit()?;
        Ok(report)
    }
}

fn rollouts(tx: &Transaction<'_>, report: &mut RebuildReport) -> Result<(), OpsError> {
    let mut stmt = tx.prepare(
        "SELECT tenant, robot, observed_ns, manifest, event_id FROM events
         WHERE type = 'deployment.applied' AND manifest IS NOT NULL AND observed_ns IS NOT NULL
         ORDER BY tenant, robot, observed_ns, event_id",
    )?;
    let rows: Vec<(String, String, i64, String, String)> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<Result<_, _>>()?;
    let mut kept: Vec<(String, String, i64, String, String)> = Vec::new();
    for row in rows {
        if let Some(last) = kept.last()
            && last.0 == row.0
            && last.1 == row.1
            && last.2 == row.2
        {
            if last.3 != row.3 {
                report.rollout_conflicts += 1;
            }
            continue;
        }
        kept.push(row);
    }
    for (i, (tenant, robot, from_ns, manifest, event_id)) in kept.iter().enumerate() {
        let to_ns = kept
            .get(i + 1)
            .filter(|n| &n.0 == tenant && &n.1 == robot)
            .map(|n| n.2);
        tx.execute(
            "INSERT INTO rollouts (tenant, robot, from_ns, to_ns, manifest, source_event) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![tenant, robot, from_ns, to_ns, manifest, event_id],
        )?;
        report.rollouts += 1;
    }
    Ok(())
}

/// (event id, observed, uncertainty ms, verified, goal id, component) of a window-join candidate.
type WindowRow = (
    String,
    i64,
    Option<f64>,
    Option<String>,
    Option<String>,
    Option<String>,
);

type EventRow = (
    String,
    String,
    String,
    String,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
);

fn task_attempts(tx: &Transaction<'_>) -> Result<Vec<Attempt>, OpsError> {
    let mut aggs: BTreeMap<(String, String, String), TaskAgg> = BTreeMap::new();
    let mut stmt = tx.prepare(
        "SELECT tenant, robot, task_id, type, observed_ns, task_type, site, manifest, reported, verified, goal_id,
                event_id, clock_domain, component, scenario_id
         FROM events WHERE task_id IS NOT NULL ORDER BY tenant, robot, task_id, observed_ns, event_id",
    )?;
    let rows: Vec<EventRow> = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
                r.get(10)?,
                r.get(11)?,
                r.get(12)?,
                r.get(13)?,
                r.get(14)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    for (
        tenant,
        robot,
        task_id,
        kind,
        observed,
        task_type,
        site,
        manifest,
        reported,
        verified,
        goal,
        event_id,
        domain,
        component,
        scenario,
    ) in rows
    {
        let agg = aggs.entry((tenant, robot, task_id)).or_default();
        if agg.scenario.is_none() {
            agg.scenario = scenario;
        }
        if kind == "task.started" || agg.clock_domain.is_none() {
            agg.clock_domain = Some(domain);
        }
        if kind == "task.started" && component.is_some() {
            agg.component = component;
        }
        agg.first_ns = min_opt(agg.first_ns, observed);
        agg.last_ns = max_opt(agg.last_ns, observed);
        if agg.task_type.is_none() {
            agg.task_type = task_type;
        }
        if agg.site.is_none() {
            agg.site = site;
        }
        if agg.any_manifest.is_none() {
            agg.any_manifest.clone_from(&manifest);
        }
        if let Some(g) = goal {
            agg.goals.insert(g);
        }
        match kind.as_str() {
            "task.started" if agg.started.is_none() => {
                agg.started = observed.map(|t| (t, manifest))
            }
            "task.ended" if agg.ended.is_none() => {
                agg.ended = Some((observed.unwrap_or(i64::MAX), reported))
            }
            "intervention.started" => agg.interventions += 1,
            _ => {}
        }
        if let Some(v) = verified {
            agg.verdicts.push(v);
        }
        agg.member_ids.push(event_id);
    }

    // Verdicts that name only a goal join the task that owns the goal (id_match on goal_id).
    let mut goal_verdicts: BTreeMap<(String, String, String), Vec<(String, String)>> =
        BTreeMap::new();
    let mut stmt = tx.prepare(
        "SELECT tenant, robot, goal_id, verified, event_id FROM events
         WHERE task_id IS NULL AND goal_id IS NOT NULL ORDER BY event_id",
    )?;
    let rows: Vec<(String, String, String, Option<String>, String)> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<Result<_, _>>()?;
    for (tenant, robot, goal, verified, event_id) in rows {
        goal_verdicts
            .entry((tenant, robot, goal))
            .or_default()
            .push((verified.unwrap_or_default(), event_id));
    }

    // The wall-clock cost of an attempt is the largest `cost.wall_s` any of its events reports (the producer may put it
    // on the end event, the episode event, or both).
    let mut wall: BTreeMap<(String, String, String), f64> = BTreeMap::new();
    let mut stmt = tx.prepare(
        "SELECT tenant, robot, task_id, json_extract(body, '$.cost.wall_s') FROM events
         WHERE task_id IS NOT NULL AND json_extract(body, '$.cost.wall_s') IS NOT NULL",
    )?;
    let costs: Vec<(String, String, String, f64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    for (tenant, robot, task_id, w) in costs {
        let slot = wall.entry((tenant, robot, task_id)).or_insert(w);
        *slot = slot.max(w);
    }

    let mut out = Vec::with_capacity(aggs.len());
    for ((tenant, robot, task_id), mut agg) in aggs {
        for goal in &agg.goals {
            if let Some(items) = goal_verdicts.get(&(tenant.clone(), robot.clone(), goal.clone())) {
                for (v, _) in items {
                    if !v.is_empty() {
                        agg.verdicts.push(v.clone());
                    }
                }
            }
        }
        let started_ns = agg.started.as_ref().map(|s| s.0).or(agg.first_ns);
        let (reported, close_reason) = match &agg.ended {
            Some((_, Some(r))) => (r.clone(), None),
            Some((_, None)) => ("unknown".to_owned(), Some("no_outcome")),
            None => ("unknown".to_owned(), Some("no_end")),
        };
        let ended_ns = agg
            .ended
            .as_ref()
            .map(|e| e.0)
            .filter(|t| *t != i64::MAX)
            .or(agg.last_ns);
        let (mut manifest, mut manifest_basis) =
            match agg.started.as_ref().and_then(|s| s.1.clone()) {
                Some(m) => (Some(m), Some("event")),
                None => match agg.any_manifest.clone() {
                    Some(m) => (Some(m), Some("event")),
                    None => (None, None),
                },
            };
        if manifest.is_none()
            && let Some(t) = started_ns
        {
            manifest = tx
                .query_row(
                    "SELECT manifest FROM rollouts WHERE tenant = ?1 AND robot = ?2 AND from_ns <= ?3
                     AND (to_ns IS NULL OR to_ns > ?3) ORDER BY from_ns DESC LIMIT 1",
                    params![tenant, robot, t],
                    |r| r.get(0),
                )
                .optional()?;
            if manifest.is_some() {
                manifest_basis = Some("rollout_interval");
            }
        }
        out.push(Attempt {
            task_type: agg.task_type.clone().unwrap_or_else(|| "untyped".into()),
            site: agg.site.clone(),
            manifest,
            manifest_basis,
            started_ns,
            ended_ns,
            reported,
            verified: aggregate_verdicts(&agg.verdicts),
            interventions: agg.interventions,
            close_reason,
            goals: agg.goals,
            member_ids: agg.member_ids,
            clock_domain: agg.clock_domain,
            component: agg.component,
            scenario_id: agg.scenario,
            wall_s: wall
                .get(&(tenant.clone(), robot.clone(), task_id.clone()))
                .copied(),
            tenant,
            robot,
            task_id,
        });
    }
    Ok(out)
}

fn incident(tx: &Transaction<'_>, a: &Attempt, report: &mut RebuildReport) -> Result<(), OpsError> {
    let mut triggers: Vec<String> = Vec::new();
    if FAILED_REPORTS.contains(&a.reported.as_str()) {
        triggers.push(format!("reported_{}", a.reported));
    }
    if a.reported == "unknown" {
        triggers.push("reported_unknown".into());
    }
    if a.verified.as_deref() == Some("violated") {
        triggers.push(
            if a.reported == "succeeded" {
                "reported_verified_disagree"
            } else {
                "verified_violated"
            }
            .into(),
        );
    }
    if a.interventions > 0 {
        triggers.push("intervention".into());
    }

    // Members joined by id.
    let mut members: BTreeMap<String, (&'static str, String)> = BTreeMap::new();
    for id in &a.member_ids {
        members.insert(id.clone(), ("id_match", "task_id".into()));
    }
    for goal in &a.goals {
        let mut stmt = tx.prepare(
            "SELECT event_id FROM events WHERE tenant = ?1 AND robot = ?2 AND goal_id = ?3 AND task_id IS NULL",
        )?;
        let ids: Vec<String> = stmt
            .query_map(params![a.tenant, a.robot, goal], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        for id in ids {
            members.entry(id).or_insert(("id_match", "goal_id".into()));
        }
    }

    // Members joined by a time window on the same robot: events that name no task and no goal of their own.
    let mut window_violation = false;
    if let (Some(start), Some(end)) = (a.started_ns, a.ended_ns) {
        let lo = start - WINDOW_BEFORE_NS;
        let hi = end + WINDOW_AFTER_NS;
        let mut stmt = tx.prepare(
            "SELECT event_id, observed_ns, uncertainty_ms, verified, goal_id, component FROM events
             WHERE tenant = ?1 AND robot = ?2 AND task_id IS NULL AND observed_ns BETWEEN ?3 AND ?4
               AND clock_domain = ?5
             ORDER BY observed_ns, event_id",
        )?;
        // Fetch with a margin, then apply each event's own uncertainty exactly.
        let margin = 60_000_000_000;
        let rows: Vec<WindowRow> = stmt
            .query_map(
                params![a.tenant, a.robot, lo - margin, hi + margin, a.clock_domain],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )?
            .collect::<Result<_, _>>()?;
        for (id, t, u, verified, goal, component) in rows {
            if goal.is_some() || members.contains_key(&id) {
                continue;
            }
            // Only times on the task's own clock are compared, and an event about another component of the robot
            // (both sides say which) is not this task's evidence.
            if let (Some(mine), Some(theirs)) = (&a.component, &component)
                && mine != theirs
            {
                continue;
            }
            let u_ns = (u.unwrap_or(0.0) * 1e6) as i64;
            if t + u_ns < lo || t - u_ns > hi {
                continue;
            }
            if verified.as_deref() == Some("violated") {
                window_violation = true;
            }
            members.insert(
                id,
                (
                    "window",
                    format!("[-30 s, +10 s] of the task, ±{} ms", u.unwrap_or(0.0)),
                ),
            );
        }
    }
    if window_violation {
        triggers.push("window_verified_violated".into());
    }
    if triggers.is_empty() {
        return Ok(());
    }

    let category: Option<String> = tx
        .query_row(
            "SELECT category FROM events WHERE tenant = ?1 AND robot = ?2 AND task_id = ?3
             AND type = 'fault.raised' AND category IS NOT NULL ORDER BY observed_ns, event_id LIMIT 1",
            params![a.tenant, a.robot, a.task_id],
            |r| r.get(0),
        )
        .optional()?;
    let fingerprint = format!(
        "{}/{}/{}",
        a.task_type,
        category.unwrap_or_else(|| triggers[0].clone()),
        a.reported
    );
    let key = format!("{}|{}|{}|{}", a.tenant, RULES_VERSION, a.robot, a.task_id);
    let digest = sha256_digest(&key);
    let incident_id = format!("INC-{}", &digest["sha256:".len().."sha256:".len() + 12]);
    tx.execute(
        "INSERT INTO incidents (tenant, incident_id, rules_version, robot, site, task_id, task_type, opened_ns,
             closed_ns, triggers, fingerprint, reported, verified, manifest)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            a.tenant,
            incident_id,
            RULES_VERSION,
            a.robot,
            a.site,
            a.task_id,
            a.task_type,
            a.started_ns,
            a.ended_ns,
            serde_json::to_string(&triggers)?,
            fingerprint,
            a.reported,
            a.verified,
            a.manifest
        ],
    )?;
    for (event_id, (basis, detail)) in &members {
        tx.execute(
            "INSERT INTO incident_members (tenant, incident_id, event_id, basis, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![a.tenant, incident_id, event_id, basis, detail],
        )?;
    }
    report.incidents += 1;
    report.members += members.len();
    Ok(())
}
