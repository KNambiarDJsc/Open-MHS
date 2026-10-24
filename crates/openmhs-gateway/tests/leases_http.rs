//! M9: leases through the real gateway (spec/topology.md, ADR-0018). Holders are the
//! principals behind credentials; revoking or rotating a credential frees its leases.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

use std::net::{Ipv4Addr, SocketAddr};

use openmhs_api::{AcquireLeaseParams, Client, ClientError, CreateTokenParams, Role};
use openmhs_gateway::{GatewayConfig, RunningGateway};
use openmhs_policy::{Effect, Grant, Scope};
use openmhs_types::CommandState;
use openmhs_types::descriptor::Operation;
use serde_json::{Value, json};

const PSU: &str = "sim.psu-01";
const SETPOINT: &str = "power.output.voltage.setpoint";

struct Lab {
    gw: RunningGateway,
    operator: Client,
    _home: tempfile::TempDir,
}

async fn lab() -> Lab {
    let home = tempfile::tempdir().unwrap();
    let mut config = GatewayConfig::new(home.path());
    config.listen = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    config.simulation = true;
    let gw = openmhs_gateway::start(config).await.unwrap();
    let operator = Client::new(&gw.url, &gw.operator_token).unwrap();
    operator
        .add_grant(Grant {
            id: "agents-everything".into(),
            principal: "agent:*".into(),
            device: "*".into(),
            capability: "*".into(),
            operations: vec![Operation::Read, Operation::Write],
            effect: Effect::Allow,
            max_side_effect: None,
            reason: None,
        })
        .await
        .unwrap();
    Lab {
        gw,
        operator,
        _home: home,
    }
}

async fn agent(lab: &Lab, principal: &str) -> (Client, String) {
    let t = lab
        .operator
        .create_token(CreateTokenParams::scoped(
            principal,
            Role::Agent,
            vec![Scope {
                device: "*".into(),
                capability: "*".into(),
                operations: vec![Operation::Read, Operation::Write],
            }],
        ))
        .await
        .unwrap();
    (Client::new(&lab.gw.url, &t.token).unwrap(), t.info.id)
}

fn psu_lease() -> AcquireLeaseParams {
    AcquireLeaseParams {
        devices: vec![PSU.into()],
        ttl_ms: 600_000,
        reason: Some("calibration".into()),
        ..AcquireLeaseParams::default()
    }
}

fn set_voltage(volts: f64) -> Value {
    json!({
        "target": { "device": PSU, "capability": SETPOINT },
        "operation": "write",
        "value": { "value": volts, "unit": "V" }
    })
}

fn code(e: ClientError) -> String {
    e.code().map_or_else(|| format!("{e}"), str::to_owned)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_over_http_keeps_other_agents_off_the_device() {
    let lab = lab().await;
    let (a, _) = agent(&lab, "agent:a").await;
    let (b, _) = agent(&lab, "agent:b").await;
    let l = a.acquire_lease(psu_lease()).await.unwrap();
    assert_eq!(l.holder, "agent:a");

    let refused = b.submit(set_voltage(1.0)).await.unwrap();
    assert_eq!(refused.state, CommandState::Denied);
    assert_eq!(refused.policy.errors[0].code, "lease.held");
    let ran = a.submit(set_voltage(1.0)).await.unwrap();
    assert_eq!(ran.state, CommandState::Verified, "{ran:?}");

    // Everyone sees who holds what; only the holder renews or releases.
    assert_eq!(b.lease(&l.id).await.unwrap().holder, "agent:a");
    assert_eq!(b.leases(true, None).await.unwrap().len(), 1);
    assert_eq!(
        code(b.acquire_lease(psu_lease()).await.unwrap_err()),
        "lease.conflict"
    );
    assert_eq!(
        code(b.release_lease(&l.id, None).await.unwrap_err()),
        "lease.not_holder"
    );
    let renewed = a.renew_lease(&l.id, 900_000).await.unwrap();
    assert!(renewed.expires_at > l.expires_at);
    let released = a.release_lease(&l.id, Some("done".into())).await.unwrap();
    assert_eq!(released.released_by.as_deref(), Some("agent:a"));
    assert!(b.leases(true, None).await.unwrap().is_empty());
    let after = b.submit(set_voltage(1.5)).await.unwrap();
    assert_eq!(after.state, CommandState::Verified, "{after:?}");

    let kinds: Vec<String> = lab
        .gw
        .engine()
        .journal()
        .audit_records()
        .unwrap()
        .into_iter()
        .map(|r| r.kind)
        .collect();
    for kind in ["lease.acquired", "lease.renewed", "lease.released"] {
        assert!(kinds.iter().any(|k| k == kind), "no {kind} in {kinds:?}");
    }
    lab.gw.shutdown().await.unwrap();
}

/// The operator frees a device an agent left leased.
#[tokio::test(flavor = "multi_thread")]
async fn the_operator_can_release_any_lease() {
    let lab = lab().await;
    let (a, _) = agent(&lab, "agent:a").await;
    let l = a.acquire_lease(psu_lease()).await.unwrap();
    let freed = lab
        .operator
        .release_lease(&l.id, Some("agent went away".into()))
        .await
        .unwrap();
    assert!(freed.released_at.is_some());
    assert_eq!(
        code(a.renew_lease(&l.id, 1_000).await.unwrap_err()),
        "lease.ended"
    );
    lab.gw.shutdown().await.unwrap();
}

/// Revoking or rotating a credential frees what it leased, as it withdraws its approvals.
#[tokio::test(flavor = "multi_thread")]
async fn revoking_or_rotating_a_credential_releases_its_leases() {
    let lab = lab().await;
    let (b, _) = agent(&lab, "agent:b").await;

    let (a, a_cred) = agent(&lab, "agent:a").await;
    let l = a.acquire_lease(psu_lease()).await.unwrap();
    assert_eq!(l.holder_credential.as_deref(), Some(a_cred.as_str()));
    lab.operator.revoke_token(&a_cred).await.unwrap();
    assert!(
        lab.operator
            .lease(&l.id)
            .await
            .unwrap()
            .released_at
            .is_some()
    );
    assert_eq!(
        b.submit(set_voltage(1.0)).await.unwrap().state,
        CommandState::Verified
    );

    let (c, c_cred) = agent(&lab, "agent:c").await;
    let l = c.acquire_lease(psu_lease()).await.unwrap();
    lab.operator.rotate_token(&c_cred, None).await.unwrap();
    assert!(
        lab.operator
            .lease(&l.id)
            .await
            .unwrap()
            .released_at
            .is_some()
    );
    assert_eq!(
        b.submit(set_voltage(1.5)).await.unwrap().state,
        CommandState::Verified
    );
    lab.gw.shutdown().await.unwrap();
}

/// A credential that can only read cannot lease: leases are not a way to lock others out.
#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_credential_cannot_lease() {
    let lab = lab().await;
    let t = lab
        .operator
        .create_token(CreateTokenParams::scoped(
            "agent:reader",
            Role::Agent,
            vec![Scope {
                device: "*".into(),
                capability: "*".into(),
                operations: vec![Operation::Read],
            }],
        ))
        .await
        .unwrap();
    let reader = Client::new(&lab.gw.url, &t.token).unwrap();
    assert_eq!(
        code(reader.acquire_lease(psu_lease()).await.unwrap_err()),
        "lease.not_authorized"
    );
    lab.gw.shutdown().await.unwrap();
}
