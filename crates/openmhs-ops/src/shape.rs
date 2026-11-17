//! The shape of an incident (ADR-0035, rules `shape-2` from ADR-0036): how it went wrong, computed from typed fields
//! only.
//!
//! Inputs are the incident's member events: their type, time, clock, component, ids, reported and verified values,
//! attributes, the verdicts' conditions (`outcome.verified.conditions`) and the values a reporter judged its own failure
//! on (`outcome.reported.conditions`), which stay labelled `reported`. Never `ext`, never free text. The words of the
//! headline are temporal and structural; nothing here says why (ADR-0030).

use std::collections::{BTreeMap, BTreeSet};

use openmhs_events::{Side, VerifiedCondition};
use serde::{Deserialize, Serialize};

use crate::stats::quantile;

/// Version of the shape rules. Every stored shape carries it.
pub const SHAPE_RULES: &str = "shape-2";

const FAILED_ACTION: &[&str] = &["failed", "aborted", "timeout", "canceled"];
const FAILED_TASK: &[&str] = &["failed", "aborted", "timeout"];

fn verified() -> String {
    "verified".into()
}

/// A failing condition of one verdict, or of a reporter's own judgement, and by how much it missed when that can be
/// measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionMiss {
    /// `verified` (a verdict's condition) or `reported` (the reporter's own, ADR-0036). Never pooled.
    #[serde(default = "verified")]
    pub source: String,
    pub contract: Option<String>,
    /// The condition's name in the contract, else `<evidence> <op>`.
    pub condition: String,
    /// `above`, `below` or `both`; absent when no distance applies (a category, mixed units, no observed value).
    pub side: Option<String>,
    /// The smallest distance outside the bound over the observed values.
    pub at_least: Option<f64>,
    pub unit: Option<String>,
}

/// One member that marks something not going as reported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub event_id: String,
    /// `verified_violated`, `action_<failed|aborted|timeout|canceled>`, `fault`, `intervention`,
    /// `task_<failed|aborted|timeout>`.
    pub kind: String,
    /// How the event joined the incident: `id_match` or `window` (ADR-0030).
    pub basis: String,
    pub observed_ns: Option<i64>,
    /// Seconds from the task's start (negative before it).
    pub offset_s: Option<f64>,
    /// `offset_s` as a fraction of the task's duration, when both ends are known.
    pub fraction: Option<f64>,
    pub component: Option<String>,
    pub goal_id: Option<String>,
    pub action_id: Option<String>,
    /// `attributes.code`, else `attributes.category`.
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub misses: Vec<ConditionMiss>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalCounts {
    pub satisfied: u64,
    pub violated: u64,
    pub unknown: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goals {
    pub per_component: BTreeMap<String, GoalCounts>,
    pub verified: u64,
    pub violated: u64,
    /// `none`, `one`, `some` or `all` of the verified goals violated (`all` when every one is, even if that is one).
    pub spread: String,
    /// The first violated goal's place among the verified goals in time order: (k, n).
    pub first_violated: Option<(u64, u64)>,
}

/// A failing condition summed over the incident's violated goals (`verified`), or over its failed tasks and actions
/// whose report carries the condition (`reported`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MissSummary {
    #[serde(default = "verified")]
    pub source: String,
    pub contract: Option<String>,
    pub condition: String,
    /// Violated goals (`verified`) or failed tasks and actions (`reported`) that failed this condition.
    pub goals: u64,
    pub above: u64,
    pub below: u64,
    pub both_sides: u64,
    /// Goals whose distance could be measured.
    pub measured: u64,
    pub unit: Option<String>,
    /// (min, median, max) over measured goals of the distance each missed by at least; only in one unit.
    pub at_least: Option<(f64, f64, f64)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Disagreements {
    /// Goals whose action reported `succeeded` and whose verdict is `violated`.
    pub reported_succeeded_verified_violated: u64,
    /// Goals whose action reported `failed`, `aborted` or `timeout` and whose verdict is `satisfied`.
    pub reported_failed_verified_satisfied: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub rules: String,
    /// One sentence from a fixed template over the fields below.
    pub headline: String,
    /// `task type / first point kind / component / failing condition keys`, for recurrence (ADR-0030, rule 4).
    pub signature: String,
    /// In time order; events without a time last.
    pub points: Vec<Point>,
    /// Points whose order against the first is not known: their times overlap within clock uncertainty, or they are
    /// on clocks that cannot be compared.
    pub first_unordered_with: Vec<String>,
    pub goals: Goals,
    pub misses: Vec<MissSummary>,
    pub disagreements: Disagreements,
    /// What the events do not contain, said instead of guessed.
    pub notes: Vec<String>,
}

/// One member event, as the shape reads it.
pub(crate) struct MemberIn {
    pub event_id: String,
    pub kind: String,
    pub basis: String,
    pub observed_ns: Option<i64>,
    pub uncertainty_ms: Option<f64>,
    pub clock_domain: String,
    pub component: Option<String>,
    pub goal_id: Option<String>,
    pub action_id: Option<String>,
    pub reported: Option<String>,
    pub verified: Option<String>,
    pub code: Option<String>,
    pub category: Option<String>,
    pub contract: Option<String>,
    pub conditions: Vec<VerifiedCondition>,
    /// `outcome.reported.conditions` (ADR-0036).
    pub reported_conditions: Vec<VerifiedCondition>,
}

pub(crate) struct TaskIn<'a> {
    pub task_type: &'a str,
    pub started_ns: Option<i64>,
    pub ended_ns: Option<i64>,
}

