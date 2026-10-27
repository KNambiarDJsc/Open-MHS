//! The MCP server end to end: a real MCP client, in process, talking to the server,
//! which talks to a real gateway running the virtual lab.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

use std::net::{Ipv4Addr, SocketAddr};

use openmhs_api::{Client, CreateTokenParams, Role};
use openmhs_gateway::{GatewayConfig, RunningGateway};
use openmhs_mcp::OpenMhsServer;
use openmhs_policy::{Effect, Grant};
use openmhs_types::descriptor::Operation;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

const PSU: &str = "sim.psu-01";
const SETPOINT: &str = "power.output.voltage.setpoint";

struct Lab {
    gw: RunningGateway,
    agent_token: String,
    _home: tempfile::TempDir,
}

async fn lab(grants: Vec<Grant>) -> Lab {
    let home = tempfile::tempdir().unwrap();
    let mut config = GatewayConfig::new(home.path());
    config.listen = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    config.simulation = true;
    let gw = openmhs_gateway::start(config).await.unwrap();
    let operator = Client::new(&gw.url, &gw.operator_token).unwrap();
    for g in grants {
        operator.add_grant(g).await.unwrap();
    }
    let agent_token = operator
        .create_token(CreateTokenParams::unrestricted("agent:mcp", Role::Agent))
        .await
        .unwrap()
        .token;
    Lab {
        gw,
        agent_token,
        _home: home,
    }
}

fn grant(id: &str, device: &str, capability: &str, ops: &[Operation], effect: Effect) -> Grant {
    Grant {
        id: id.into(),
        principal: "agent:*".into(),
        device: device.into(),
        capability: capability.into(),
        operations: ops.to_vec(),
        effect,
        max_side_effect: None,
        reason: None,
    }
}

fn psu_grants() -> Vec<Grant> {
    vec![
        grant("read-all", "*", "*", &[Operation::Read], Effect::Allow),
        grant("psu-write", PSU, "*", &[Operation::Write], Effect::Allow),
    ]
}

/// Connect an MCP client to a fresh server over an in-memory pipe.
async fn connect(url: &str, token: &str, allow_operator: bool) -> RunningService<RoleClient, ()> {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server = OpenMhsServer::new(Client::new(url, token).unwrap(), allow_operator);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    ().serve(client_io).await.unwrap()
}

async fn call(client: &RunningService<RoleClient, ()>, tool: &str, args: Value) -> CallToolResult {
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    client.call_tool(params).await.unwrap()
}

fn structured(r: &CallToolResult) -> &Value {
    match &r.structured_content {
        Some(v) => v,
        None => panic!("no structured content: {}", text(r)),
    }
}

fn is_error(r: &CallToolResult) -> bool {
    r.is_error == Some(true)
}

