//! M8: scoped credentials through the real gateway (spec/authorization.md §7).
//!
//! Every test talks to a running gateway over HTTP, the way the CLI, MCP server and SDKs
//! do. Nothing here calls the engine directly, so a check that passes here is a check the
//! public interfaces cannot skip.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use openmhs_api::{Client, ClientError, CreateTokenParams, CreatedToken, Role};
use openmhs_gateway::{GatewayConfig, RunningGateway};
use openmhs_policy::{Effect, Grant, Scope};
use openmhs_types::descriptor::Operation;
use serde_json::{Value, json};

const PSU: &str = "sim.psu-01";
const AXIS: &str = "sim.axis-01";
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
    // Policy is generous on purpose: what these tests probe is the credential's ceiling.
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

fn scope(device: &str, capability: &str, ops: &[Operation]) -> Scope {
    Scope {
        device: device.into(),
        capability: capability.into(),
        operations: ops.to_vec(),
    }
}

async fn issue(
    lab: &Lab,
    principal: &str,
    scopes: Vec<Scope>,
    ttl_ms: Option<u64>,
) -> CreatedToken {
    lab.operator
        .create_token(CreateTokenParams {
            principal: principal.into(),
            role: Role::Agent,
            scopes,
            ttl_ms,
        })
        .await
        .unwrap()
}

fn client(lab: &Lab, token: &str) -> Client {
    Client::new(&lab.gw.url, token).unwrap()
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

/// 1. A valid credential reads what its scope and the policy allow.
#[tokio::test(flavor = "multi_thread")]
async fn a_valid_scoped_credential_can_read() {
    let lab = lab().await;
    let t = issue(
        &lab,
        "agent:reader",
        vec![scope(PSU, "*", &[Operation::Read])],
        None,
    )
    .await;
    let r = client(&lab, &t.token).read(PSU, SETPOINT).await.unwrap();
    assert_eq!(r.device, PSU);
    lab.gw.shutdown().await.unwrap();
}

/// 2. No token and a made-up token are both refused, and nothing is said about which.
#[tokio::test(flavor = "multi_thread")]
async fn missing_and_invalid_credentials_are_rejected() {
    let lab = lab().await;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let http = reqwest::Client::new();
    let url = format!("{}/rpc", lab.gw.url);
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "gateway.info" });
    let none = http.post(&url).json(&body).send().await.unwrap();
    assert_eq!(none.status(), reqwest::StatusCode::UNAUTHORIZED);
    let wrong = client(
        &lab,
        "omhs_0000000000000000000000000000000000000000000000000000000000000000",
    );
    assert!(matches!(wrong.info().await, Err(ClientError::Unauthorized)));
    let garbage = http
        .post(&url)
        .header("authorization", "Basic abc")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(garbage.status(), reqwest::StatusCode::UNAUTHORIZED);
    lab.gw.shutdown().await.unwrap();
}

/// 3. An expired credential stops working at its expiry, for every method.
#[tokio::test(flavor = "multi_thread")]
async fn an_expired_credential_is_rejected() {
    let lab = lab().await;
    // Wide enough that a slow CI runner cannot let it lapse before the first read (a 400 ms
    // lifetime did, on Windows); still short enough to keep the test quick.
    let t = issue(&lab, "agent:short", vec![Scope::any()], Some(3000)).await;
    let c = client(&lab, &t.token);
    c.read(PSU, SETPOINT).await.unwrap();
    tokio::time::sleep(Duration::from_millis(3200)).await;
    let e = c.read(PSU, SETPOINT).await.unwrap_err();
    assert_eq!(code(e), "credential.expired");
    let e = c.submit(set_voltage(1.0)).await.unwrap_err();
    assert_eq!(code(e), "credential.expired");
    // The refusal is audited once, without the secret.
    let tail = lab.operator.audit_tail(Some(50)).await.unwrap();
    let rejected: Vec<_> = tail
        .iter()
        .filter(|a| a.kind == "credential.rejected")
        .collect();
    assert_eq!(rejected.len(), 1, "one record however many retries");
    assert!(!serde_json::to_string(&tail).unwrap().contains(&t.token));
    lab.gw.shutdown().await.unwrap();
}

/// 4. Revocation takes effect on the very next request.
#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_credential_is_rejected_immediately() {
    let lab = lab().await;
    let t = issue(&lab, "agent:gone", vec![Scope::any()], None).await;
    let c = client(&lab, &t.token);
    c.read(PSU, SETPOINT).await.unwrap();
    lab.operator.revoke_token(&t.info.id).await.unwrap();
    assert_eq!(
        code(c.read(PSU, SETPOINT).await.unwrap_err()),
        "credential.revoked"
    );
    assert_eq!(
        code(c.submit(set_voltage(1.0)).await.unwrap_err()),
        "credential.revoked"
    );
    // It stays on record, marked revoked, and revoking again changes nothing.
    let listed = lab.operator.tokens().await.unwrap();
    let rec = listed.iter().find(|i| i.id == t.info.id).unwrap();
    assert!(rec.revoked_at.is_some());
    lab.operator.revoke_token(&t.info.id).await.unwrap();
    lab.gw.shutdown().await.unwrap();
}