fn rank(verified: Option<&str>) -> u8 {
    match verified {
        Some("violated") => 2,
        Some("unknown") => 1,
        _ => 0,
    }
}

fn time_key(m: &MemberIn) -> (bool, i64, &str) {
    (
        m.observed_ns.is_none(),
        m.observed_ns.unwrap_or(0),
        m.event_id.as_str(),
    )
}

/// Whether the order of two events cannot be known: same clock and times within both uncertainties (an unknown
/// uncertainty compares directly, as in RFC 0004), or different clocks without both uncertainties, or no time.
fn unordered(a: &MemberIn, b: &MemberIn) -> bool {
    let (Some(ta), Some(tb)) = (a.observed_ns, b.observed_ns) else {
        return true;
    };
    let unc = |m: &MemberIn| m.uncertainty_ms.map(|u| (u * 1e6) as i64);
    match (a.clock_domain == b.clock_domain, unc(a), unc(b)) {
        (true, ua, ub) => (ta - tb).abs() <= ua.unwrap_or(0) + ub.unwrap_or(0),
        (false, Some(ua), Some(ub)) => (ta - tb).abs() <= ua + ub,
        (false, _, _) => true,
    }
}

fn miss_of(source: &str, contract: Option<&String>, c: &VerifiedCondition) -> ConditionMiss {
    let m = c.miss();
    ConditionMiss {
        source: source.to_owned(),
        contract: contract.cloned(),
        condition: c.label(),
        side: m.as_ref().map(|m| {
            m.side
                .map_or("both", |s| match s {
                    Side::Above => "above",
                    Side::Below => "below",
                })
                .to_owned()
        }),
        at_least: m.as_ref().map(|m| m.at_least),
        unit: m.and_then(|m| m.unit),
    }
}

fn point_kind(m: &MemberIn) -> Option<String> {
    let reported = m.reported.as_deref().unwrap_or_default();
    match m.kind.as_str() {
        "outcome.verified" if m.verified.as_deref() == Some("violated") => {
            Some("verified_violated".into())
        }
        "action.ended" if FAILED_ACTION.contains(&reported) => Some(format!("action_{reported}")),
        "fault.raised" => Some("fault".into()),
        "intervention.started" => Some("intervention".into()),
        "task.ended" if FAILED_TASK.contains(&reported) => Some(format!("task_{reported}")),
        _ => None,
    }
}

