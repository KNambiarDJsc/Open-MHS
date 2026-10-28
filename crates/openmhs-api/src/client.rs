//! Async client for the gateway API.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use openmhs_policy::Grant;
use openmhs_types::receipt::PreflightResult;
use openmhs_types::{Approval, ApprovalStatus, CommandState, Descriptor, Receipt};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::runtime::RuntimeInfo;
use crate::*;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("no running gateway found ({0}); start one with `openmhs serve`")]
    NotRunning(String),
    #[error("cannot reach the gateway at {url}: {message}")]
    Connect { url: String, message: String },
    #[error("unauthorized: the token was rejected")]
    Unauthorized,
    /// The gateway answered with an error.
    #[error("{}", .0.message)]
    Rpc(RpcError),
    #[error("unexpected response: {0}")]
    Protocol(String),
}

impl ClientError {
    /// Stable OpenMHS error code, when the gateway sent one.
    pub fn code(&self) -> Option<&str> {
        match self {
            ClientError::Rpc(e) => e.data.as_ref().map(|d| d.code.as_str()),
            _ => None,
        }
    }
}

pub struct Client {
    http: reqwest::Client,
    url: String,
    token: String,
    next_id: AtomicU64,
}

impl Client {
    pub fn new(base_url: &str, token: &str) -> Result<Self, ClientError> {
        // reqwest needs a rustls provider to build any client; ring is OpenMHS's
        // (docs/security/transport-security.md). A no-op if one is installed already.
        let _ = rustls::crypto::ring::default_provider().install_default();
        // No overall timeout: a submit waits for the command's own deadline and
        // verification, which the gateway enforces.
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        Ok(Self {
            http,
            url: format!("{}{RPC_PATH}", base_url.trim_end_matches('/')),
            token: token.to_owned(),
            next_id: AtomicU64::new(1),
        })
    }

