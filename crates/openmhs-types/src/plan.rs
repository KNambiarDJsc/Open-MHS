//! Plans (ADR-0018, spec/plans.md): ordered commands across devices, run under one lease,
//! with defined behaviour when a step fails. Every step is an ordinary command with its own
//! receipt; a plan only decides what to submit next.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::descriptor::{CompareOp, PreconditionValue};
use crate::{ErrorObject, Timestamp};

pub const MAX_PLAN_STEPS: usize = 100;
/// How long the plan's lease runs from each step's start if the plan does not say.
pub const DEFAULT_PLAN_LEASE_TTL_MS: u64 = 15 * 60 * 1_000;

/// What a caller submits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanSpec {
    pub steps: Vec<PlanStep>,
    /// The plan's lease is renewed to this long from the start of each step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_ttl_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanStep {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A command request (spec/schemas/command.schema.json), without an idempotency key:
    /// the plan derives one per attempt.
    pub request: serde_json::Value,
    /// Steps that must have succeeded first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    /// Readings that must hold just before the step is submitted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preconditions: Vec<StepPrecondition>,
    #[serde(default)]
    pub on_failure: OnFailure,
    /// Undoes this step, if the plan compensates after it succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensate: Option<serde_json::Value>,
}

/// A reading a step needs, compared like a descriptor precondition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepPrecondition {
    pub device: String,
    pub capability: String,
    pub op: CompareOp,
    pub value: PreconditionValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// What a definite failure of the step does. An uncertain outcome always waits for a
/// decision, whatever this says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnFailure {
    /// End the plan; completed steps stay as they are.
    #[default]
    Halt,
    /// Run completed steps' compensations, newest first.
    Compensate,
    /// Wait for an approver's decision.
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PlanState {
    Running,
    /// A step is held for approval by policy.
    WaitingApproval,
    /// An approver must decide: resume, retry, compensate or abort.
    AwaitingDecision,
    Compensating,
    Succeeded,
    Halted,
    Compensated,
    Aborted,
    Cancelled,
}

impl PlanState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Halted | Self::Compensated | Self::Aborted | Self::Cancelled
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Running => "RUNNING",
            Self::WaitingApproval => "WAITING_APPROVAL",
            Self::AwaitingDecision => "AWAITING_DECISION",
            Self::Compensating => "COMPENSATING",
            Self::Succeeded => "SUCCEEDED",
            Self::Halted => "HALTED",
            Self::Compensated => "COMPENSATED",
            Self::Aborted => "ABORTED",
            Self::Cancelled => "CANCELLED",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StepStatus {
    Pending,
    Running,
    WaitingApproval,
    Succeeded,
    /// Ended definitely without effect or with a failure the device reported.
    Failed,
    /// Its command may or may not have taken effect.
    Uncertain,
    Compensating,
    Compensated,
    CompensationFailed,
    CompensationUncertain,
}

/// What happened to one step. The commands themselves are ordinary receipts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepRecord {
    pub id: String,
    pub status: StepStatus,
    /// Every command submitted for the step, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
    /// Every compensating command submitted for the step, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compensations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObject>,
}

