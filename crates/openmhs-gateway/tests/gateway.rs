//! In-process gateway tests for behaviour the CLI tests can't easily observe.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use openmhs_api::{Client, Role};
use openmhs_gateway::GatewayConfig;
use openmhs_policy::{Effect, Grant};
use openmhs_types::CommandState;
use openmhs_types::descriptor::Operation;
use serde_json::json;

async fn start(home: &std::path::Path) -> openmhs_gateway::RunningGateway {
    let mut config = GatewayConfig::new(home);
    config.listen = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    config.simulation = true;
    openmhs_gateway::start(config).await.unwrap()
}

fn psu_write_grant() -> Grant {
    Grant {
        id: "psu".into(),
        principal: "agent:*".into(),
        device: "sim.psu-01".into(),
        capability: "*".into(),
        operations: vec![Operation::Read, Operation::Write],
        effect: Effect::Allow,
        max_side_effect: None,
        reason: None,
    }
}

/// A client that gives up mid-request must not cancel the command half-way through its
/// lifecycle: it still runs to a defined state (here UNKNOWN_OUTCOME at its deadline),
/// not stuck in DISPATCHED until the next restart.
#[tokio::test(flavor = "multi_thread")]
async fn client_disconnect_does_not_abandon_the_command() {
    let home = tempfile::tempdir().unwrap();
    let gw = start(home.path()).await;
    let operator = Client::new(&gw.url, &gw.operator_token).unwrap();
    operator.add_grant(psu_write_grant()).await.unwrap();
    let agent = operator.create_token("agent:t", Role::Agent).await.unwrap();
    operator.sim_fault("sim.psu-01", "hang").await.unwrap();

    let impatient = reqwest::Client::builder()
        .timeout(Duration::from_millis(300))
        .build()
        .unwrap();
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "commands.submit",
        "params": { "request": {
            "target": { "device": "sim.psu-01", "capability": "power.output.voltage.setpoint" },
            "operation": "write", "value": { "value": 1, "unit": "V" }, "deadline_ms": 1000
        } }
    });
    let sent = impatient
        .post(format!("{}/rpc", gw.url))
        .bearer_auth(&agent.token)
        .json(&body)
        .send()
        .await;
    assert!(sent.is_err(), "the client should have timed out");

    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let commands = gw.engine().commands(None, &[], 10).unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].state, CommandState::UnknownOutcome);
    gw.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_need_a_token_and_json() {
    let home = tempfile::tempdir().unwrap();
    let gw = start(home.path()).await;
    let http = reqwest::Client::new();
    let url = format!("{}/rpc", gw.url);
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "gateway.info" });

    let no_token = http.post(&url).json(&body).send().await.unwrap();
    assert_eq!(no_token.status(), reqwest::StatusCode::UNAUTHORIZED);

    // A browser form post (no JSON content type) is refused before authentication.
    let form = http
        .post(&url)
        .bearer_auth(&gw.operator_token)
        .header("content-type", "text/plain")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(form.status(), reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let ok = http
        .post(&url)
        .bearer_auth(&gw.operator_token)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), reqwest::StatusCode::OK);
    gw.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_removes_the_runtime_file_and_releases_the_home() {
    let home = tempfile::tempdir().unwrap();
    let gw = start(home.path()).await;
    let runtime = home.path().join("gateway.json");
    assert!(runtime.exists());
    gw.shutdown().await.unwrap();
    assert!(!runtime.exists());
    // The lock is released: a new gateway can start on the same home.
    start(home.path()).await.shutdown().await.unwrap();
}

/// The stable code travels in `data.code`; the message is for people and does not repeat
/// it (clients that print `[code] message` would otherwise show it twice).
#[tokio::test(flavor = "multi_thread")]
async fn denial_messages_do_not_repeat_the_code() {
    let home = tempfile::tempdir().unwrap();
    let gw = start(home.path()).await;
    let operator = Client::new(&gw.url, &gw.operator_token).unwrap();
    // Deny by default applies to operators too: no grant, no read.
    let err = operator
        .read("sim.psu-01", "power.output.voltage.setpoint")
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("policy.denied.no_grant"));
    assert!(!err.to_string().contains("policy.denied"), "{err}");
    gw.shutdown().await.unwrap();
}

/// A program embedding the gateway registers its own adapters; they are served like any
/// other device, and simulator fault injection stays off without `simulation`.
#[tokio::test(flavor = "multi_thread")]
async fn embedded_adapters_are_registered() {
    let home = tempfile::tempdir().unwrap();
    let clock: std::sync::Arc<dyn openmhs_types::Clock> =
        std::sync::Arc::new(openmhs_types::SystemClock);
    let (psu, _control) = openmhs_sim::VirtualPsu::new(clock, 7);
    let mut config = GatewayConfig::new(home.path());
    config.listen = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    config.adapters.push(std::sync::Arc::new(psu));
    let gw = openmhs_gateway::start(config).await.unwrap();
    let operator = Client::new(&gw.url, &gw.operator_token).unwrap();
    let ids: Vec<String> = operator
        .devices()
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.device.id)
        .collect();
    assert_eq!(ids, ["sim.psu-01"]);
    let err = operator.sim_fault("sim.psu-01", "hang").await.unwrap_err();
    assert_eq!(err.code(), Some("gateway.not_simulation"));
    gw.shutdown().await.unwrap();
}