/// Decimals to print a distance with: three, and more when needed for its first significant digit and one more, so a
/// miss of a few millimetres or of 0.0003 never prints as zero.
pub fn decimals(d: f64) -> usize {
    if d > 0.0 && d < 0.001 {
        ((-d.log10()).floor() as usize + 2).min(9)
    } else {
        3
    }
}

fn amount(d: f64, unit: Option<&str>) -> String {
    let n = decimals(d);
    match unit {
        Some(u) => format!("{d:.n$} {u}"),
        None => format!("{d:.n$}"),
    }
}

fn miss_text(m: &ConditionMiss) -> String {
    match (m.side.as_deref(), m.at_least) {
        (Some("both"), Some(d)) => format!(
            "{}: outside on both sides by at least {}",
            m.condition,
            amount(d, m.unit.as_deref())
        ),
        (Some(side), Some(d)) => format!(
            "{}: {side} the bound by at least {}",
            m.condition,
            amount(d, m.unit.as_deref())
        ),
        _ => format!("{}: not met", m.condition),
    }
}

/// A failed task's or action's own reported misses, said as the reporter's (ADR-0036).
fn reported_detail(p: &Point) -> String {
    let misses: Vec<String> = p.misses.iter().map(miss_text).collect();
    if misses.is_empty() {
        String::new()
    } else {
        format!(" (reported {})", misses.join(", reported "))
    }
}

fn describe(p: &Point, goals: &Goals) -> String {
    let component = p.component.as_deref().unwrap_or("unnamed");
    match p.kind.as_str() {
        "verified_violated" => {
            let place = goals
                .first_violated
                .map(|(k, n)| format!(" ({k} of {n} verified goals)"))
                .unwrap_or_default();
            let misses: Vec<String> = p.misses.iter().map(miss_text).collect();
            let detail = if misses.is_empty() {
                String::new()
            } else {
                format!(", {}", misses.join(", "))
            };
            format!("{component} goal verified violated{place}{detail}")
        }
        "fault" => format!(
            "fault {} ({component})",
            p.code.as_deref().unwrap_or("without a code")
        ),
        "intervention" => format!(
            "intervention {}",
            p.code.as_deref().unwrap_or("without a category")
        ),
        k if k.starts_with("action_") => format!(
            "{component} action ended {}{}",
            &k["action_".len()..],
            reported_detail(p)
        ),
        k if k.starts_with("task_") => {
            format!("task ended {}{}", &k["task_".len()..], reported_detail(p))
        }
        k => k.to_owned(),
    }
}

