//! M9: plans through the real gateway (spec/plans.md). Each step is an ordinary command
//! of the plan's caller; deciding how a waiting plan continues takes an operator.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

use std::net::{Ipv4Addr, SocketAddr};

use openmhs_api::{Client, ClientError, CreateTokenParams, CreatedToken, Role};
use openmhs_gateway::{GatewayConfig, RunningGateway};
use openmhs_policy::{Effect, Grant};
use openmhs_types::descriptor::Operation;
use openmhs_types::plan::{PlanDecision, PlanSpec, PlanState, StepStatus, StepVerdict};
use serde_json::{Value, json};

const PSU: &str = "sim.psu-01";
const SETPOINT: &str = "power.output.voltage.setpoint";
const ENABLED: &str = "power.output.enabled";

struct Lab {
    gw: RunningGateway,
    operator: Client,
    _home: tempfile::TempDir,
}

fn grant(id: &str, capability: &str, effect: Effect) -> Grant {
    Grant {
        id: id.into(),
        principal: "agent:*".into(),
        device: PSU.into(),
        capability: capability.into(),
        operations: vec![Operation::Read, Operation::Write],
        effect,
        max_side_effect: None,
        reason: None,
    }
}

/// Agents may set the PSU; switching its output on is held for approval.
async fn lab() -> Lab {
    let home = tempfile::tempdir().unwrap();
    let mut config = GatewayConfig::new(home.path());
    config.listen = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    config.simulation = true;
    let gw = openmhs_gateway::start(config).await.unwrap();
    let operator = Client::new(&gw.url, &gw.operator_token).unwrap();
    operator
        .add_grant(grant("ask", ENABLED, Effect::RequireApproval))
        .await
        .unwrap();
    operator
        .add_grant(grant("rest", "*", Effect::Allow))
        .await
        .unwrap();
    Lab {
        gw,
        operator,
        _home: home,
    }
}

async fn agent(lab: &Lab, name: &str) -> (Client, CreatedToken) {
    let t = lab
        .operator
        .create_token(CreateTokenParams::unrestricted(name, Role::Agent))
        .await
        .unwrap();
    (Client::new(&lab.gw.url, &t.token).unwrap(), t)
}

fn set_voltage(volts: f64) -> Value {
    json!({
        "target": { "device": PSU, "capability": SETPOINT },
        "operation": "write",
        "value": { "value": volts, "unit": "V" }
    })
}

fn output(on: bool) -> Value {
    json!({
        "target": { "device": PSU, "capability": ENABLED },
        "operation": "write",
        "value": on
    })
}

fn plan(steps: Value) -> PlanSpec {
    serde_json::from_value(json!({ "description": "bench power-up", "steps": steps })).unwrap()
}

/// Set 1 V, then switch the output on (held for approval).
fn power_up() -> PlanSpec {
    plan(json!([
        { "id": "set", "request": set_voltage(1.0), "compensate": set_voltage(0.0) },
        { "id": "on", "after": ["set"], "request": output(true),
          "on_failure": "compensate", "compensate": output(false) }
    ]))
}

fn code(e: &ClientError) -> String {
    e.code().map_or_else(|| format!("{e}"), str::to_owned)
}

