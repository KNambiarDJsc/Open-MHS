//! Procedures through the real gateway (spec/procedures.md): an agent proposes, only an
//! operator approves, a run is an ordinary plan as the caller with every check, parameters
//! are refused before anything is created, versions are never replaced, and procedures
//! survive a restart.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};

use openmhs_api::{Client, ClientError, CreateTokenParams, Role, RunProcedureParams};
use openmhs_gateway::{GatewayConfig, RunningGateway};
use openmhs_policy::{Effect, Grant};
use openmhs_types::descriptor::Operation;
use openmhs_types::plan::PlanState;
use openmhs_types::procedure::{ProcedureSpec, ProcedureStatus};
use serde_json::{Value, json};

async fn start(home: &std::path::Path) -> RunningGateway {
    let mut config = GatewayConfig::new(home);
    config.listen = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    config.simulation = true;
    openmhs_gateway::start(config).await.unwrap()
}

async fn clients(gw: &RunningGateway) -> (Client, Client) {
    let operator = Client::new(&gw.url, &gw.operator_token).unwrap();
    for (id, device, ops) in [
        ("read", "*", vec![Operation::Read]),
        ("psu", "sim.psu-01", vec![Operation::Write]),
    ] {
        let _ = operator
            .add_grant(Grant {
                id: id.into(),
                principal: "agent:*".into(),
                device: device.into(),
                capability: "*".into(),
                operations: ops,
                effect: Effect::Allow,
                max_side_effect: None,
                reason: None,
            })
            .await;
    }
    let token = operator
        .create_token(CreateTokenParams::unrestricted("agent:proc", Role::Agent))
        .await
        .unwrap();
    (operator, Client::new(&gw.url, &token.token).unwrap())
}

fn set_output(version: u32) -> ProcedureSpec {
    serde_json::from_value(json!({
        "name": "psu.set-output",
        "version": version,
        "description": "set the supply's voltage",
        "params": { "voltage": { "type": "quantity", "unit": "V", "min": 0, "max": 2 } },
        "plan": { "steps": [ { "id": "set", "request": {
            "target": { "device": "sim.psu-01", "capability": "power.output.voltage.setpoint" },
            "operation": "write", "value": { "$param": "voltage" } } } ] }
    }))
    .unwrap()
}

fn run(name: &str, params: Value) -> RunProcedureParams {
    RunProcedureParams {
        name: name.into(),
        version: None,
        params: serde_json::from_value::<BTreeMap<String, Value>>(params).unwrap(),
    }
}

fn code(e: ClientError) -> String {
    e.code().unwrap_or_default().to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_proposes_an_operator_approves_and_a_run_is_an_ordinary_plan() {
    let home = tempfile::tempdir().unwrap();
    let gw = start(home.path()).await;
    let (operator, agent) = clients(&gw).await;

    let proposed = agent.propose_procedure(set_output(1)).await.unwrap();
    assert_eq!(proposed.status, ProcedureStatus::Proposed);
    assert_eq!(proposed.proposed_by, "agent:proc");
    assert_eq!(proposed.sha256.len(), 64);

    // Proposed is not runnable, and an agent can neither approve nor add.
    let volts = |v: f64| json!({ "voltage": { "value": v, "unit": "V" } });
    let e = agent.run_procedure(run("psu.set-output", volts(1.0))).await;
    assert_eq!(code(e.unwrap_err()), "procedure.not_approved");
    assert_eq!(
        code(
            agent
                .approve_procedure("psu.set-output", 1)
                .await
                .unwrap_err()
        ),
        "gateway.forbidden"
    );
    assert_eq!(
        code(agent.add_procedure(set_output(2)).await.unwrap_err()),
        "gateway.forbidden"
    );

    let approved = operator
        .approve_procedure("psu.set-output", 1)
        .await
        .unwrap();
    assert_eq!(approved.status, ProcedureStatus::Approved);
    assert_eq!(
        approved.sha256, proposed.sha256,
        "what ran is what was approved"
    );

    // A dry run first, then the real one: an ordinary plan, as the agent.
    let check = agent
        .check_procedure(run("psu.set-output", volts(1.0)))
        .await
        .unwrap();
    assert!(check.ok, "{check:?}");
    let plan = agent
        .run_procedure(run("psu.set-output", volts(1.0)))
        .await
        .unwrap();
    assert_eq!(plan.state, PlanState::Succeeded, "{plan:?}");
    assert_eq!(plan.principal, "agent:proc");
    let origin = plan.spec.description.clone().unwrap();
    assert!(
        origin.starts_with(&format!(
            "procedure psu.set-output v1 (sha256 {})",
            &approved.sha256[..16]
        )),
        "{origin}"
    );

    // Parameters are checked before anything is created.
    let before = operator.plans(Some(100)).await.unwrap().len();
    for (bad, expect) in [
        (
            json!({ "voltage": { "value": 1, "unit": "mV" } }),
            "procedure.param.invalid",
        ),
        (volts(5.0), "procedure.param.invalid"),
        (json!({}), "procedure.param.missing"),
    ] {
        let e = agent.run_procedure(run("psu.set-output", bad)).await;
        let err = e.unwrap_err();
        assert!(format!("{err:?}").contains(expect), "{err:?}");
    }
    assert_eq!(operator.plans(Some(100)).await.unwrap().len(), before);

    // Versions are never replaced.
    assert_eq!(
        code(agent.propose_procedure(set_output(1)).await.unwrap_err()),
        "procedure.exists"
    );

    let kinds: Vec<String> = operator
        .audit_tail(Some(200))
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.kind)
        .filter(|k| k.starts_with("procedure."))
        .collect();
    assert_eq!(
        kinds,
        ["procedure.proposed", "procedure.approved", "procedure.run"]
    );
    assert!(operator.verify_audit().await.unwrap().valid);
    gw.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_procedure_grants_nothing_and_survives_a_restart() {
    let home = tempfile::tempdir().unwrap();
    let gw = start(home.path()).await;
    let (operator, agent) = clients(&gw).await;

    // The operator approves a procedure that homes the stage; the agent has no grant for it.
    let home_stage: ProcedureSpec = serde_json::from_value(json!({
        "name": "stage.home", "version": 1,
        "plan": { "steps": [ { "id": "home", "request": {
            "target": { "device": "sim.axis-01", "capability": "axis.home" },
            "operation": "invoke" } } ] }
    }))
    .unwrap();
    operator.add_procedure(home_stage).await.unwrap();
    // Refused before any step: the plan cannot even lease a device the agent may not operate.
    let e = agent.run_procedure(run("stage.home", json!({}))).await;
    assert_eq!(code(e.unwrap_err()), "lease.not_authorized");
    gw.shutdown().await.unwrap();

    let gw = start(home.path()).await;
    let operator = Client::new(&gw.url, &gw.operator_token).unwrap();
    let kept = operator.procedure("stage.home", None).await.unwrap();
    assert_eq!(kept.status, ProcedureStatus::Approved);
    gw.shutdown().await.unwrap();
}