/// The shape of one incident's members. Deterministic: the same members give the same shape.
pub(crate) fn compute(task: &TaskIn<'_>, members: &[MemberIn]) -> Shape {
    let mut sorted: Vec<&MemberIn> = members.iter().collect();
    sorted.sort_by(|a, b| time_key(a).cmp(&time_key(b)));

    // Goals: the worst verdict per goal (violated over unknown over satisfied), in time order.
    let mut action_component: BTreeMap<&str, &str> = BTreeMap::new();
    let mut action_reported: BTreeMap<&str, &str> = BTreeMap::new();
    for m in &sorted {
        if let (Some(g), true) = (m.goal_id.as_deref(), m.kind.starts_with("action.")) {
            if let Some(c) = m.component.as_deref() {
                action_component.entry(g).or_insert(c);
            }
            if m.kind == "action.ended"
                && let Some(r) = m.reported.as_deref()
            {
                action_reported.insert(g, r);
            }
        }
    }
    let mut verdict_of: BTreeMap<String, &MemberIn> = BTreeMap::new();
    for m in sorted.iter().filter(|m| m.kind == "outcome.verified") {
        let Some(v) = m.verified.as_deref() else {
            continue;
        };
        let goal = m
            .goal_id
            .clone()
            .or_else(|| m.action_id.clone())
            .unwrap_or_else(|| m.event_id.clone());
        match verdict_of.get(&goal) {
            Some(prev) if rank(prev.verified.as_deref()) >= rank(Some(v)) => {}
            _ => {
                verdict_of.insert(goal, m);
            }
        }
    }
    let mut goal_list: Vec<(&String, &MemberIn)> =
        verdict_of.iter().map(|(g, m)| (g, *m)).collect();
    goal_list.sort_by(|a, b| time_key(a.1).cmp(&time_key(b.1)));
    let component_of = |goal: &str, m: &MemberIn| -> String {
        m.component
            .clone()
            .or_else(|| action_component.get(goal).map(|c| (*c).to_owned()))
            .unwrap_or_else(|| "(unspecified)".into())
    };
    let mut goals = Goals::default();
    let mut disagreements = Disagreements::default();
    for (i, (goal, m)) in goal_list.iter().enumerate() {
        let counts = goals
            .per_component
            .entry(component_of(goal, m))
            .or_default();
        goals.verified += 1;
        match m.verified.as_deref() {
            Some("violated") => {
                counts.violated += 1;
                goals.violated += 1;
                if goals.first_violated.is_none() {
                    goals.first_violated = Some((i as u64 + 1, goal_list.len() as u64));
                }
            }
            Some("unknown") => counts.unknown += 1,
            _ => counts.satisfied += 1,
        }
        match (
            action_reported.get(goal.as_str()).copied(),
            m.verified.as_deref(),
        ) {
            (Some("succeeded"), Some("violated")) => {
                disagreements.reported_succeeded_verified_violated += 1
            }
            (Some("failed" | "aborted" | "timeout"), Some("satisfied")) => {
                disagreements.reported_failed_verified_satisfied += 1
            }
            _ => {}
        }
    }
    goals.spread = match (goals.violated, goals.verified) {
        (0, _) => "none",
        (v, n) if v == n => "all",
        (1, _) => "one",
        _ => "some",
    }
    .into();
    let chosen: BTreeSet<&str> = verdict_of.values().map(|m| m.event_id.as_str()).collect();

    // Points, in time order.
    let duration = match (task.started_ns, task.ended_ns) {
        (Some(s), Some(e)) if e > s => Some((e - s) as f64 / 1e9),
        _ => None,
    };
    let mut points = Vec::new();
    let mut point_members: Vec<&MemberIn> = Vec::new();
    let mut silent_verdicts = 0u64;
    for m in &sorted {
        let Some(kind) = point_kind(m) else {
            continue;
        };
        if kind == "verified_violated" && !chosen.contains(m.event_id.as_str()) {
            continue;
        }
        let offset_s = match (m.observed_ns, task.started_ns) {
            (Some(t), Some(s)) => Some((t - s) as f64 / 1e9),
            _ => None,
        };
        let goal = m.goal_id.as_deref().unwrap_or_default();
        // A verdict's misses are its verified conditions; a failed task's or action's are its reporter's own.
        let misses: Vec<ConditionMiss> = if kind == "verified_violated" {
            m.conditions
                .iter()
                .filter(|c| c.result == "false")
                .map(|c| miss_of("verified", m.contract.as_ref(), c))
                .collect()
        } else if kind.starts_with("task_") || kind.starts_with("action_") {
            m.reported_conditions
                .iter()
                .filter(|c| c.result == "false")
                .map(|c| miss_of("reported", None, c))
                .collect()
        } else {
            Vec::new()
        };
        if kind == "verified_violated" && m.conditions.is_empty() {
            silent_verdicts += 1;
        }
        points.push(Point {
            event_id: m.event_id.clone(),
            kind,
            basis: m.basis.clone(),
            observed_ns: m.observed_ns,
            offset_s,
            fraction: offset_s.zip(duration).map(|(o, d)| o / d),
            component: m
                .component
                .clone()
                .or_else(|| action_component.get(goal).map(|c| (*c).to_owned())),
            goal_id: m.goal_id.clone(),
            action_id: m.action_id.clone(),
            code: m.code.clone().or_else(|| m.category.clone()),
            misses,
        });
        point_members.push(m);
    }
    let first_unordered_with: Vec<String> = match point_members.split_first() {
        Some((first, rest)) => rest
            .iter()
            .filter(|m| unordered(first, m))
            .map(|m| m.event_id.clone())
            .collect(),
        None => Vec::new(),
    };

    // Misses over the violated goals.
    #[derive(Default)]
    struct Acc {
        goals: u64,
        above: u64,
        below: u64,
        both: u64,
        distances: Vec<f64>,
        units: BTreeSet<Option<String>>,
    }
    let mut acc: BTreeMap<(&str, Option<String>, String), Acc> = BTreeMap::new();
    let mut add = |key: (&'static str, Option<String>, String), c: &VerifiedCondition| {
        let a = acc.entry(key).or_default();
        a.goals += 1;
        if let Some(miss) = c.miss() {
            match miss.side {
                Some(Side::Above) => a.above += 1,
                Some(Side::Below) => a.below += 1,
                None => a.both += 1,
            }
            a.distances.push(miss.at_least);
            a.units.insert(miss.unit);
        }
    };
    for m in verdict_of.values() {
        if m.verified.as_deref() != Some("violated") {
            continue;
        }
        for c in m.conditions.iter().filter(|c| c.result == "false") {
            add(("verified", m.contract.clone(), c.label()), c);
        }
    }
    for (p, m) in points.iter().zip(&point_members) {
        if p.kind.starts_with("task_") || p.kind.starts_with("action_") {
            for c in m.reported_conditions.iter().filter(|c| c.result == "false") {
                add(("reported", None, c.label()), c);
            }
        }
    }
    let misses: Vec<MissSummary> = acc
        .into_iter()
        .map(|((source, contract, condition), mut a)| {
            a.distances.sort_unstable_by(f64::total_cmp);
            let one_unit = a.units.len() == 1;
            let at_least = if one_unit {
                match (
                    a.distances.first(),
                    quantile(&a.distances, 0.5),
                    a.distances.last(),
                ) {
                    (Some(lo), Some(mid), Some(hi)) => Some((*lo, mid, *hi)),
                    _ => None,
                }
            } else {
                None
            };
            MissSummary {
                source: source.to_owned(),
                contract,
                condition,
                goals: a.goals,
                above: a.above,
                below: a.below,
                both_sides: a.both,
                measured: a.distances.len() as u64,
                unit: if one_unit {
                    a.units.into_iter().next().flatten()
                } else {
                    None
                },
                at_least,
            }
        })
        .collect();

    // Notes: what the events do not say.
    let mut notes = Vec::new();
    if points.is_empty() {
        notes.push("nothing went wrong that the events can place".to_owned());
    }
    if !first_unordered_with.is_empty() {
        notes.push(format!(
            "the order of the first point against {} other point(s) is not known within clock uncertainty",
            first_unordered_with.len()
        ));
    }
    if silent_verdicts > 0 {
        notes.push(format!(
            "{silent_verdicts} violated verdict(s) carry no measured values"
        ));
    }
    if misses
        .iter()
        .any(|m| m.measured > 0 && m.at_least.is_none())
    {
        notes
            .push("a condition's misses are in more than one unit and are not combined".to_owned());
    }

    // Signature: explainable field by field.
    let signature = match point_members.first() {
        None => format!("{}/none", task.task_type),
        Some(first) => {
            let tied: Vec<&Point> = points
                .iter()
                .filter(|p| {
                    p.event_id == first.event_id || first_unordered_with.contains(&p.event_id)
                })
                .collect();
            let kinds: BTreeSet<&str> = tied.iter().map(|p| p.kind.as_str()).collect();
            let components: BTreeSet<&str> = tied
                .iter()
                .map(|p| p.component.as_deref().unwrap_or("-"))
                .collect();
            let conditions: BTreeSet<String> = tied
                .iter()
                .flat_map(|p| {
                    p.misses.iter().map(|m| {
                        if m.source == "reported" {
                            format!("reported:{}", evidence_of(first, m))
                        } else {
                            format!(
                                "{}:{}",
                                m.contract.as_deref().unwrap_or("-"),
                                evidence_of(first, m)
                            )
                        }
                    })
                })
                .collect();
            format!(
                "{}/{}/{}/{}",
                task.task_type,
                kinds.into_iter().collect::<Vec<_>>().join("|"),
                components.into_iter().collect::<Vec<_>>().join("|"),
                if conditions.is_empty() {
                    "-".to_owned()
                } else {
                    conditions.into_iter().collect::<Vec<_>>().join("+")
                }
            )
        }
    };

    let headline = headline(
        &points,
        &first_unordered_with,
        &goals,
        &disagreements,
        silent_verdicts,
    );
    Shape {
        rules: SHAPE_RULES.into(),
        headline,
        signature,
        points,
        first_unordered_with,
        goals,
        misses,
        disagreements,
        notes,
    }
}