/// 5. A scope for one device does not reach another, even where policy allows both.
#[tokio::test(flavor = "multi_thread")]
async fn a_credential_cannot_reach_a_device_outside_its_scope() {
    let lab = lab().await;
    let t = issue(
        &lab,
        "agent:psu-only",
        vec![scope(PSU, "*", &[Operation::Read, Operation::Write])],
        None,
    )
    .await;
    let c = client(&lab, &t.token);
    c.read(PSU, SETPOINT).await.unwrap();
    let e = c.read(AXIS, "motion.position").await.unwrap_err();
    assert_eq!(code(e), "auth.scope.denied");
    // The refusal is the same for a device that does not exist, so scope cannot probe.
    let e = c.read("sim.nothing-99", "x").await.unwrap_err();
    assert_eq!(code(e), "auth.scope.denied");
    lab.gw.shutdown().await.unwrap();
}

/// 6. A read-only scope cannot write, and a capability outside the scope is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_credential_cannot_exceed_its_operations_or_capabilities() {
    let lab = lab().await;
    let t = issue(
        &lab,
        "agent:ro",
        vec![scope(PSU, "power.output.*", &[Operation::Read])],
        None,
    )
    .await;
    let c = client(&lab, &t.token);
    c.read(PSU, SETPOINT).await.unwrap();
    // Wrong operation: the policy allows writes, the credential does not.
    let r = c.submit(set_voltage(1.0)).await.unwrap();
    assert_eq!(r.policy.errors[0].code, "auth.scope.denied");
    assert_eq!(r.state, openmhs_types::CommandState::Denied);
    // Wrong capability on the right device.
    let e = c.read(PSU, "protection.tripped").await.unwrap_err();
    assert_eq!(code(e), "auth.scope.denied");
    // Preflight honors the same ceiling, so it cannot be used to find out what is possible.
    let p = c.preflight(set_voltage(1.0)).await.unwrap();
    assert_eq!(p.errors[0].code, "auth.scope.denied");
    lab.gw.shutdown().await.unwrap();
}

/// 7. A wide scope is only a ceiling: without a grant the call is still denied.
#[tokio::test(flavor = "multi_thread")]
async fn a_scope_never_replaces_a_grant() {
    let lab = lab().await;
    // `service:*` has no grant, however wide its credential.
    let t = lab
        .operator
        .create_token(CreateTokenParams::unrestricted(
            "service:scheduler",
            Role::Agent,
        ))
        .await
        .unwrap();
    let c = client(&lab, &t.token);
    let e = c.read(PSU, SETPOINT).await.unwrap_err();
    assert_eq!(code(e), "policy.denied.no_grant");
    lab.gw.shutdown().await.unwrap();
}

/// 8. A credential issued with no scopes can do nothing but identify itself.
#[tokio::test(flavor = "multi_thread")]
async fn a_credential_without_scopes_authorizes_nothing() {
    let lab = lab().await;
    let t = issue(&lab, "agent:empty", vec![], None).await;
    let c = client(&lab, &t.token);
    assert_eq!(c.info().await.unwrap().principal, "agent:empty");
    assert_eq!(
        code(c.read(PSU, SETPOINT).await.unwrap_err()),
        "auth.scope.denied"
    );
    let r = c.submit(set_voltage(1.0)).await.unwrap();
    assert_eq!(r.state, openmhs_types::CommandState::Denied);
    lab.gw.shutdown().await.unwrap();
}

/// 9. Identity comes from the credential alone. A principal named in the request body is
///    ignored, and an agent cannot issue itself credentials or widen its own.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_cannot_claim_another_identity_or_issue_credentials() {
    let lab = lab().await;
    let t = issue(
        &lab,
        "agent:honest",
        vec![scope(PSU, "*", &[Operation::Read, Operation::Write])],
        None,
    )
    .await;
    let c = client(&lab, &t.token);

    // A principal smuggled into the request body or the params is not read.
    let mut request = set_voltage(1.0);
    request["principal"] = json!("operator:local");
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "commands.submit",
        "params": { "request": request, "principal": "operator:local", "caller": { "principal": "operator:local" } }
    });
    let http = reqwest::Client::new();
    let resp: Value = http
        .post(format!("{}/rpc", lab.gw.url))
        .bearer_auth(&t.token)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // The schema refuses the extra field; the point is that it never becomes an identity.
    assert!(resp["error"].is_object(), "{resp}");
    let clean = c.submit(set_voltage(1.0)).await.unwrap();
    assert_eq!(clean.principal, "agent:honest");

    for attempt in [
        c.create_token(CreateTokenParams::unrestricted("agent:honest", Role::Agent))
            .await
            .map(|_| ()),
        c.rotate_token(&t.info.id, None).await.map(|_| ()),
        c.revoke_token(&t.info.id).await.map(|_| ()),
        c.tokens().await.map(|_| ()),
    ] {
        assert_eq!(code(attempt.unwrap_err()), "gateway.forbidden");
    }
    lab.gw.shutdown().await.unwrap();
}