fn audit_kinds(lab: &Lab) -> Vec<String> {
    lab.gw
        .engine()
        .journal()
        .audit_records()
        .unwrap()
        .into_iter()
        .map(|r| r.kind)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plan_over_http_runs_is_private_and_is_audited() {
    let lab = lab().await;
    let (a, _) = agent(&lab, "agent:a").await;
    let (b, _) = agent(&lab, "agent:b").await;
    let spec = plan(json!([{ "id": "set", "request": set_voltage(1.0) }]));

    let check = a.preflight_plan(spec.clone()).await.unwrap();
    assert!(check.ok, "{check:?}");
    assert_eq!(check.outcome, PlanState::Succeeded);
    assert!(
        a.plans(None).await.unwrap().is_empty(),
        "preflight made a plan"
    );

    let p = a.submit_plan(spec).await.unwrap();
    assert_eq!(p.state, PlanState::Succeeded, "{p:?}");
    let step = p.step("set").unwrap();
    let receipt = a.command(step.last_command().unwrap()).await.unwrap();
    assert_eq!(receipt.principal, "agent:a");
    let lease = a.lease(p.lease_id.as_deref().unwrap()).await.unwrap();
    assert!(lease.released_at.is_some(), "{lease:?}");

    // Another agent cannot see it; the operator can.
    assert_eq!(code(&b.plan(&p.id).await.unwrap_err()), "plan.unknown");
    assert!(b.plans(None).await.unwrap().is_empty());
    assert_eq!(a.plan(&p.id).await.unwrap().id, p.id);
    assert_eq!(lab.operator.plans(None).await.unwrap().len(), 1);

    // A malformed plan is refused whole, with every reason.
    let e = a.submit_plan(plan(json!([]))).await.unwrap_err();
    let ClientError::Rpc(rpc) = &e else {
        panic!("{e}")
    };
    let errors = &rpc.data.as_ref().unwrap().errors;
    assert!(errors.iter().any(|e| e.code == "plan.empty"), "{errors:?}");

    let kinds = audit_kinds(&lab);
    for kind in ["plan.preflight", "plan.submitted", "plan.succeeded"] {
        assert!(kinds.iter().any(|k| k == kind), "no {kind} in {kinds:?}");
    }
    lab.gw.shutdown().await.unwrap();
}

/// A held step pauses the plan; the operator's approval runs it and the plan finishes
/// before the approval call returns.
#[tokio::test(flavor = "multi_thread")]
async fn approving_a_held_step_over_http_finishes_the_plan() {
    let lab = lab().await;
    let (a, _) = agent(&lab, "agent:a").await;

    let check = a.preflight_plan(power_up()).await.unwrap();
    assert!(check.ok, "{check:?}");
    assert_eq!(check.steps[1].verdict, StepVerdict::NeedsApproval);
    assert!(
        lab.operator
            .commands(vec![], None)
            .await
            .unwrap()
            .is_empty(),
        "preflight created a command"
    );

    let p = a.submit_plan(power_up()).await.unwrap();
    assert_eq!(p.state, PlanState::WaitingApproval, "{p:?}");
    let held = p.step("on").unwrap().last_command().unwrap().to_owned();
    lab.operator.approve(&held).await.unwrap();
    let p = a.plan(&p.id).await.unwrap();
    assert_eq!(p.state, PlanState::Succeeded, "{p:?}");
    lab.gw.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn only_an_operator_decides_how_a_plan_continues() {
    let lab = lab().await;
    let (a, _) = agent(&lab, "agent:a").await;
    // 48 V is out of range: the step is denied and `ask` waits for a person.
    let p = a
        .submit_plan(plan(json!([
            { "id": "set", "request": set_voltage(48.0), "on_failure": "ask" }
        ])))
        .await
        .unwrap();
    assert_eq!(p.state, PlanState::AwaitingDecision, "{p:?}");
    assert_eq!(p.step("set").unwrap().status, StepStatus::Failed);

    let refused = a
        .decide_plan(&p.id, PlanDecision::Abort, None)
        .await
        .unwrap_err();
    assert_eq!(code(&refused), "gateway.forbidden");
    let ended = lab
        .operator
        .decide_plan(&p.id, PlanDecision::Abort, Some("wrong voltage".into()))
        .await
        .unwrap();
    assert_eq!(ended.state, PlanState::Aborted);
    assert!(ended.decisions[0].by.starts_with("operator:"), "{ended:?}");
    assert_eq!(ended.decisions[0].decision, PlanDecision::Abort);
    assert!(audit_kinds(&lab).iter().any(|k| k == "plan.decision"));
    lab.gw.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_a_token_cancels_its_waiting_plans() {
    let lab = lab().await;
    let (a, token) = agent(&lab, "agent:a").await;
    let p = a.submit_plan(power_up()).await.unwrap();
    assert_eq!(p.state, PlanState::WaitingApproval, "{p:?}");

    lab.operator.revoke_token(&token.info.id).await.unwrap();
    let p = lab.operator.plan(&p.id).await.unwrap();
    assert_eq!(p.state, PlanState::Cancelled, "{p:?}");
    let lease = lab
        .operator
        .lease(p.lease_id.as_deref().unwrap())
        .await
        .unwrap();
    assert!(lease.released_at.is_some(), "{lease:?}");
    lab.gw.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_submitter_can_cancel_and_others_cannot() {
    let lab = lab().await;
    let (a, _) = agent(&lab, "agent:a").await;
    let (b, _) = agent(&lab, "agent:b").await;
    let p = a.submit_plan(power_up()).await.unwrap();
    assert_eq!(p.state, PlanState::WaitingApproval, "{p:?}");
    assert_eq!(
        code(&b.cancel_plan(&p.id, None).await.unwrap_err()),
        "plan.unknown"
    );
    let p = a
        .cancel_plan(&p.id, Some("not today".into()))
        .await
        .unwrap();
    assert_eq!(p.state, PlanState::Cancelled);
    lab.gw.shutdown().await.unwrap();
}

/// A preflight leaves nothing behind that a real run could trip over.
#[tokio::test(flavor = "multi_thread")]
async fn a_plan_runs_after_its_preflight() {
    let lab = lab().await;
    let (a, _) = agent(&lab, "agent:a").await;
    let spec = plan(json!([
        { "id": "low", "request": set_voltage(1.0) },
        { "id": "high", "after": ["low"], "request": set_voltage(2.0) }
    ]));
    assert!(a.preflight_plan(spec.clone()).await.unwrap().ok);
    let p = tokio::time::timeout(std::time::Duration::from_secs(20), a.submit_plan(spec))
        .await
        .expect("the plan hung after its preflight")
        .unwrap();
    assert_eq!(p.state, PlanState::Succeeded, "{p:?}");
    lab.gw.shutdown().await.unwrap();
}
