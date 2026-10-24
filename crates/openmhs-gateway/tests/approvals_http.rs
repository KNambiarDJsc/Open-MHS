//! M8: approvals and the shared door, end to end over HTTP (ADR-0017).
//!
//! The gateway is the only way to reach the engine. These tests use only what the CLI,
//! the MCP server and the SDKs have: the JSON-RPC API.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

use std::net::{Ipv4Addr, SocketAddr};

use openmhs_api::{Client, ClientError, CreateTokenParams, CreatedToken, Role, method};
use openmhs_gateway::{GatewayConfig, RunningGateway};
use openmhs_policy::{Effect, Grant};
use openmhs_types::descriptor::Operation;
use openmhs_types::{ApprovalStatus, CommandState};
use serde_json::{Value, json};

const PSU: &str = "sim.psu-01";
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

fn enable() -> Value {
    json!({
        "target": { "device": PSU, "capability": ENABLED },
        "operation": "write", "value": true
    })
}

fn code(e: ClientError) -> String {
    e.code().map_or_else(|| format!("{e}"), str::to_owned)
}

/// Ask, approve (as the operator), submit: the agent never decides its own approval.
#[tokio::test(flavor = "multi_thread")]
async fn request_approve_submit_over_the_api() {
    let lab = lab().await;
    let (agent, _) = agent(&lab, "agent:asker").await;

    let a = agent.request_approval(enable(), None).await.unwrap();
    assert_eq!(a.status, ApprovalStatus::Pending);

    // The agent cannot approve, deny or see others' approvals.
    assert_eq!(
        code(agent.approve_approval(&a.id).await.unwrap_err()),
        "gateway.forbidden"
    );
    assert_eq!(
        code(agent.deny_approval(&a.id, None).await.unwrap_err()),
        "gateway.forbidden"
    );
    let (other, _) = self::agent(&lab, "agent:other").await;
    assert_eq!(
        code(other.approval(&a.id).await.unwrap_err()),
        "approval.unknown"
    );
    assert!(other.approvals(vec![], None).await.unwrap().is_empty());
    assert_eq!(agent.approvals(vec![], None).await.unwrap().len(), 1);

    // Submitting before approval is refused, with the reason.
    let early = agent.submit_approved(enable(), &a.id).await.unwrap();
    assert_eq!(early.state, CommandState::Denied);
    assert_eq!(early.policy.errors[0].code, "approval.pending");

    let approved = lab.operator.approve_approval(&a.id).await.unwrap();
    assert_eq!(approved.status, ApprovalStatus::Approved);
    assert_eq!(approved.decided_by.as_deref(), Some("operator:local"));

    let run = agent.submit_approved(enable(), &a.id).await.unwrap();
    assert_eq!(run.state, CommandState::Verified);
    let spent = lab.operator.approval(&a.id).await.unwrap();
    assert_eq!(spent.status, ApprovalStatus::Consumed);

    // And it is spent.
    let again = agent.submit_approved(enable(), &a.id).await.unwrap();
    assert_eq!(again.state, CommandState::Denied);
    assert_eq!(again.policy.errors[0].code, "approval.consumed");
    lab.gw.shutdown().await.unwrap();
}

/// A command held by policy can be approved by its approval id as well as by command id.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_command_is_approved_through_its_approval() {
    let lab = lab().await;
    let (agent, _) = agent(&lab, "agent:held").await;
    let held = agent.submit(enable()).await.unwrap();
    assert_eq!(held.state, CommandState::AwaitingApproval);
    let pending = lab
        .operator
        .approvals(vec![ApprovalStatus::Pending], None)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].command_id.as_deref(),
        Some(held.command_id.as_str())
    );
    lab.operator.approve_approval(&pending[0].id).await.unwrap();
    let done = lab.operator.command(&held.command_id).await.unwrap();
    assert_eq!(done.state, CommandState::Verified);
    lab.gw.shutdown().await.unwrap();
}

/// Denying and revoking end the wait, and neither can be used afterwards.
#[tokio::test(flavor = "multi_thread")]
async fn deny_and_revoke_end_the_wait() {
    let lab = lab().await;
    let (agent, _) = agent(&lab, "agent:waiter").await;

    let denied = agent.submit(enable()).await.unwrap();
    let a = lab
        .operator
        .approvals(vec![ApprovalStatus::Pending], None)
        .await
        .unwrap()
        .remove(0);
    let out = lab
        .operator
        .deny_approval(&a.id, Some("not today".into()))
        .await
        .unwrap();
    assert_eq!(out.status, ApprovalStatus::Denied);
    assert_eq!(out.reason.as_deref(), Some("not today"));
    assert_eq!(
        lab.operator
            .command(&denied.command_id)
            .await
            .unwrap()
            .state,
        CommandState::Denied
    );
    assert_eq!(
        code(lab.operator.approve_approval(&a.id).await.unwrap_err()),
        "approval.denied"
    );

    // The requester can withdraw their own request.
    let b = agent.request_approval(enable(), None).await.unwrap();
    let withdrawn = agent.revoke_approval(&b.id, None).await.unwrap();
    assert_eq!(withdrawn.status, ApprovalStatus::Revoked);
    assert_eq!(
        code(lab.operator.approve_approval(&b.id).await.unwrap_err()),
        "approval.revoked"
    );
    lab.gw.shutdown().await.unwrap();
}

