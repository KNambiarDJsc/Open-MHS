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

    pub async fn approve(&self, command_id: &str) -> Result<Receipt, ClientError> {
        self.call(method::COMMANDS_APPROVE, id(command_id)).await
    }

    pub async fn cancel(&self, command_id: &str) -> Result<Receipt, ClientError> {
        self.call(method::COMMANDS_CANCEL, id(command_id)).await
    }

    pub async fn reconcile(&self, command_id: &str) -> Result<Receipt, ClientError> {
        self.call(method::COMMANDS_RECONCILE, id(command_id)).await
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