/// 10. Credentials cannot be issued for another identity's name or role.
#[tokio::test(flavor = "multi_thread")]
async fn issuing_refuses_impersonation_of_the_operator_and_role_mixing() {
    let lab = lab().await;
    let create = |principal: &str, role: Role| {
        lab.operator
            .create_token(CreateTokenParams::unrestricted(principal, role))
    };
    assert_eq!(
        code(create("operator:local", Role::Operator).await.unwrap_err()),
        "credential.principal.reserved"
    );
    assert_eq!(
        code(create("operator:local", Role::Agent).await.unwrap_err()),
        "credential.principal.reserved"
    );
    assert_eq!(
        code(create("agent:evil", Role::Operator).await.unwrap_err()),
        "credential.role.invalid"
    );
    create("alice", Role::Agent).await.unwrap();
    assert_eq!(
        code(create("alice", Role::Operator).await.unwrap_err()),
        "credential.role.conflict"
    );
    lab.gw.shutdown().await.unwrap();
}

/// 11. Rotation: the new secret works, the old one is revoked at once, scopes carry over.
#[tokio::test(flavor = "multi_thread")]
async fn rotation_replaces_a_credential_without_widening_it() {
    let lab = lab().await;
    let old = issue(
        &lab,
        "agent:rot",
        vec![scope(PSU, "*", &[Operation::Read])],
        None,
    )
    .await;
    let fresh = lab.operator.rotate_token(&old.info.id, None).await.unwrap();
    assert_ne!(fresh.token, old.token);
    assert_eq!(fresh.info.scopes, old.info.scopes);
    assert_eq!(
        code(client(&lab, &old.token).info().await.unwrap_err()),
        "credential.revoked"
    );
    let c = client(&lab, &fresh.token);
    c.read(PSU, SETPOINT).await.unwrap();
    assert_eq!(
        code(c.read(AXIS, "motion.position").await.unwrap_err()),
        "auth.scope.denied"
    );
    lab.gw.shutdown().await.unwrap();
}

/// 12. Secrets stay out of the stored files, the listing, the audit log and Debug output.
#[tokio::test(flavor = "multi_thread")]
async fn secrets_are_never_stored_or_logged() {
    let lab = lab().await;
    let t = issue(&lab, "agent:quiet", vec![Scope::any()], None).await;
    let _ = client(&lab, &t.token).read(PSU, SETPOINT).await;
    let on_disk = std::fs::read_to_string(lab.gw.home().join("tokens.json")).unwrap();
    assert!(!on_disk.contains(&t.token));
    let listing = serde_json::to_string(&lab.operator.tokens().await.unwrap()).unwrap();
    assert!(!listing.contains(&t.token) && !listing.contains("sha256"));
    let audit = serde_json::to_string(&lab.operator.audit_tail(Some(200)).await.unwrap()).unwrap();
    assert!(!audit.contains(&t.token));
    assert!(
        !format!("{t:?}").contains(&t.token),
        "Debug redacts the secret"
    );
    // The operator's runtime file is the one place the operator token lives.
    let runtime = openmhs_api::RuntimeInfo::load(lab.gw.home()).unwrap();
    assert!(!format!("{runtime:?}").contains(&runtime.token));
    assert!(!audit.contains(&lab.gw.operator_token));
    lab.gw.shutdown().await.unwrap();
}

/// 13. The audit trail names the credential that made each request.
#[tokio::test(flavor = "multi_thread")]
async fn commands_are_audited_with_their_credential() {
    let lab = lab().await;
    let t = issue(
        &lab,
        "agent:traced",
        vec![scope(PSU, "*", &[Operation::Read, Operation::Write])],
        None,
    )
    .await;
    let r = client(&lab, &t.token)
        .submit(set_voltage(1.0))
        .await
        .unwrap();
    let tail = lab.operator.audit_tail(Some(100)).await.unwrap();
    let created = tail
        .iter()
        .find(|a| a.kind == "command.created" && a.subject == r.command_id)
        .unwrap();
    assert_eq!(created.data["origin"]["credential_id"], json!(t.info.id));
    assert_eq!(created.data["origin"]["auth_method"], json!("credential"));
    lab.gw.shutdown().await.unwrap();
}
