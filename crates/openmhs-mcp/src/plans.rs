//! Plans for agents (spec/plans.md): the input schema a model fills in, and the views
//! it gets back. Deciding how a waiting plan continues is a human's job, so there is no
//! tool for it here.

use openmhs_types::ErrorObject;
use openmhs_types::plan::{Plan, PlanPreflight, PlanSpec, PlanState, StepStatus, StepVerdict};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::guidance::{Reason, dedup, name, steps_for_reason};
use crate::standard_formats;

// ------------------------------------------------------------------ arguments

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = standard_formats)]
pub struct PlanInput {
    /// What the plan is for, in a sentence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// How long the plan's lease runs from the start of each step (default 900000 = 15
    /// minutes, at most 24 hours).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_ttl_ms: Option<u64>,
    /// 1 to 100 steps. They run one at a time in dependency order; among ready steps,
    /// the one listed first runs first.
    pub steps: Vec<StepInput>,
    /// Run ready steps on separate devices at the same time. Steps that share a device or
    /// resource, or that need approval, still run one at a time.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub parallel: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = standard_formats)]
pub struct StepInput {
    /// Unique in the plan: 1-64 letters, digits, '-', '_' or '.'.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A write or invoke request, without an idempotency_key (the plan makes one per
    /// attempt), e.g. {"target": {"device": "sim.psu-01", "capability":
    /// "power.output.enabled"}, "operation": "write", "value": true} or {"target":
    /// {"device": "sim.axis-01", "capability": "axis.move_absolute"}, "operation":
    /// "invoke", "params": {"position": {"value": 50, "unit": "mm"}}}.
    pub request: Value,
    /// Ids of steps that must have succeeded first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    /// Readings that must hold just before the step is sent; otherwise the step fails
    /// without sending anything.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preconditions: Vec<PreconditionInput>,
    /// Readings that decide whether the step runs at all: if one is definitely false
    /// the step is skipped (and so is every step after it in `after`), and nothing is
    /// sent for it. A reading that is missing or stale fails the step instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when: Vec<PreconditionInput>,
    /// What a definite failure does: `halt` (default: stop, leave completed steps as
    /// they are), `compensate` (undo completed steps, newest first) or `ask` (wait for a
    /// human). An unknown outcome always waits for a human.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<String>,
    /// A request that undoes this step, used when the plan compensates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensate: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = standard_formats)]
pub struct PreconditionInput {
    pub device: String,
    pub capability: String,
    /// `eq`, `ne`, `lt`, `le`, `gt`, `ge` or `in`.
    pub op: String,
    /// The value to compare with, in the capability's unit; a list for `in`.
    pub value: Value,
    /// The reading must be at most this old.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl PlanInput {
    /// The gateway's plan type. A shape error comes back as text the model can fix.
    pub fn into_spec(self) -> Result<PlanSpec, String> {
        let v = serde_json::to_value(self).map_err(|e| e.to_string())?;
        serde_json::from_value(v).map_err(|e| format!("[plan.invalid] {e}"))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanArgs {
    pub plan: PlanInput,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = standard_formats)]
pub struct GetPlanArgs {
    pub plan_id: String,
    /// If set, wait up to this many milliseconds (max 60000) for the plan to end or to
    /// stop for a human.
    #[serde(default)]
    pub wait_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelPlanArgs {
    pub plan_id: String,
    #[serde(default)]
    pub reason: Option<String>,
}

// ------------------------------------------------------------------ views

/// Step messages and plan reasons can carry an adapter's words.
fn reason(e: &ErrorObject) -> Reason {
    Reason {
        message: crate::untrusted::quote(&e.message),
        ..Reason::from(e)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct StepView {
    pub id: String,
    /// `PENDING`, `RUNNING`, `WAITING_APPROVAL`, `SUCCEEDED`, `FAILED`, `UNCERTAIN`,
    /// `COMPENSATING`, `COMPENSATED`, `COMPENSATION_FAILED` or `COMPENSATION_UNCERTAIN`.
    pub status: String,
    /// The step's latest command; use with get_command and reconcile_command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_id: Option<String>,
    /// The latest command that undid it, if the plan compensated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compensation_command_id: Option<String>,
    pub reasons: Vec<Reason>,
}

/// A plan as an agent should see it.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct PlanView {
    /// Use with get_plan and cancel_plan.
    pub plan_id: String,
    /// `RUNNING`, `WAITING_APPROVAL`, `AWAITING_DECISION`, `COMPENSATING`, `SUCCEEDED`,
    /// `HALTED`, `COMPENSATED`, `ABORTED` or `CANCELLED`.
    pub state: String,
    /// True once the plan has ended; nothing more will be sent for it.
    pub finished: bool,
    pub summary: String,
    /// Why it is waiting, or how it ended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The step it is on or stopped at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_step: Option<String>,
    pub steps: Vec<StepView>,
    pub next_steps: Vec<String>,
}

pub fn plan_view(p: &Plan) -> PlanView {
    let steps: Vec<StepView> = p
        .steps
        .iter()
        .map(|s| StepView {
            id: s.id.clone(),
            status: name(&s.status),
            command_id: s.commands.last().cloned(),
            compensation_command_id: s.compensations.last().cloned(),
            reasons: s.error.iter().map(reason).collect(),
        })
        .collect();
    let done = p
        .steps
        .iter()
        .filter(|s| s.status == StepStatus::Succeeded)
        .count();
    let total = p.steps.len();
    let at = p.current.as_deref().unwrap_or("?");
    let (summary, mut next_steps): (String, Vec<String>) = match p.state {
        PlanState::Succeeded => (format!("All {total} steps succeeded."), vec![]),
        PlanState::Running | PlanState::Compensating => (
            format!("In progress: {done} of {total} steps done."),
            vec!["Check again with get_plan(wait_ms).".into()],
        ),
        PlanState::WaitingApproval => (
            format!(
                "Waiting for a human to approve step '{at}'; nothing has been sent for it yet."
            ),
            vec!["Wait with get_plan(wait_ms). Do not resubmit the plan or its step.".into()],
        ),
        PlanState::AwaitingDecision => {
            let uncertain = p.steps.iter().any(|s| {
                matches!(
                    s.status,
                    StepStatus::Uncertain | StepStatus::CompensationUncertain
                )
            });
            let mut steps = vec![
                "Stopped for a human, who decides whether it resumes, retries, compensates or \
                 aborts. You cannot decide; tell the operator and wait with get_plan(wait_ms)."
                    .to_owned(),
            ];
            if uncertain {
                steps.push(
                    "A step's outcome is UNKNOWN: it may or may not have happened. Never \
                     resubmit it. You may reconcile_command its command so the human can \
                     decide on facts."
                        .into(),
                );
            }
            (
                format!(
                    "Stopped at step '{at}' for a human decision ({done} of {total} steps done)."
                ),
                steps,
            )
        }
        PlanState::Halted => (
            format!("Halted at step '{at}'; completed steps were left as they are."),
            vec!["Replan from the step's reasons; do not resubmit the same plan unchanged.".into()],
        ),
        PlanState::Compensated => (
            format!("Step '{at}' failed and the completed steps were undone."),
            vec!["Replan from the step's reasons; do not resubmit the same plan unchanged.".into()],
        ),
        PlanState::Aborted => (
            "Aborted by a human; nothing more will be sent.".into(),
            vec!["Ask the operator before trying again.".into()],
        ),
        PlanState::Cancelled => ("Cancelled; nothing more will be sent.".into(), vec![]),
    };
    for s in &steps {
        for r in &s.reasons {
            next_steps.extend(steps_for_reason(r));
        }
    }
    PlanView {
        plan_id: p.id.clone(),
        state: p.state.name().into(),
        finished: p.state.is_terminal(),
        summary,
        reason: p.reason.as_deref().map(crate::untrusted::quote),
        current_step: p.current.clone(),
        steps,
        next_steps: dedup(next_steps),
    }
}

/// Whether an MCP client should flag the result as an error: the plan did not achieve
/// what was asked, or stopped on something a human must settle. Waiting for approval is
/// not an error.
pub fn is_error(p: &Plan) -> bool {
    matches!(
        p.state,
        PlanState::Halted
            | PlanState::Compensated
            | PlanState::Aborted
            | PlanState::Cancelled
            | PlanState::AwaitingDecision
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct StepCheckView {
    pub id: String,
    /// `allowed`, `needs_approval`, `refused` or `not_reached`.
    pub verdict: String,
    /// The verdict on its compensation, if the twin had to compensate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compensation: Option<String>,
    pub reasons: Vec<Reason>,
}

/// The answer to "would this plan run now?".
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct PlanCheckView {
    /// The lease is free now and every step would run (some after approval).
    pub ok: bool,
    /// How the plan would end in the twin, e.g. `SUCCEEDED` or `HALTED`.
    pub outcome: String,
    pub lease_available: bool,
    /// The devices the plan's lease would claim.
    pub lease_devices: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_reason: Option<Reason>,
    pub steps: Vec<StepCheckView>,
    pub next_steps: Vec<String>,
    /// Always true: the twins have no physics, and state can change before submission.
    pub advisory: bool,
}

pub fn plan_check_view(p: &PlanPreflight) -> PlanCheckView {
    let steps: Vec<StepCheckView> = p
        .steps
        .iter()
        .map(|s| StepCheckView {
            id: s.id.clone(),
            verdict: name(&s.verdict),
            compensation: s.compensation.map(|c| name(&c)),
            reasons: s.errors.iter().map(reason).collect(),
        })
        .collect();
    let lease_reason = p.lease.error.as_ref().map(reason);
    let mut next_steps: Vec<String> = lease_reason
        .iter()
        .chain(steps.iter().flat_map(|s| s.reasons.iter()))
        .flat_map(steps_for_reason)
        .collect();
    if p.ok {
        next_steps.push(
            if p.steps
                .iter()
                .any(|s| s.verdict == StepVerdict::NeedsApproval)
            {
                "Would run, with some steps held for human approval. Submit it with submit_plan; \
             every step is checked again then."
                    .into()
            } else {
                "Would run against the current readings. Submit it with submit_plan; every step \
             is checked again then. The check has no physics: a device can still fail."
                    .into()
            },
        );
    }
    PlanCheckView {
        ok: p.ok,
        outcome: p.outcome.name().into(),
        lease_available: p.lease.available,
        lease_devices: p.lease.claim.devices.iter().cloned().collect(),
        lease_reason,
        steps,
        next_steps: dedup(next_steps),
        advisory: true,
    }
}
