//! The OpenMHS local gateway API (spec/api.md, ADR-0005, ADR-0011).
//!
//! JSON-RPC 2.0 over HTTP `POST /rpc` on loopback, authenticated with a bearer token.
//! This crate defines the wire types once, for the gateway and every Rust client.

pub mod client;
pub mod runtime;

use openmhs_policy::{Grant, PrincipalKind, Scope};
use openmhs_types::{CommandState, ErrorObject, StateValue};
use serde::{Deserialize, Serialize};

pub use client::{Client, ClientError};
pub use runtime::RuntimeInfo;

pub const RPC_PATH: &str = "/rpc";

/// Method names.
pub mod method {
    pub const GATEWAY_INFO: &str = "gateway.info";
    pub const DEVICES_LIST: &str = "devices.list";
    pub const DEVICES_DESCRIBE: &str = "devices.describe";
    pub const STATE_READ: &str = "state.read";
    pub const COMMANDS_SUBMIT: &str = "commands.submit";
    pub const COMMANDS_PREFLIGHT: &str = "commands.preflight";
    pub const COMMANDS_GET: &str = "commands.get";
    pub const COMMANDS_LIST: &str = "commands.list";
    pub const COMMANDS_APPROVE: &str = "commands.approve";
    pub const COMMANDS_CANCEL: &str = "commands.cancel";
    pub const COMMANDS_RECONCILE: &str = "commands.reconcile";
    pub const GRANTS_LIST: &str = "grants.list";
    pub const GRANTS_ADD: &str = "grants.add";
    pub const GRANTS_REVOKE: &str = "grants.revoke";
    pub const TOKENS_CREATE: &str = "tokens.create";
    pub const TOKENS_LIST: &str = "tokens.list";
    pub const TOKENS_REVOKE: &str = "tokens.revoke";
    pub const TOKENS_ROTATE: &str = "tokens.rotate";
    pub const AUDIT_VERIFY: &str = "audit.verify";
    pub const AUDIT_TAIL: &str = "audit.tail";
    pub const SIM_FAULT: &str = "sim.fault";
}

/// JSON-RPC error codes. OpenMHS errors use [`rpc_code::GATEWAY`] and carry the stable
/// OpenMHS code in `data.code`; clients branch on that, never on the message.
pub mod rpc_code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const GATEWAY: i64 = -32000;
}

// ------------------------------------------------------------------ envelope

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<ErrorData>,
}

/// Structured detail of a gateway error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorData {
    /// Stable OpenMHS error code (spec/errors.md).
    pub code: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ErrorObject>,
    /// For `command.idempotency.conflict`: the existing command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<String>,
}

// ------------------------------------------------------------------ identity

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Manages policy, tokens and approvals. Still subject to grants for device access.
    Operator,
    /// Reads devices and submits commands within its grants. Cannot change policy.
    Agent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GatewayInfo {
    pub version: String,
    pub spec_version: String,
    pub principal: String,
    pub role: Role,
    /// The credential this call authenticated with; absent for the operator runtime token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    /// What the credential may reach (spec/authorization.md §7).
    #[serde(default)]
    pub scopes: Vec<Scope>,
    pub simulation: bool,
    pub devices: usize,
}

// ------------------------------------------------------------------ params and results

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceParams {
    pub device: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadParams {
    pub device: String,
    pub capability: String,
}

/// A reading plus how fresh it is relative to the capability's declared freshness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadResult {
    pub device: String,
    pub capability: String,
    pub state: StateValue,
    pub age_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_ms: Option<u64>,
    /// False when older than `max_age_ms` or not `GOOD` quality.
    pub usable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitParams {
    /// A command request (`command.schema.json`). The principal comes from the token.
    pub request: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandIdParams {
    pub command_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListCommandsParams {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<CommandState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantParams {
    pub grant: Grant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdParams {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTokenParams {
    pub principal: String,
    pub role: Role,
    /// The credential's ceiling (spec/authorization.md §7). Without scopes it can
    /// authenticate but cannot read or command anything.
    #[serde(default)]
    pub scopes: Vec<Scope>,
    /// Lifetime from now. Absent means the credential does not expire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

impl CreateTokenParams {
    /// A credential limited to `scopes`.
    pub fn scoped(principal: &str, role: Role, scopes: Vec<Scope>) -> Self {
        Self {
            principal: principal.into(),
            role,
            scopes,
            ttl_ms: None,
        }
    }

    /// A credential whose ceiling is open: policy grants alone limit it. Convenient for
    /// tests and demos; prefer [`CreateTokenParams::scoped`] for anything real.
    pub fn unrestricted(principal: &str, role: Role) -> Self {
        Self::scoped(principal, role, vec![Scope::any()])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotateTokenParams {
    pub id: String,
    /// Lifetime of the replacement; absent keeps the old credential's remaining lifetime
    /// policy (no expiry if it had none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

/// What the gateway knows about a credential. Never contains the secret or its hash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenInfo {
    pub id: String,
    pub principal: String,
    pub role: Role,
    #[serde(default = "default_kind")]
    pub kind: PrincipalKind,
    pub created_at: openmhs_types::Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<openmhs_types::Timestamp>,
    /// Absent in credentials issued before M8, which therefore authorize nothing.
    #[serde(default)]
    pub scopes: Vec<Scope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<openmhs_types::Timestamp>,
}

fn default_kind() -> PrincipalKind {
    PrincipalKind::Human
}

impl TokenInfo {
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    pub fn is_expired(&self, now: openmhs_types::Timestamp) -> bool {
        self.expires_at.is_some_and(|t| now >= t)
    }
}

/// Returned once by `tokens.create`; the secret is not stored and cannot be shown again.
#[derive(Clone, Serialize, Deserialize)]
pub struct CreatedToken {
    #[serde(flatten)]
    pub info: TokenInfo,
    pub token: String,
}

// The secret must never reach a log through `{:?}`.
impl std::fmt::Debug for CreatedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreatedToken")
            .field("info", &self.info)
            .field("token", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditVerifyResult {
    pub records: u64,
    pub valid: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_invalid: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditTailParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub seq: i64,
    pub at: openmhs_types::Timestamp,
    pub kind: String,
    pub subject: String,
    pub data: serde_json::Value,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimFaultParams {
    pub device: String,
    /// `name` or `name=value`, e.g. `stale=5000`, `apply-then-hang`, `clear`.
    pub fault: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimFaultResult {
    pub message: String,
}