/// Revoking a credential withdraws what it asked for and nobody has approved yet.
#[tokio::test(flavor = "multi_thread")]
async fn revoking_a_credential_withdraws_its_open_approvals() {
    let lab = lab().await;
    let (agent, token) = agent(&lab, "agent:leaving").await;
    let held = agent.submit(enable()).await.unwrap();
    let ahead = agent.request_approval(enable(), None).await.unwrap();
    lab.operator.revoke_token(&token.info.id).await.unwrap();
    assert_eq!(
        lab.operator.command(&held.command_id).await.unwrap().state,
        CommandState::Cancelled
    );
    assert_eq!(
        lab.operator.approval(&ahead.id).await.unwrap().status,
        ApprovalStatus::Revoked
    );
    assert_eq!(
        code(lab.operator.approve_approval(&ahead.id).await.unwrap_err()),
        "approval.revoked"
    );
    lab.gw.shutdown().await.unwrap();
}

/// No public method answers without a verified credential: no token, a made-up token and
/// a wrong scheme are all refused before any method runs.
#[tokio::test(flavor = "multi_thread")]
async fn every_method_requires_authentication() {
    let lab = lab().await;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let http = reqwest::Client::new();
    let url = format!("{}/rpc", lab.gw.url);
    let methods = [
        method::GATEWAY_INFO,
        method::DEVICES_LIST,
        method::DEVICES_DESCRIBE,
        method::STATE_READ,
        method::COMMANDS_SUBMIT,
        method::COMMANDS_PREFLIGHT,
        method::COMMANDS_GET,
        method::COMMANDS_LIST,
        method::COMMANDS_APPROVE,
        method::COMMANDS_CANCEL,
        method::COMMANDS_RECONCILE,
        method::APPROVALS_REQUEST,
        method::APPROVALS_GET,
        method::APPROVALS_LIST,
        method::APPROVALS_APPROVE,
        method::APPROVALS_DENY,
        method::APPROVALS_REVOKE,
        method::GRANTS_LIST,
        method::GRANTS_ADD,
        method::GRANTS_REVOKE,
        method::TOKENS_CREATE,
        method::TOKENS_LIST,
        method::TOKENS_REVOKE,
        method::TOKENS_ROTATE,
        method::AUDIT_VERIFY,
        method::AUDIT_TAIL,
        method::SIM_FAULT,
    ];
    for m in methods {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": m, "params": {} });
        for header in [
            None,
            Some("Bearer omhs_nope"),
            Some("Basic Zm9vOmJhcg=="),
            Some("Bearer "),
        ] {
            let mut req = http.post(&url).json(&body);
            if let Some(h) = header {
                req = req.header("authorization", h);
            }
            let status = req.send().await.unwrap().status();
            assert_eq!(
                status,
                reqwest::StatusCode::UNAUTHORIZED,
                "{m} with {header:?}"
            );
        }
    }
    // Nothing was created by any of that.
    assert!(lab.gw.engine().commands(None, &[], 10).unwrap().is_empty());
    lab.gw.shutdown().await.unwrap();
}

/// Administration is for the operator role only. A holder of an agent credential reaches
/// none of it, whatever its scopes.
#[tokio::test(flavor = "multi_thread")]
async fn agent_credentials_cannot_administer() {
    let lab = lab().await;
    let (agent, _) = agent(&lab, "agent:curious").await;
    for (name, result) in [
        ("grants.list", agent.grants().await.map(|_| ())),
        (
            "grants.add",
            agent
                .add_grant(grant("mine", "*", Effect::Allow))
                .await
                .map(|_| ()),
        ),
        ("tokens.list", agent.tokens().await.map(|_| ())),
        ("audit.tail", agent.audit_tail(None).await.map(|_| ())),
        ("audit.verify", agent.verify_audit().await.map(|_| ())),
        ("sim.fault", agent.sim_fault(PSU, "hang").await.map(|_| ())),
        ("commands.approve", agent.approve("nope").await.map(|_| ())),
    ] {
        assert_eq!(code(result.unwrap_err()), "gateway.forbidden", "{name}");
    }
    lab.gw.shutdown().await.unwrap();
}