fn text(r: &CallToolResult) -> String {
    r.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn volts(v: f64) -> Value {
    json!({ "value": v, "unit": "V" })
}

#[tokio::test(flavor = "multi_thread")]
async fn tools_declare_their_risk_and_output_schemas() {
    let lab = lab(vec![]).await;
    let client = connect(&lab.gw.url, &lab.agent_token, false).await;
    let info = client.peer_info().unwrap();
    assert!(
        info.instructions
            .as_deref()
            .unwrap()
            .contains("UNKNOWN_OUTCOME")
    );
    assert!(
        info.instructions
            .as_deref()
            .unwrap()
            .contains("<<device-text")
    );

    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for expected in [
        "list_devices",
        "describe_device",
        "read_state",
        "check_command",
        "submit_command",
        "get_command",
        "list_pending_approvals",
        "reconcile_command",
        "cancel_command",
        "check_plan",
        "submit_plan",
        "get_plan",
        "cancel_plan",
    ] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    for t in &tools {
        assert!(t.output_schema.is_some(), "{} has no output schema", t.name);
        assert!(t.description.is_some(), "{} has no description", t.name);
        let a = t.annotations.as_ref().unwrap();
        let read_only = a.read_only_hint == Some(true);
        let mutating = [
            "submit_command",
            "reconcile_command",
            "cancel_command",
            "submit_plan",
            "cancel_plan",
        ];
        assert_eq!(
            read_only,
            !mutating.contains(&t.name.as_ref()),
            "{}",
            t.name
        );
    }
    // Only standard JSON Schema: strict client-side validators warn on `uint64` and kin.
    let all = serde_json::to_string(&tools).unwrap();
    for f in [
        "int8", "int16", "int32", "int64", "uint", "uint8", "uint16", "uint32", "uint64",
    ] {
        assert!(
            !all.contains(&format!("\"format\":\"{f}\"")),
            "format {f} in {all}"
        );
    }
    let submit = tools.iter().find(|t| t.name == "submit_command").unwrap();
    let a = submit.annotations.as_ref().unwrap();
    assert_eq!(a.destructive_hint, Some(true));
    assert_eq!(a.open_world_hint, Some(true));
    let plan = tools.iter().find(|t| t.name == "submit_plan").unwrap();
    assert_eq!(
        plan.annotations.as_ref().unwrap().destructive_hint,
        Some(true)
    );
    // Deciding how a plan continues is a human's job.
    assert!(!names.iter().any(|n| n.contains("decide")), "{names:?}");
    client.cancel().await.unwrap();
    lab.gw.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn discovery_and_reads_return_structured_results() {
    let lab = lab(psu_grants()).await;
    let client = connect(&lab.gw.url, &lab.agent_token, false).await;

    let r = call(&client, "list_devices", json!({})).await;
    assert!(!is_error(&r));
    let devices = structured(&r)["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 3);
    let psu = devices.iter().find(|d| d["id"] == PSU).unwrap();
    let setpoint = psu["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == SETPOINT)
        .unwrap();
    assert_eq!(setpoint["value"]["unit"], "V");
    assert_eq!(setpoint["side_effect"], "reversible");
    // Availability travels with the summary (ADR-0019).
    assert_eq!(psu["availability"], "ONLINE");
    assert_eq!(psu["busy"], false);
    assert!(psu.get("lease_holder").is_none());
    // Descriptor prose reaches the model marked as unattested; identifiers do not.
    let quoted = |v: &Value| {
        let t = v.as_str().unwrap();
        t.starts_with("<<device-text ") && t.ends_with(" device-text>>")
    };
    assert!(quoted(&psu["name"]), "{}", psu["name"]);
    assert_eq!(psu["model"], "VPSU-1");
    assert_eq!(psu["id"], PSU);

    let r = call(&client, "describe_device", json!({ "device": PSU })).await;
    let d = &structured(&r)["descriptor"];
    assert_eq!(d["device"]["id"], PSU);
    let pre = d["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == SETPOINT)
        .unwrap()["preconditions"][0]["description"]
        .clone();
    assert!(quoted(&pre), "{pre}");

    let r = call(
        &client,
        "read_state",
        json!({ "device": PSU, "capability": SETPOINT }),
    )
    .await;
    assert!(!is_error(&r), "{}", text(&r));
    let s = structured(&r);
    assert_eq!(s["value"]["unit"], "V");
    assert_eq!(s["usable"], true);
    // The text block mirrors the structured result for clients that only read text.
    assert_eq!(serde_json::from_str::<Value>(&text(&r)).unwrap(), *s);

    let r = call(&client, "describe_device", json!({ "device": "sim.nope" })).await;
    assert!(is_error(&r));
    assert!(text(&r).contains("device.unknown"), "{}", text(&r));
    client.cancel().await.unwrap();
    lab.gw.shutdown().await.unwrap();
}

/// The heart of M4: a refusal comes back as data the model can replan from.
#[tokio::test(flavor = "multi_thread")]
async fn denials_are_structured_with_rule_ids_and_next_steps() {
    let lab = lab(psu_grants()).await;
    let client = connect(&lab.gw.url, &lab.agent_token, false).await;

    // Out of range.
    let r = call(
        &client,
        "submit_command",
        json!({ "device": PSU, "capability": SETPOINT, "value": volts(48.0), "idempotency_key": "k1" }),
    )
    .await;
    assert!(is_error(&r));
    let v = structured(&r);
    assert_eq!(v["outcome"], "denied");
    assert_eq!(v["state"], "DENIED");
    assert_eq!(v["confirmed"], false);
    assert_eq!(v["reached_device"], false);
    assert_eq!(v["reasons"][0]["code"], "command.value.out_of_range");
    assert_eq!(v["reasons"][0]["details"]["max"], 12.0);
    let steps = v["next_steps"][0].as_str().unwrap();
    assert!(steps.contains("0..12 V"), "{steps}");

    // Slew: 0 V to 9 V at once exceeds 2 V per second; the guidance carries the numbers.
    let r = call(
        &client,
        "submit_command",
        json!({ "device": PSU, "capability": SETPOINT, "value": volts(9.0), "idempotency_key": "k2" }),
    )
    .await;
    let v = structured(&r);
    assert_eq!(v["reasons"][0]["code"], "command.slew_exceeded");
    let steps = v["next_steps"][0].as_str().unwrap();
    assert!(steps.contains("at most 2 V per 1000 ms"), "{steps}");

    // No grant: the reason says so and the guidance says not to work around it.
    let r = call(
        &client,
        "submit_command",
        json!({ "device": "sim.axis-01", "capability": "axis.home", "idempotency_key": "k3" }),
    )
    .await;
    let v = structured(&r);
    assert_eq!(v["reasons"][0]["code"], "policy.denied.no_grant");
    assert!(
        v["next_steps"][0]
            .as_str()
            .unwrap()
            .contains("Do not retry")
    );

    // Preflight sees the same thing without creating a command.
    let r = call(
        &client,
        "check_command",
        json!({ "device": PSU, "capability": SETPOINT, "value": volts(9.0) }),
    )
    .await;
    assert!(
        !is_error(&r),
        "a check that says no is still a successful check"
    );
    let v = structured(&r);
    assert_eq!(v["decision"], "deny");
    assert_eq!(v["reasons"][0]["code"], "command.slew_exceeded");
    assert_eq!(v["advisory"], true);

    // Wrong shape for the capability is caught before the gateway is asked.
    let r = call(
        &client,
        "submit_command",
        json!({ "device": PSU, "capability": SETPOINT, "params": { "x": 1 } }),
    )
    .await;
    assert!(is_error(&r));
    assert!(text(&r).contains("property"), "{}", text(&r));

    client.cancel().await.unwrap();
    lab.gw.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_verified_change_is_confirmed_and_idempotent() {
    let lab = lab(psu_grants()).await;
    let client = connect(&lab.gw.url, &lab.agent_token, false).await;
    let args = json!({ "device": PSU, "capability": SETPOINT, "value": volts(2.0), "idempotency_key": "ramp-1" });

    let r = call(&client, "submit_command", args.clone()).await;
    assert!(!is_error(&r), "{}", text(&r));
    let v = structured(&r).clone();
    assert_eq!(v["state"], "VERIFIED");
    assert_eq!(v["confirmed"], true);
    assert_eq!(v["verification"]["matched"], true);

    // The same key returns the same command; nothing runs twice.
    let again = call(&client, "submit_command", args).await;
    assert_eq!(structured(&again)["command_id"], v["command_id"]);

    let r = call(
        &client,
        "get_command",
        json!({ "command_id": v["command_id"] }),
    )
    .await;
    assert_eq!(structured(&r)["state"], "VERIFIED");
    client.cancel().await.unwrap();
    lab.gw.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn approval_flow_waits_for_a_human() {
    let lab = lab(vec![
        grant("read-all", "*", "*", &[Operation::Read], Effect::Allow),
        grant(
            "psu-approve",
            PSU,
            SETPOINT,
            &[Operation::Write],
            Effect::RequireApproval,
        ),
    ])
    .await;
    let client = connect(&lab.gw.url, &lab.agent_token, false).await;

    let r = call(
        &client,
        "submit_command",
        json!({ "device": PSU, "capability": SETPOINT, "value": volts(1.0), "idempotency_key": "a1" }),
    )
    .await;
    assert!(!is_error(&r), "waiting for approval is not an error");
    let v = structured(&r).clone();
    assert_eq!(v["state"], "AWAITING_APPROVAL");
    assert_eq!(v["confirmed"], false);
    let id = v["command_id"].as_str().unwrap().to_string();

    let r = call(&client, "list_pending_approvals", json!({})).await;
    let pending = structured(&r)["commands"].as_array().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["command_id"], id.as_str());

    // A human approves while the agent waits.
    let operator = Client::new(&lab.gw.url, &lab.gw.operator_token).unwrap();
    let approver = {
        let id = id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            operator.approve(&id).await.unwrap();
        })
    };
    let r = call(
        &client,
        "get_command",
        json!({ "command_id": id, "wait_ms": 10_000 }),
    )
    .await;
    approver.await.unwrap();
    assert_eq!(structured(&r)["state"], "VERIFIED", "{}", text(&r));
    assert_eq!(structured(&r)["confirmed"], true);
    client.cancel().await.unwrap();
    lab.gw.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn operator_tokens_are_refused_unless_allowed() {
    let lab = lab(psu_grants()).await;
    let client = connect(&lab.gw.url, &lab.gw.operator_token, false).await;
    let r = call(&client, "list_devices", json!({})).await;
    assert!(is_error(&r));
    assert!(text(&r).contains("mcp.operator_token"), "{}", text(&r));
    assert!(r.structured_content.is_none());
    client.cancel().await.unwrap();

    let client = connect(&lab.gw.url, &lab.gw.operator_token, true).await;
    let r = call(&client, "list_devices", json!({})).await;
    assert!(!is_error(&r), "{}", text(&r));
    client.cancel().await.unwrap();
    lab.gw.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bad_token_is_a_tool_error_not_a_crash() {
    let lab = lab(vec![]).await;
    let client = connect(&lab.gw.url, "not-a-token", false).await;
    let r = call(&client, "list_devices", json!({})).await;
    assert!(is_error(&r));
    assert!(text(&r).contains("unauthorized"), "{}", text(&r));
    assert!(text(&r).contains("tell the human operator"), "{}", text(&r));
    // The session survives and keeps answering.
    let r = call(&client, "list_devices", json!({})).await;
    assert!(is_error(&r));
    client.cancel().await.unwrap();
    lab.gw.shutdown().await.unwrap();
}

/// Nothing the server returns ever contains a credential: not its own token, not the
/// operator's.
#[tokio::test(flavor = "multi_thread")]
async fn no_output_contains_a_token() {
    let lab = lab(psu_grants()).await;
    let client = connect(&lab.gw.url, &lab.agent_token, false).await;
    let mut outputs = vec![serde_json::to_string(&client.list_all_tools().await.unwrap()).unwrap()];
    outputs.push(serde_json::to_string(&*client.peer_info().unwrap()).unwrap());
    for (tool, args) in [
        ("list_devices", json!({})),
        ("describe_device", json!({ "device": PSU })),
        (
            "read_state",
            json!({ "device": PSU, "capability": SETPOINT }),
        ),
        (
            "check_command",
            json!({ "device": PSU, "capability": SETPOINT, "value": volts(1.0) }),
        ),
        (
            "submit_command",
            json!({ "device": PSU, "capability": SETPOINT, "value": volts(1.0) }),
        ),
        (
            "submit_command",
            json!({ "device": PSU, "capability": SETPOINT, "value": volts(99.0) }),
        ),
        ("list_pending_approvals", json!({})),
        ("get_command", json!({ "command_id": "cmd_nope" })),
    ] {
        outputs.push(serde_json::to_string(&call(&client, tool, args).await).unwrap());
    }
    for out in &outputs {
        assert!(!out.contains(&lab.agent_token), "agent token leaked: {out}");
        assert!(
            !out.contains(&lab.gw.operator_token),
            "operator token leaked: {out}"
        );
    }
    client.cancel().await.unwrap();
    lab.gw.shutdown().await.unwrap();
}

/// Malformed arguments are a tool error the model can read and correct (MCP 2025-11-25:
/// input validation errors belong in the result, not the protocol), and the session lives on.
#[tokio::test(flavor = "multi_thread")]
async fn malformed_arguments_are_correctable_tool_errors() {
    let lab = lab(psu_grants()).await;
    let client = connect(&lab.gw.url, &lab.agent_token, false).await;
    let r = call(
        &client,
        "submit_command",
        json!({ "device": PSU, "voltage": 5 }),
    )
    .await;
    assert!(is_error(&r));
    assert!(text(&r).contains("unknown field `voltage`"), "{}", text(&r));
    let r = call(&client, "list_devices", json!({})).await;
    assert!(!is_error(&r));
    client.cancel().await.unwrap();
    lab.gw.shutdown().await.unwrap();
}

fn write_req(v: f64) -> Value {
    json!({
        "target": { "device": PSU, "capability": SETPOINT },
        "operation": "write",
        "value": volts(v)
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn plans_check_run_and_explain_a_failure() {
    let lab = lab(psu_grants()).await;
    let client = connect(&lab.gw.url, &lab.agent_token, false).await;
    let good = json!({ "plan": { "steps": [
        { "id": "one", "request": write_req(1.0), "compensate": write_req(0.0) }
    ] } });

    let r = call(&client, "check_plan", good.clone()).await;
    assert!(!is_error(&r), "{}", text(&r));
    let v = structured(&r);
    assert_eq!(v["ok"], true);
    assert_eq!(v["advisory"], true);
    assert_eq!(v["steps"][0]["verdict"], "allowed");

    let r = call(&client, "submit_plan", good).await;
    assert!(!is_error(&r), "{}", text(&r));
    let v = structured(&r).clone();
    assert_eq!(v["state"], "SUCCEEDED");
    assert_eq!(v["finished"], true);
    let r = call(&client, "get_plan", json!({ "plan_id": v["plan_id"] })).await;
    assert_eq!(structured(&r)["state"], "SUCCEEDED");

    // The second step is out of range: the first is undone, and the agent learns why.
    let bad = json!({ "plan": { "steps": [
        { "id": "one", "request": write_req(1.5), "compensate": write_req(1.0) },
        { "id": "two", "after": ["one"], "request": write_req(48.0), "on_failure": "compensate" }
    ] } });
    let r = call(&client, "check_plan", bad.clone()).await;
    let v = structured(&r);
    assert_eq!(v["ok"], false);
    assert_eq!(v["steps"][1]["verdict"], "refused");
    assert_eq!(
        v["steps"][1]["reasons"][0]["code"],
        "command.value.out_of_range"
    );
    let r = call(&client, "submit_plan", bad).await;
    assert!(is_error(&r), "a compensated plan did not do what was asked");
    let v = structured(&r);
    assert_eq!(v["state"], "COMPENSATED");
    assert_eq!(v["steps"][0]["status"], "COMPENSATED");
    assert_eq!(
        v["steps"][1]["reasons"][0]["code"],
        "command.value.out_of_range"
    );
    let steps = v["next_steps"].to_string();
    assert!(steps.contains("declared range"), "{steps}");

    // A malformed plan is caught with a reason the model can fix.
    let r = call(
        &client,
        "submit_plan",
        json!({ "plan": { "steps": [
            { "id": "one", "request": write_req(1.0), "on_failure": "explode" }
        ] } }),
    )
    .await;
    assert!(is_error(&r));
    assert!(text(&r).contains("plan.invalid"), "{}", text(&r));
    client.cancel().await.unwrap();
    lab.gw.shutdown().await.unwrap();
}

/// ADR-0019: an agent sees a device that stopped answering, is refused before anything is
/// sent, and is told not to retry in a loop.
#[tokio::test(flavor = "multi_thread")]
async fn an_offline_device_is_visible_and_refused_with_a_next_step() {
    let lab = lab(psu_grants()).await;
    let operator = Client::new(&lab.gw.url, &lab.gw.operator_token).unwrap();
    operator.sim_fault(PSU, "link-down").await.unwrap();
    // A status refresh probes a device at most once a second.
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    operator.device_status(Some(PSU), true).await.unwrap();
    let client = connect(&lab.gw.url, &lab.agent_token, false).await;

    let r = call(&client, "list_devices", json!({})).await;
    let devices = structured(&r)["devices"].as_array().unwrap().clone();
    let psu = devices.iter().find(|d| d["id"] == PSU).unwrap();
    assert_eq!(psu["availability"], "OFFLINE");
    assert!(
        psu["reason"]
            .as_str()
            .unwrap()
            .contains("simulated disconnect")
    );

    let r = call(
        &client,
        "submit_command",
        json!({ "device": PSU, "capability": SETPOINT, "value": volts(1.0), "idempotency_key": "off-1" }),
    )
    .await;
    assert!(is_error(&r));
    let v = structured(&r);
    assert_eq!(v["state"], "DENIED");
    assert_eq!(v["reached_device"], false);
    assert_eq!(v["reasons"][0]["code"], "device.unavailable");
    let steps = v["next_steps"][0].as_str().unwrap();
    assert!(steps.contains("Do not retry in a loop"), "{steps}");
    client.cancel().await.unwrap();
    lab.gw.shutdown().await.unwrap();
}

/// A stop that cannot be confirmed is never reported as having stopped anything, and the
/// advice is the opposite of a write's: sending it again is fine, and the equipment's own
/// emergency stop is what to use. A write in the same state is told never to resubmit.
#[tokio::test(flavor = "multi_thread")]
async fn an_unconfirmed_stop_is_not_reported_as_stopped() {
    let lab = lab(vec![
        grant("read-all", "*", "*", &[Operation::Read], Effect::Allow),
        grant(
            "axis-invoke",
            "sim.axis-01",
            "*",
            &[Operation::Invoke],
            Effect::Allow,
        ),
    ])
    .await;
    let operator = Client::new(&lab.gw.url, &lab.gw.operator_token).unwrap();
    let client = connect(&lab.gw.url, &lab.agent_token, false).await;
    // The link breaks after the gateway last saw the stage answer, so the stop is sent.
    operator
        .sim_fault("sim.axis-01", "link-down")
        .await
        .unwrap();

    let r = call(
        &client,
        "submit_command",
        json!({ "device": "sim.axis-01", "capability": "axis.stop", "params": {}, "idempotency_key": "stop-1" }),
    )
    .await;
    assert!(is_error(&r), "an unconfirmed stop is flagged as an error");
    let v = structured(&r);
    assert_eq!(v["confirmed"], false);
    assert_ne!(v["state"], "VERIFIED");
    let steps = v["next_steps"].to_string();
    assert!(steps.contains("NOT confirmed"), "{steps}");
    assert!(steps.contains("emergency stop"), "{steps}");
    assert!(steps.contains("safe"), "{steps}");
    assert!(!steps.contains("Do NOT resubmit"), "{steps}");
    client.cancel().await.unwrap();
    lab.gw.shutdown().await.unwrap();
}