impl StepRecord {
    pub fn last_command(&self) -> Option<&str> {
        self.commands.last().map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanDecision {
    /// Carry on after the step in question, once its command has succeeded (for example
    /// after reconciliation).
    Resume,
    /// Submit the failed step (or compensation) again.
    Retry,
    /// Compensate the completed steps.
    Compensate,
    /// End the plan where it is.
    Abort,
}

impl PlanDecision {
    pub fn name(self) -> &'static str {
        match self {
            Self::Resume => "resume",
            Self::Retry => "retry",
            Self::Compensate => "compensate",
            Self::Abort => "abort",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub decision: PlanDecision,
    pub by: String,
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub principal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    pub state: PlanState,
    pub spec: PlanSpec,
    /// Step ids in the order they run.
    pub order: Vec<String>,
    pub steps: Vec<StepRecord>,
    /// The step the plan is on, or was on when it stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
    /// Whether the plan has switched to undoing its completed steps.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub compensating: bool,
    /// Why the plan is waiting or how it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<DecisionRecord>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Plan {
    pub fn step(&self, id: &str) -> Option<&StepRecord> {
        self.steps.iter().find(|s| s.id == id)
    }

    pub fn step_mut(&mut self, id: &str) -> Option<&mut StepRecord> {
        self.steps.iter_mut().find(|s| s.id == id)
    }

    pub fn spec_step(&self, id: &str) -> Option<&PlanStep> {
        self.spec.steps.iter().find(|s| s.id == id)
    }
}

/// What a twin preflight expects of one step (spec/plans.md §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepVerdict {
    /// Would be allowed and would pass every check the twin can make.
    Allowed,
    /// Would be held for approval; the twin assumed it approved to check what follows.
    NeedsApproval,
    /// Would be refused or would fail; `errors` say why.
    Refused,
    /// An earlier step stops the plan before this one.
    NotReached,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepPreflight {
    pub id: String,
    pub verdict: StepVerdict,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ErrorObject>,
    /// The step's compensation, if the twin had to run it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensation: Option<StepVerdict>,
}

/// Whether the plan's lease could be taken now, against the real gateway's leases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaseCheck {
    pub available: bool,
    pub claim: crate::graph::Claim,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObject>,
}

/// The result of dry-running a plan against a twin of its devices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanPreflight {
    /// The lease is free now, and every step would run (some after approval).
    pub ok: bool,
    pub lease: LeaseCheck,
    /// How the plan would end in the twin.
    pub outcome: PlanState,
    pub order: Vec<String>,
    pub steps: Vec<StepPreflight>,
    pub checked_at: Timestamp,
}

fn valid_step_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The order steps run in: dependencies first, ties by declaration order. Fails on
/// structural problems (`plan.*` codes), all reported at once.
pub fn execution_order(spec: &PlanSpec) -> Result<Vec<String>, Vec<ErrorObject>> {
    let mut errors = Vec::new();
    if spec.steps.is_empty() {
        errors.push(ErrorObject::new(
            "plan.empty",
            "a plan needs at least one step",
        ));
    }
    if spec.steps.len() > MAX_PLAN_STEPS {
        errors.push(ErrorObject::new(
            "plan.too_many_steps",
            format!("a plan has at most {MAX_PLAN_STEPS} steps"),
        ));
    }
    let mut index = BTreeMap::new();
    for (i, s) in spec.steps.iter().enumerate() {
        if !valid_step_id(&s.id) {
            errors.push(
                ErrorObject::new(
                    "plan.step.id_invalid",
                    format!(
                        "step id '{}' must be 1-64 letters, digits, '-', '_' or '.'",
                        s.id
                    ),
                )
                .detail("step", s.id.clone()),
            );
        }
        if index.insert(s.id.as_str(), i).is_some() {
            errors.push(
                ErrorObject::new(
                    "plan.step.duplicate",
                    format!("step '{}' is declared twice", s.id),
                )
                .detail("step", s.id.clone()),
            );
        }
    }
    for s in &spec.steps {
        for dep in &s.after {
            if !index.contains_key(dep.as_str()) || dep == &s.id {
                errors.push(
                    ErrorObject::new(
                        "plan.step.unknown_dependency",
                        format!(
                            "step '{}' runs after '{dep}', which is not another step",
                            s.id
                        ),
                    )
                    .detail("step", s.id.clone()),
                );
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    // Kahn's algorithm, always taking the earliest-declared ready step.
    let mut done: BTreeSet<&str> = BTreeSet::new();
    let mut order = Vec::with_capacity(spec.steps.len());
    while order.len() < spec.steps.len() {
        let next = spec.steps.iter().find(|s| {
            !done.contains(s.id.as_str()) && s.after.iter().all(|d| done.contains(d.as_str()))
        });
        let Some(next) = next else {
            let stuck: Vec<&str> = spec
                .steps
                .iter()
                .filter(|s| !done.contains(s.id.as_str()))
                .map(|s| s.id.as_str())
                .collect();
            return Err(vec![ErrorObject::new(
                "plan.cycle",
                format!(
                    "steps depend on each other in a cycle: {}",
                    stuck.join(", ")
                ),
            )]);
        };
        done.insert(next.id.as_str());
        order.push(next.id.clone());
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn step(id: &str, after: &[&str]) -> PlanStep {
        PlanStep {
            id: id.into(),
            description: None,
            request: json!({}),
            after: after.iter().map(|s| (*s).to_owned()).collect(),
            preconditions: vec![],
            on_failure: OnFailure::Halt,
            compensate: None,
        }
    }

    fn spec(steps: Vec<PlanStep>) -> PlanSpec {
        PlanSpec {
            steps,
            lease_ttl_ms: None,
            description: None,
        }
    }

    fn codes(r: Result<Vec<String>, Vec<ErrorObject>>) -> Vec<String> {
        r.unwrap_err().into_iter().map(|e| e.code).collect()
    }

    #[test]
    fn dependencies_first_then_declaration_order() {
        let s = spec(vec![
            step("c", &["b"]),
            step("a", &[]),
            step("b", &["a"]),
            step("d", &[]),
        ]);
        assert_eq!(execution_order(&s).unwrap(), ["a", "b", "c", "d"]);
    }

    #[test]
    fn structural_problems_are_all_reported() {
        let s = spec(vec![
            step("a", &["zz"]),
            step("a", &[]),
            step("bad id", &["bad id"]),
        ]);
        let c = codes(execution_order(&s));
        assert!(c.contains(&"plan.step.duplicate".to_owned()));
        assert!(c.contains(&"plan.step.unknown_dependency".to_owned()));
        assert!(c.contains(&"plan.step.id_invalid".to_owned()));
        assert_eq!(codes(execution_order(&spec(vec![]))), ["plan.empty"]);
    }

    #[test]
    fn cycles_are_refused() {
        let s = spec(vec![step("a", &["b"]), step("b", &["a"])]);
        assert_eq!(codes(execution_order(&s)), ["plan.cycle"]);
    }
}
