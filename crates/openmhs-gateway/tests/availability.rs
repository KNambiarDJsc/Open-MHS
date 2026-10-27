//! Device availability through the real gateway (ADR-0019): a device that is switched off
//! does not stop the gateway or the others, status shows it to any caller, commands to it
//! are refused before anything is sent, and it is used again once it answers.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use async_trait::async_trait;
use openmhs_adapter::{
    AdapterCommand, AdapterError, AdapterErrorKind, ExecuteReport, HardwareAdapter, Health,
    Progress,
};
use openmhs_api::{Client, ClientError, CreateTokenParams, Role};
use openmhs_gateway::{GatewayConfig, RunningGateway};
use openmhs_policy::{Effect, Grant, Scope};
use openmhs_types::descriptor::Operation;
use openmhs_types::{Availability, CommandState, StateValue};
use serde_json::{Value, json};

const PSU: &str = "sim.psu-01";
const SETPOINT: &str = "power.output.voltage.setpoint";
/// A second supply on the bench that is switched off.
const OFF: &str = "lab.psu-02";

/// A supply whose port refuses every connection, as one that is switched off does.
struct SwitchedOff;

fn refused() -> AdapterError {
    AdapterError::new(AdapterErrorKind::Disconnected, "connection refused")
}

#[async_trait]
impl HardwareAdapter for SwitchedOff {
    fn adapter_id(&self) -> &str {
        "openmhs.sim.psu"
    }
    async fn describe(&self) -> Result<Value, AdapterError> {
        let mut d: Value = serde_json::from_str(include_str!(
            "../../../spec/examples/valid/power-supply.json"
        ))
        .unwrap();
        d["device"]["id"] = OFF.into();
        Ok(d)
    }
    async fn connect(&self) -> Result<(), AdapterError> {
        Err(refused())
    }
    async fn health(&self) -> Health {
        Health::Disconnected("connection refused".into())
    }
    async fn read(&self, _capability: &str) -> Result<StateValue, AdapterError> {
        Err(refused())
    }
    async fn execute(
        &self,
        _command: AdapterCommand,
        _progress: Progress,
    ) -> Result<ExecuteReport, AdapterError> {
        panic!("nothing may be sent to a device that is switched off");
    }
    async fn shutdown(&self) -> Result<(), AdapterError> {
        Ok(())
    }
}

fn config(home: &std::path::Path) -> GatewayConfig {
    let mut config = GatewayConfig::new(home);
    config.listen = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    config.simulation = true;
    config.adapters = vec![Arc::new(SwitchedOff)];
    config
}

async fn agent(gw: &RunningGateway, operator: &Client) -> Client {
    operator
        .add_grant(Grant {
            id: "agents-supplies".into(),
            principal: "agent:*".into(),
            device: "*psu*".into(),
            capability: "*".into(),
            operations: vec![Operation::Read, Operation::Write],
            effect: Effect::Allow,
            max_side_effect: None,
            reason: None,
        })
        .await
        .unwrap();
    let t = operator
        .create_token(CreateTokenParams::scoped(
            "agent:bench",
            Role::Agent,
            vec![Scope {
                device: "*".into(),
                capability: "*".into(),
                operations: vec![Operation::Read, Operation::Write],
            }],
        ))
        .await
        .unwrap();
    Client::new(&gw.url, &t.token).unwrap()
}

fn set_voltage(device: &str, volts: f64) -> Value {
    json!({
        "target": { "device": device, "capability": SETPOINT },
        "operation": "write",
        "value": { "value": volts, "unit": "V" }
    })
}

async fn availability(client: &Client, device: &str) -> Availability {
    client.device_status(Some(device), true).await.unwrap()[0].availability
}

fn code(e: ClientError) -> String {
    e.code().unwrap_or_default().to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_switched_off_device_does_not_stop_the_gateway_or_the_others() {
    let home = tempfile::tempdir().unwrap();
    let gw = openmhs_gateway::start(config(home.path())).await.unwrap();
    let operator = Client::new(&gw.url, &gw.operator_token).unwrap();
    let agent = agent(&gw, &operator).await;

    // Any caller sees every device's status, as it sees every device.
    let all = agent.device_status(None, false).await.unwrap();
    let off = all.iter().find(|s| s.device == OFF).unwrap();
    assert_eq!(off.availability, Availability::Offline);
    assert_eq!(off.reason.as_deref(), Some("connection refused"));
    assert!(
        all.iter()
            .any(|s| s.device == PSU && s.availability == Availability::Online)
    );

    // The switched-off supply is refused before anything is sent (its execute panics).
    let r = agent.submit(set_voltage(OFF, 1.0)).await.unwrap();
    assert_eq!(r.state, CommandState::Denied, "{r:?}");
    assert_eq!(r.policy.errors[0].code, "device.unavailable");
    // The other supply works.
    let r = agent.submit(set_voltage(PSU, 1.0)).await.unwrap();
    assert_eq!(r.state, CommandState::Verified, "{r:?}");

    assert_eq!(
        code(
            agent
                .device_status(Some("lab.nothing"), false)
                .await
                .unwrap_err()
        ),
        "command.device.unknown"
    );
    let audit = operator.audit_tail(Some(500)).await.unwrap();
    assert!(
        audit
            .iter()
            .any(|a| a.kind == "device.registered" && a.subject == OFF)
    );
    assert!(operator.verify_audit().await.unwrap().valid);
    gw.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_that_goes_away_and_comes_back_is_followed() {
    let home = tempfile::tempdir().unwrap();
    let gw = openmhs_gateway::start(config(home.path())).await.unwrap();
    let operator = Client::new(&gw.url, &gw.operator_token).unwrap();
    let agent = agent(&gw, &operator).await;

    operator.sim_fault(PSU, "link-down").await.unwrap();
    // A refresh probes a device at most once a second; it was checked at registration.
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    assert_eq!(availability(&agent, PSU).await, Availability::Offline);
    let r = agent.submit(set_voltage(PSU, 1.0)).await.unwrap();
    assert_eq!(r.policy.errors[0].code, "device.unavailable", "{r:?}");

    operator.sim_fault(PSU, "clear").await.unwrap();
    // A command does not wait for the next probe: it checks again itself.
    let r = agent.submit(set_voltage(PSU, 1.0)).await.unwrap();
    assert_eq!(r.state, CommandState::Verified, "{r:?}");
    assert_eq!(availability(&agent, PSU).await, Availability::Online);

    let changes: Vec<_> = operator
        .audit_tail(Some(500))
        .await
        .unwrap()
        .into_iter()
        .filter(|a| a.kind == "device.availability" && a.subject == PSU)
        .collect();
    assert_eq!(changes.len(), 2, "{changes:?}");
    gw.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn require_devices_keeps_the_strict_start() {
    let home = tempfile::tempdir().unwrap();
    let mut strict = config(home.path());
    strict.require_devices = true;
    assert!(openmhs_gateway::start(strict).await.is_err());
}
