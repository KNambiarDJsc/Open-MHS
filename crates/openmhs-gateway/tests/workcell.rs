//! M9 acceptance: the workcell example (examples/workcell/plan.json) on the virtual lab,
//! through the real gateway, clean and under simulator faults. The engine's plan tests
//! fault every step one by one; here the faults are the simulators' own.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

use std::net::{Ipv4Addr, SocketAddr};

use openmhs_api::{Client, CreateTokenParams, Role};
use openmhs_gateway::{GatewayConfig, RunningGateway};
use openmhs_policy::{Effect, Grant};
use openmhs_types::descriptor::Operation;
use openmhs_types::plan::{Plan, PlanDecision, PlanSpec, PlanState, StepStatus};
use openmhs_types::{CommandState, Value};

const PLAN: &str = include_str!("../../../examples/workcell/plan.json");
const PSU: &str = "sim.psu-01";
const AXIS: &str = "sim.axis-01";
const STEPS: [&str; 5] = ["set-voltage", "output-on", "home", "move", "output-off"];

struct Lab {
    gw: RunningGateway,
    operator: Client,
    agent: Client,
    _home: tempfile::TempDir,
}

async fn lab() -> Lab {
    let home = tempfile::tempdir().unwrap();
    let mut config = GatewayConfig::new(home.path());
    config.listen = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    config.simulation = true;
    let gw = openmhs_gateway::start(config).await.unwrap();
    let operator = Client::new(&gw.url, &gw.operator_token).unwrap();
    for device in [PSU, AXIS] {
        operator
            .add_grant(Grant {
                id: format!("cell-{device}"),
                principal: "agent:cell".into(),
                device: device.into(),
                capability: "*".into(),
                operations: vec![Operation::Read, Operation::Write, Operation::Invoke],
                effect: Effect::Allow,
                max_side_effect: None,
                reason: None,
            })
            .await
            .unwrap();
    }
    let token = operator
        .create_token(CreateTokenParams::unrestricted("agent:cell", Role::Agent))
        .await
        .unwrap();
    let agent = Client::new(&gw.url, &token.token).unwrap();
    Lab {
        gw,
        operator,
        agent,
        _home: home,
    }
}

fn plan() -> PlanSpec {
    serde_json::from_str(PLAN).unwrap()
}

fn statuses(p: &Plan) -> Vec<StepStatus> {
    STEPS.iter().map(|s| p.step(s).unwrap().status).collect()
}

async fn value(c: &Client, device: &str, capability: &str) -> Value {
    c.read(device, capability).await.unwrap().state.value
}

/// A quantity reading, near enough to `expected` for a simulator with noise.
async fn near(c: &Client, device: &str, capability: &str, expected: f64) {
    match value(c, device, capability).await {
        Value::Quantity(q) => assert!((q.value - expected).abs() < 0.1, "{capability}: {q:?}"),
        other => panic!("{capability}: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_example_preflights_and_runs_on_the_virtual_lab() {
    let lab = lab().await;
    let check = lab.agent.preflight_plan(plan()).await.unwrap();
    assert!(check.ok, "{check:?}");
    assert_eq!(check.order, STEPS);

    let p = lab.agent.submit_plan(plan()).await.unwrap();
    assert_eq!(p.state, PlanState::Succeeded, "{p:?}");
    for s in STEPS {
        let receipt = lab
            .agent
            .command(p.step(s).unwrap().last_command().unwrap())
            .await
            .unwrap();
        assert_eq!(receipt.state, CommandState::Verified, "{s}: {receipt:?}");
    }
    near(&lab.agent, AXIS, "axis.position", 50.0).await;
    assert_eq!(
        value(&lab.agent, PSU, "power.output.enabled").await,
        Value::Bool(false)
    );
    lab.gw.shutdown().await.unwrap();
}

/// The axis refuses: the power steps are undone, newest first, and the axis is left as
/// it was.
#[tokio::test(flavor = "multi_thread")]
async fn an_axis_that_refuses_is_compensated_back_to_safe() {
    let lab = lab().await;
    lab.operator.sim_fault(AXIS, "reject").await.unwrap();
    let p = lab.agent.submit_plan(plan()).await.unwrap();
    assert_eq!(p.state, PlanState::Compensated, "{p:?}");
    assert_eq!(
        statuses(&p),
        [
            StepStatus::Compensated,
            StepStatus::Compensated,
            StepStatus::Failed,
            StepStatus::Pending,
            StepStatus::Pending,
        ]
    );
    assert_eq!(
        value(&lab.agent, PSU, "power.output.enabled").await,
        Value::Bool(false)
    );
    near(&lab.agent, PSU, "power.output.voltage.setpoint", 0.0).await;
    lab.gw.shutdown().await.unwrap();
}

/// The supply refuses its first command: nothing reaches the axis, and there is nothing
/// to undo.
#[tokio::test(flavor = "multi_thread")]
async fn a_supply_that_refuses_stops_the_cell_before_the_axis() {
    let lab = lab().await;
    lab.operator.sim_fault(PSU, "reject").await.unwrap();
    let p = lab.agent.submit_plan(plan()).await.unwrap();
    assert_eq!(p.state, PlanState::Compensated, "{p:?}");
    assert_eq!(p.step("set-voltage").unwrap().status, StepStatus::Failed);
    for s in ["home", "move"] {
        assert!(p.step(s).unwrap().commands.is_empty(), "{s} was sent");
    }
    lab.gw.shutdown().await.unwrap();
}

/// The supply applies the setpoint but never answers. The plan does not guess: it waits.
/// The operator clears the fault, the command is reconciled by reading the device, and
/// the operator resumes; the cell finishes.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_outcome_waits_for_reconcile_and_a_decision() {
    let lab = lab().await;
    lab.operator
        .sim_fault(PSU, "apply-then-hang")
        .await
        .unwrap();
    let p = lab.agent.submit_plan(plan()).await.unwrap();
    assert_eq!(p.state, PlanState::AwaitingDecision, "{p:?}");
    assert_eq!(p.step("set-voltage").unwrap().status, StepStatus::Uncertain);
    assert_eq!(p.step("output-on").unwrap().commands.len(), 0);

    // Deciding before the facts are known is refused.
    let e = lab
        .operator
        .decide_plan(&p.id, PlanDecision::Resume, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("plan.decision.uncertain"));

    lab.operator.sim_fault(PSU, "clear").await.unwrap();
    let command = p.step("set-voltage").unwrap().last_command().unwrap();
    let settled = lab.agent.reconcile(command).await.unwrap();
    assert_eq!(
        settled.state,
        CommandState::ReconciledApplied,
        "{settled:?}"
    );
    let p = lab
        .operator
        .decide_plan(
            &p.id,
            PlanDecision::Resume,
            Some("setpoint read back".into()),
        )
        .await
        .unwrap();
    assert_eq!(p.state, PlanState::Succeeded, "{p:?}");
    lab.gw.shutdown().await.unwrap();
}