/// The evidence item behind a miss of the first point (stable when the threshold in the name changes).
fn evidence_of(first: &MemberIn, m: &ConditionMiss) -> String {
    let list = if m.source == "reported" {
        &first.reported_conditions
    } else {
        &first.conditions
    };
    list.iter()
        .find(|c| c.label() == m.condition)
        .map_or_else(|| m.condition.clone(), |c| c.evidence.clone())
}

fn headline(
    points: &[Point],
    unordered_with: &[String],
    goals: &Goals,
    disagreements: &Disagreements,
    silent_verdicts: u64,
) -> String {
    let mut parts = Vec::new();
    match points.first() {
        None => parts.push("nothing went wrong that the events can place".to_owned()),
        Some(p) => {
            let when = match (p.offset_s, p.fraction) {
                (Some(o), Some(f)) => format!("at {o:.1} s ({:.0} % of the task)", f * 100.0),
                (Some(o), None) => format!("at {o:.1} s"),
                _ => "at an unknown time".to_owned(),
            };
            let order = if unordered_with.is_empty() {
                "first".to_owned()
            } else {
                format!(
                    "first, in an order not known against {} other point(s),",
                    unordered_with.len()
                )
            };
            parts.push(format!("{order} {when}: {}", describe(p, goals)));
        }
    }
    if goals.verified > 0 {
        let per: Vec<String> = goals
            .per_component
            .iter()
            .map(|(c, n)| {
                format!(
                    "{c} {} of {}",
                    n.violated,
                    n.satisfied + n.violated + n.unknown
                )
            })
            .collect();
        parts.push(format!("violated goals: {}", per.join(", ")));
    }
    if let Some(first) = points.first() {
        let others: Vec<&Point> = points
            .iter()
            .skip(1)
            .filter(|p| matches!(p.kind.as_str(), "fault" | "intervention"))
            .take(3)
            .collect();
        for p in others {
            let relation = if unordered_with.contains(&p.event_id) {
                "at the same time as the first point (within clock uncertainty)".to_owned()
            } else {
                match (p.observed_ns, first.observed_ns) {
                    (Some(t), Some(f)) => {
                        format!("{:.1} s after the first point", (t - f) as f64 / 1e9)
                    }
                    _ => "at an unknown time".to_owned(),
                }
            };
            parts.push(format!("{} {relation}", describe(p, goals)));
        }
    }
    if disagreements.reported_succeeded_verified_violated > 0 {
        parts.push(format!(
            "{} goal(s) reported succeeded and verified violated",
            disagreements.reported_succeeded_verified_violated
        ));
    }
    if disagreements.reported_failed_verified_satisfied > 0 {
        parts.push(format!(
            "{} goal(s) reported failed and verified satisfied",
            disagreements.reported_failed_verified_satisfied
        ));
    }
    if silent_verdicts > 0 {
        parts.push(format!(
            "{silent_verdicts} violated verdict(s) carry no measured values"
        ));
    }
    parts.join("; ")
}