    /// Connect using the runtime file in `home`, optionally with another token.
    pub fn from_home(home: &Path, token: Option<&str>) -> Result<Self, ClientError> {
        let info = RuntimeInfo::load(home)
            .map_err(|e| ClientError::NotRunning(format!("{}: {e}", home.display())))?;
        Self::new(&info.url, token.unwrap_or(&info.token))
    }

    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
    ) -> Result<R, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = RpcRequest {
            jsonrpc: "2.0".into(),
            id: id.into(),
            method: method.into(),
            params: serde_json::to_value(params)
                .map_err(|e| ClientError::Protocol(e.to_string()))?,
        };
        let response = self
            .http
            .post(&self.url)
            .bearer_auth(&self.token)
            .json(&request)
            .send()
            .await
            .map_err(|e| ClientError::Connect {
                url: self.url.clone(),
                message: e.to_string(),
            })?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            // An expired or revoked credential says so; anything else is just rejected.
            let specific = response
                .json::<RpcResponse>()
                .await
                .ok()
                .and_then(|b| b.error)
                .filter(|e| {
                    matches!(
                        e.data.as_ref().map(|d| d.code.as_str()),
                        Some("credential.expired" | "credential.revoked")
                    )
                });
            return Err(specific.map_or(ClientError::Unauthorized, ClientError::Rpc));
        }
        let body: RpcResponse = response
            .json()
            .await
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        if let Some(error) = body.error {
            return Err(ClientError::Rpc(error));
        }
        let result = body
            .result
            .ok_or_else(|| ClientError::Protocol("response has neither result nor error".into()))?;
        serde_json::from_value(result).map_err(|e| ClientError::Protocol(e.to_string()))
    }

    pub async fn info(&self) -> Result<GatewayInfo, ClientError> {
        self.call(method::GATEWAY_INFO, serde_json::json!({})).await
    }

    pub async fn devices(&self) -> Result<Vec<Descriptor>, ClientError> {
        self.call(method::DEVICES_LIST, serde_json::json!({})).await
    }

    /// Whether devices answer, are busy or leased (ADR-0019). `device` narrows it to one;
    /// `refresh` probes devices not checked in the last second first.
    /// Events after `after` (ADR-0023). Pass the previous batch's `stream` and `next` to
    /// continue; `wait_ms` waits for one when there are none.
    pub async fn poll_events(
        &self,
        params: crate::EventsPollParams,
    ) -> Result<openmhs_types::event::EventBatch, ClientError> {
        self.call(method::EVENTS_POLL, params).await
    }

    pub async fn create_watch(
        &self,
        params: crate::WatchParams,
    ) -> Result<openmhs_types::event::Watch, ClientError> {
        self.call(method::WATCHES_CREATE, params).await
    }

    pub async fn watches(&self) -> Result<Vec<openmhs_types::event::Watch>, ClientError> {
        self.call(method::WATCHES_LIST, serde_json::json!({})).await
    }

    pub async fn cancel_watch(
        &self,
        watch_id: &str,
    ) -> Result<openmhs_types::event::Watch, ClientError> {
        self.call(
            method::WATCHES_CANCEL,
            crate::IdParams {
                id: watch_id.to_owned(),
            },
        )
        .await
    }

    pub async fn procedures(
        &self,
    ) -> Result<Vec<openmhs_types::procedure::Procedure>, ClientError> {
        self.call(method::PROCEDURES_LIST, serde_json::json!({}))
            .await
    }

    pub async fn procedure(
        &self,
        name: &str,
        version: Option<u32>,
    ) -> Result<openmhs_types::procedure::Procedure, ClientError> {
        let r = crate::ProcedureRef {
            name: name.to_owned(),
            version,
        };
        self.call(method::PROCEDURES_GET, r).await
    }

    /// Store a procedure as proposed; it cannot run until an operator approves it.
    pub async fn propose_procedure(
        &self,
        procedure: openmhs_types::procedure::ProcedureSpec,
    ) -> Result<openmhs_types::procedure::Procedure, ClientError> {
        self.call(
            method::PROCEDURES_PROPOSE,
            crate::ProcedureParams { procedure },
        )
        .await
    }

    /// Store a procedure as approved (operator).
    pub async fn add_procedure(
        &self,
        procedure: openmhs_types::procedure::ProcedureSpec,
    ) -> Result<openmhs_types::procedure::Procedure, ClientError> {
        self.call(method::PROCEDURES_ADD, crate::ProcedureParams { procedure })
            .await
    }

    pub async fn approve_procedure(
        &self,
        name: &str,
        version: u32,
    ) -> Result<openmhs_types::procedure::Procedure, ClientError> {
        let r = crate::ProcedureRef {
            name: name.to_owned(),
            version: Some(version),
        };
        self.call(method::PROCEDURES_APPROVE, r).await
    }

    pub async fn remove_procedure(
        &self,
        name: &str,
        version: u32,
    ) -> Result<openmhs_types::procedure::Procedure, ClientError> {
        let r = crate::ProcedureRef {
            name: name.to_owned(),
            version: Some(version),
        };
        self.call(method::PROCEDURES_REMOVE, r).await
    }

    /// Dry-run an approved procedure with `params`, like `plans.preflight`.
    pub async fn check_procedure(
        &self,
        params: crate::RunProcedureParams,
    ) -> Result<openmhs_types::plan::PlanPreflight, ClientError> {
        self.call(method::PROCEDURES_CHECK, params).await
    }

    /// Run an approved procedure with `params`: an ordinary plan, as this caller.
    pub async fn run_procedure(
        &self,
        params: crate::RunProcedureParams,
    ) -> Result<openmhs_types::plan::Plan, ClientError> {
        self.call(method::PROCEDURES_RUN, params).await
    }

    pub async fn device_status(
        &self,
        device: Option<&str>,
        refresh: bool,
    ) -> Result<Vec<openmhs_types::DeviceStatus>, ClientError> {
        let params = crate::DeviceStatusParams {
            device: device.map(str::to_owned),
            refresh,
        };
        self.call(method::DEVICES_STATUS, params).await
    }

    /// The capability graph: devices, relationships, shared resources, frames.
    pub async fn graph(&self) -> Result<openmhs_types::graph::Graph, ClientError> {
        self.call(method::GRAPH_GET, serde_json::json!({})).await
    }

    /// Take exclusive use of devices and shared resources until released or due.
    pub async fn acquire_lease(
        &self,
        params: crate::AcquireLeaseParams,
    ) -> Result<openmhs_types::lease::Lease, ClientError> {
        self.call(method::LEASES_ACQUIRE, params).await
    }

    pub async fn lease(&self, id: &str) -> Result<openmhs_types::lease::Lease, ClientError> {
        self.call(method::LEASES_GET, crate::IdParams { id: id.into() })
            .await
    }

    pub async fn leases(
        &self,
        active: bool,
        limit: Option<usize>,
    ) -> Result<Vec<openmhs_types::lease::Lease>, ClientError> {
        self.call(
            method::LEASES_LIST,
            crate::ListLeasesParams { active, limit },
        )
        .await
    }

    pub async fn renew_lease(
        &self,
        id: &str,
        ttl_ms: u64,
    ) -> Result<openmhs_types::lease::Lease, ClientError> {
        self.call(
            method::LEASES_RENEW,
            crate::RenewLeaseParams {
                id: id.into(),
                ttl_ms,
            },
        )
        .await
    }

    pub async fn release_lease(
        &self,
        id: &str,
        reason: Option<String>,
    ) -> Result<openmhs_types::lease::Lease, ClientError> {
        self.call(
            method::LEASES_RELEASE,
            crate::ReleaseLeaseParams {
                id: id.into(),
                reason,
            },
        )
        .await
    }

    /// Submit a plan; returns when it ends or waits (spec/plans.md §2).
    pub async fn submit_plan(
        &self,
        plan: openmhs_types::plan::PlanSpec,
    ) -> Result<openmhs_types::plan::Plan, ClientError> {
        self.call(method::PLANS_SUBMIT, crate::PlanParams { plan })
            .await
    }

    /// Dry-run a plan against twins of its devices (spec/plans.md §8).
    pub async fn preflight_plan(
        &self,
        plan: openmhs_types::plan::PlanSpec,
    ) -> Result<openmhs_types::plan::PlanPreflight, ClientError> {
        self.call(method::PLANS_PREFLIGHT, crate::PlanParams { plan })
            .await
    }

    pub async fn plan(&self, id: &str) -> Result<openmhs_types::plan::Plan, ClientError> {
        self.call(method::PLANS_GET, crate::IdParams { id: id.into() })
            .await
    }

    pub async fn plans(
        &self,
        limit: Option<usize>,
    ) -> Result<Vec<openmhs_types::plan::Plan>, ClientError> {
        self.call(method::PLANS_LIST, crate::ListPlansParams { limit })
            .await
    }

    pub async fn decide_plan(
        &self,
        id: &str,
        decision: openmhs_types::plan::PlanDecision,
        reason: Option<String>,
    ) -> Result<openmhs_types::plan::Plan, ClientError> {
        self.call(
            method::PLANS_DECIDE,
            crate::DecidePlanParams {
                id: id.into(),
                decision,
                reason,
            },
        )
        .await
    }

    pub async fn cancel_plan(
        &self,
        id: &str,
        reason: Option<String>,
    ) -> Result<openmhs_types::plan::Plan, ClientError> {
        self.call(
            method::PLANS_CANCEL,
            crate::CancelPlanParams {
                id: id.into(),
                reason,
            },
        )
        .await
    }

    pub async fn describe(&self, device: &str) -> Result<Descriptor, ClientError> {
        self.call(
            method::DEVICES_DESCRIBE,
            DeviceParams {
                device: device.into(),
            },
        )
        .await
    }

    pub async fn read(&self, device: &str, capability: &str) -> Result<ReadResult, ClientError> {
        self.call(
            method::STATE_READ,
            ReadParams {
                device: device.into(),
                capability: capability.into(),
            },
        )
        .await
    }

    /// Submit a command together with an approval requested and approved ahead of it.
    pub async fn submit_approved(
        &self,
        request: serde_json::Value,
        approval_id: &str,
    ) -> Result<Receipt, ClientError> {
        self.call(
            method::COMMANDS_SUBMIT,
            SubmitParams {
                request,
                approval_id: Some(approval_id.into()),
            },
        )
        .await
    }

    /// Ask for approval of a command before submitting it.
    pub async fn request_approval(
        &self,
        request: serde_json::Value,
        ttl_ms: Option<u64>,
    ) -> Result<Approval, ClientError> {
        self.call(
            method::APPROVALS_REQUEST,
            ApprovalRequestParams { request, ttl_ms },
        )
        .await
    }

    pub async fn approval(&self, id: &str) -> Result<Approval, ClientError> {
        self.call(method::APPROVALS_GET, IdParams { id: id.into() })
            .await
    }

    pub async fn approvals(
        &self,
        statuses: Vec<ApprovalStatus>,
        limit: Option<usize>,
    ) -> Result<Vec<Approval>, ClientError> {
        self.call(
            method::APPROVALS_LIST,
            ListApprovalsParams { statuses, limit },
        )
        .await
    }

    async fn approval_action(
        &self,
        method: &str,
        id: &str,
        reason: Option<String>,
    ) -> Result<Approval, ClientError> {
        self.call(
            method,
            ApprovalActionParams {
                id: id.into(),
                reason,
            },
        )
        .await
    }

    pub async fn approve_approval(&self, id: &str) -> Result<Approval, ClientError> {
        self.approval_action(method::APPROVALS_APPROVE, id, None)
            .await
    }

    pub async fn deny_approval(
        &self,
        id: &str,
        reason: Option<String>,
    ) -> Result<Approval, ClientError> {
        self.approval_action(method::APPROVALS_DENY, id, reason)
            .await
    }

    pub async fn revoke_approval(
        &self,
        id: &str,
        reason: Option<String>,
    ) -> Result<Approval, ClientError> {
        self.approval_action(method::APPROVALS_REVOKE, id, reason)
            .await
    }

    pub async fn submit(&self, request: serde_json::Value) -> Result<Receipt, ClientError> {
        self.call(
            method::COMMANDS_SUBMIT,
            SubmitParams {
                request,
                approval_id: None,
            },
        )
        .await
    }

    /// Would this request be allowed right now? Creates no command.
    pub async fn preflight(
        &self,
        request: serde_json::Value,
    ) -> Result<PreflightResult, ClientError> {
        self.call(
            method::COMMANDS_PREFLIGHT,
            SubmitParams {
                request,
                approval_id: None,
            },
        )
        .await
    }

    pub async fn command(&self, command_id: &str) -> Result<Receipt, ClientError> {
        self.call(method::COMMANDS_GET, id(command_id)).await
    }

    pub async fn commands(
        &self,
        states: Vec<CommandState>,
        limit: Option<usize>,
    ) -> Result<Vec<Receipt>, ClientError> {
        self.call(method::COMMANDS_LIST, ListCommandsParams { states, limit })
            .await
    }

    /// What the journal says happened for `device` (optionally one capability), from the
    /// newest commands this credential may list: refusals and the numbers behind them,
    /// confirmed values, tracking error, readback latency, unresolved outcomes (ADR-0022).
    pub async fn insights(
        &self,
        device: &str,
        capability: Option<&str>,
    ) -> Result<crate::insights::Insights, ClientError> {
        const WINDOW: usize = 500;
        let receipts = self.commands(Vec::new(), Some(WINDOW)).await?;
        Ok(crate::insights::summarize(
            &receipts, device, capability, WINDOW,
        ))
    }

    /// Which registered devices can do what `query` asks (capability-based selection).
    /// Candidates only: whether a command is allowed is decided when it is checked or sent.
    pub async fn find_capabilities(
        &self,
        query: &crate::selection::CapabilityQuery,
    ) -> Result<crate::selection::Selection, ClientError> {
        let descriptors = self.devices().await?;
        let statuses = self.device_status(None, false).await?;
        Ok(crate::selection::select(&descriptors, &statuses, query))
    }

    pub async fn approve(&self, command_id: &str) -> Result<Receipt, ClientError> {
        self.call(method::COMMANDS_APPROVE, id(command_id)).await
    }

    pub async fn cancel(&self, command_id: &str) -> Result<Receipt, ClientError> {
        self.call(method::COMMANDS_CANCEL, id(command_id)).await
    }

    pub async fn reconcile(&self, command_id: &str) -> Result<Receipt, ClientError> {
        self.call(method::COMMANDS_RECONCILE, id(command_id)).await
    }

    /// Operator only: record whether an `UNKNOWN_OUTCOME` command took effect, and how
    /// that was established. The device is not contacted.
    pub async fn resolve(
        &self,
        command_id: &str,
        applied: bool,
        reason: &str,
    ) -> Result<Receipt, ClientError> {
        self.call(
            method::COMMANDS_RESOLVE,
            ResolveParams {
                command_id: command_id.into(),
                applied,
                reason: reason.into(),
            },
        )
        .await
    }

    pub async fn grants(&self) -> Result<Vec<Grant>, ClientError> {
        self.call(method::GRANTS_LIST, serde_json::json!({})).await
    }

    pub async fn add_grant(&self, grant: Grant) -> Result<Grant, ClientError> {
        self.call(method::GRANTS_ADD, GrantParams { grant }).await
    }

    pub async fn revoke_grant(&self, grant_id: &str) -> Result<Grant, ClientError> {
        self.call(
            method::GRANTS_REVOKE,
            IdParams {
                id: grant_id.into(),
            },
        )
        .await
    }

    pub async fn create_token(
        &self,
        params: CreateTokenParams,
    ) -> Result<CreatedToken, ClientError> {
        self.call(method::TOKENS_CREATE, params).await
    }

    pub async fn tokens(&self) -> Result<Vec<TokenInfo>, ClientError> {
        self.call(method::TOKENS_LIST, serde_json::json!({})).await
    }

    /// Replace a credential with a fresh secret for the same principal and scopes; the old
    /// one stops working immediately.
    pub async fn rotate_token(
        &self,
        token_id: &str,
        ttl_ms: Option<u64>,
    ) -> Result<CreatedToken, ClientError> {
        self.call(
            method::TOKENS_ROTATE,
            RotateTokenParams {
                id: token_id.into(),
                ttl_ms,
            },
        )
        .await
    }

    pub async fn revoke_token(&self, token_id: &str) -> Result<TokenInfo, ClientError> {
        self.call(
            method::TOKENS_REVOKE,
            IdParams {
                id: token_id.into(),
            },
        )
        .await
    }

    pub async fn verify_audit(&self) -> Result<AuditVerifyResult, ClientError> {
        self.call(method::AUDIT_VERIFY, serde_json::json!({})).await
    }

    pub async fn audit_tail(&self, limit: Option<usize>) -> Result<Vec<AuditEntry>, ClientError> {
        self.call(method::AUDIT_TAIL, AuditTailParams { limit })
            .await
    }

    pub async fn sim_fault(
        &self,
        device: &str,
        fault: &str,
    ) -> Result<SimFaultResult, ClientError> {
        self.call(
            method::SIM_FAULT,
            SimFaultParams {
                device: device.into(),
                fault: fault.into(),
            },
        )
        .await
    }
}

fn id(command_id: &str) -> CommandIdParams {
    CommandIdParams {
        command_id: command_id.into(),
    }
}
