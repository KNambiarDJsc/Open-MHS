//! The OpenMHS local gateway API (spec/api.md, ADR-0005, ADR-0011).
//!
//! JSON-RPC 2.0 over HTTP `POST /rpc` on loopback, authenticated with a bearer token.
//! This crate defines the wire types once, for the gateway and every Rust client.

pub mod client;
pub mod insights;
pub mod runtime;
pub mod selection;

use openmhs_policy::{Grant, PrincipalKind, Scope};
use openmhs_types::{ApprovalStatus, CommandState, ErrorObject, StateValue};
use serde::{Deserialize, Serialize};

pub use client::{Client, ClientError};
pub use runtime::RuntimeInfo;

pub const RPC_PATH: &str = "/rpc";

/// Method names.
pub mod method {
    pub const GATEWAY_INFO: &str = "gateway.info";
    pub const DEVICES_LIST: &str = "devices.list";
    pub const DEVICES_DESCRIBE: &str = "devices.describe";
    pub const DEVICES_STATUS: &str = "devices.status";
    pub const EVENTS_POLL: &str = "events.poll";
    pub const WATCHES_CREATE: &str = "watches.create";
    pub const WATCHES_LIST: &str = "watches.list";
    pub const WATCHES_CANCEL: &str = "watches.cancel";
    pub const PROCEDURES_LIST: &str = "procedures.list";
    pub const PROCEDURES_GET: &str = "procedures.get";
    pub const PROCEDURES_PROPOSE: &str = "procedures.propose";
    pub const PROCEDURES_ADD: &str = "procedures.add";
    pub const PROCEDURES_APPROVE: &str = "procedures.approve";
    pub const PROCEDURES_REMOVE: &str = "procedures.remove";
    pub const PROCEDURES_CHECK: &str = "procedures.check";
    pub const PROCEDURES_RUN: &str = "procedures.run";
    pub const GRAPH_GET: &str = "graph.get";
    pub const LEASES_ACQUIRE: &str = "leases.acquire";
    pub const LEASES_GET: &str = "leases.get";
    pub const LEASES_LIST: &str = "leases.list";
    pub const LEASES_RENEW: &str = "leases.renew";
    pub const LEASES_RELEASE: &str = "leases.release";
    pub const STATE_READ: &str = "state.read";
    pub const COMMANDS_SUBMIT: &str = "commands.submit";
    pub const COMMANDS_PREFLIGHT: &str = "commands.preflight";
    pub const COMMANDS_GET: &str = "commands.get";
    pub const COMMANDS_LIST: &str = "commands.list";
    pub const COMMANDS_APPROVE: &str = "commands.approve";
    pub const COMMANDS_CANCEL: &str = "commands.cancel";
    pub const COMMANDS_RECONCILE: &str = "commands.reconcile";
    pub const COMMANDS_RESOLVE: &str = "commands.resolve";
    pub const APPROVALS_REQUEST: &str = "approvals.request";
    pub const APPROVALS_GET: &str = "approvals.get";
    pub const APPROVALS_LIST: &str = "approvals.list";
    pub const APPROVALS_APPROVE: &str = "approvals.approve";
    pub const APPROVALS_DENY: &str = "approvals.deny";
    pub const APPROVALS_REVOKE: &str = "approvals.revoke";
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
    pub const PLANS_SUBMIT: &str = "plans.submit";
    pub const PLANS_PREFLIGHT: &str = "plans.preflight";
    pub const PLANS_GET: &str = "plans.get";
    pub const PLANS_LIST: &str = "plans.list";
    pub const PLANS_DECIDE: &str = "plans.decide";
    pub const PLANS_CANCEL: &str = "plans.cancel";
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

/// `events.poll` (ADR-0023): events after a cursor, optionally waiting for one.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventsPollParams {
    /// The `stream` of the batch the cursor came from; a different one means the gateway
    /// restarted, and the answer starts again (`reset`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
    /// Return events with a greater `seq`. Absent: only events from now on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<u64>,
    /// `*` patterns of device ids; empty means all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<String>,
    /// `observation`, `availability`, `command`; empty means all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<openmhs_types::event::EventKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Wait this long for an event if there is none yet (at most 30 000).
    #[serde(default)]
    pub wait_ms: u64,
}

/// `watches.create` (ADR-0023): sample a capability that declares `subscribe`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchParams {
    pub device: String,
    pub capability: String,
    pub interval_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

/// `procedures.propose` and `procedures.add` (spec/procedures.md).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcedureParams {
    pub procedure: openmhs_types::procedure::ProcedureSpec,
}

/// One procedure: a version, or the newest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcedureRef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
}

/// `procedures.check` and `procedures.run`: the newest approved version unless one is named.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunProcedureParams {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub params: std::collections::BTreeMap<String, serde_json::Value>,
}

/// `devices.status`: one device or all, optionally probing first (ADR-0019).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceStatusParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Probe devices not checked in the last second before answering.
    #[serde(default)]
    pub refresh: bool,
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
    /// An approval requested and approved ahead of this command (`approvals.request`).
    /// It is spent only if it was given for exactly this command, to this requester.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequestParams {
    /// The command to be approved, as it will be submitted.
    pub request: serde_json::Value,
    /// How long the approval stays valid once approved; default 15 minutes, at most 24 hours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalActionParams {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListApprovalsParams {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub statuses: Vec<ApprovalStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AcquireLeaseParams {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<String>,
    /// Required; at most 24 hours.
    pub ttl_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenewLeaseParams {
    pub id: String,
    pub ttl_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseLeaseParams {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListLeasesParams {
    /// Only leases that hold something now.
    #[serde(default)]
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandIdParams {
    pub command_id: String,
}

/// An operator's account of an `UNKNOWN_OUTCOME` command: whether it took effect, and how
/// that was established.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveParams {
    pub command_id: String,
    pub applied: bool,
    pub reason: String,
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

/// `plans.submit` and `plans.preflight` (spec/plans.md).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanParams {
    pub plan: openmhs_types::plan::PlanSpec,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListPlansParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecidePlanParams {
    pub id: String,
    pub decision: openmhs_types::plan::PlanDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelPlanParams {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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
