//! JSON-RPC over HTTP: authentication, roles and method dispatch.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use openmhs_api::{
    AcquireLeaseParams, ApprovalActionParams, ApprovalRequestParams, AuditEntry, AuditTailParams,
    AuditVerifyResult, CommandIdParams, CreateTokenParams, CreatedToken, DeviceParams, ErrorData,
    GatewayInfo, GrantParams, IdParams, ListApprovalsParams, ListCommandsParams, ListLeasesParams,
    RPC_PATH, ReadParams, ReadResult, ReleaseLeaseParams, RenewLeaseParams, Role,
    RotateTokenParams, RpcError, RpcRequest, RpcResponse, SimFaultParams, SimFaultResult,
    SubmitParams, method, rpc_code,
};
use openmhs_engine::journal::sha256_hex;
use openmhs_engine::{Engine, EngineError};
use openmhs_policy::{AuthMethod, Caller, Policy, Scope};
use openmhs_sim::SimControl;
use openmhs_types::{Clock, ErrorObject};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;

use crate::store::{Lookup, NewToken, PolicyStore, TokenError, TokenStore, validate_grant};

const DEFAULT_LIST_LIMIT: usize = 50;
const MAX_LIST_LIMIT: usize = 500;

/// A verified request: who is calling, how, and with what ceiling. Built only by
/// [`Shared::authenticate`]; nothing in a request body can set any of it.
#[derive(Debug, Clone)]
pub(crate) struct Session {
    pub principal: String,
    pub role: Role,
    pub caller: Caller,
}

impl Session {
    /// The gateway's own operator, authenticated by the runtime-file token.
    pub fn operator(principal: &str) -> Self {
        Self {
            principal: principal.to_owned(),
            role: Role::Operator,
            caller: Caller::authenticated(
                principal,
                "operator-runtime",
                AuthMethod::OperatorRuntime,
                vec![Scope::any()],
            ),
        }
    }
}

/// Why a bearer token was not accepted.
enum Rejection {
    /// Missing, malformed or unknown. Nothing is said about which.
    Unknown,
    Expired(String),
    Revoked(String),
}

pub(crate) struct Shared {
    pub engine: Engine,
    pub clock: Arc<dyn Clock>,
    pub simulation: bool,
    pub operator: Session,
    pub operator_token_sha256: String,
    pub tokens: Mutex<TokenStore>,
    /// Credentials already reported as used after expiry or revocation, so a client that
    /// keeps retrying cannot fill the audit log.
    pub reported: Mutex<std::collections::HashSet<(String, &'static str)>>,
    /// Serializes policy edits so concurrent grant changes can't lose each other.
    pub policy_store: tokio::sync::Mutex<PolicyStore>,
    pub sims: BTreeMap<String, Arc<dyn SimControl>>,
}

impl Shared {
    fn tokens(&self) -> MutexGuard<'_, TokenStore> {
        self.tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Verify a bearer token against the clock, at every request: a credential revoked or
    /// expired a moment ago cannot start another call.
    fn authenticate(&self, token: &str) -> Result<Session, Rejection> {
        let presented = sha256_hex(token);
        if bool::from(
            presented
                .as_bytes()
                .ct_eq(self.operator_token_sha256.as_bytes()),
        ) {
            return Ok(self.operator.clone());
        }
        match self.tokens().lookup(token, self.clock.now()) {
            Lookup::Valid(t) => Ok(Session {
                principal: t.principal.clone(),
                role: t.role,
                caller: Caller::authenticated(
                    t.principal.clone(),
                    t.id.clone(),
                    AuthMethod::Credential,
                    t.scopes.clone(),
                ),
            }),
            Lookup::Expired(t) => Err(Rejection::Expired(t.id.clone())),
            Lookup::Revoked(t) => Err(Rejection::Revoked(t.id.clone())),
            Lookup::Unknown => Err(Rejection::Unknown),
        }
    }

    /// Record, once per credential and reason, that a known credential was refused.
    fn report_rejection(&self, id: &str, reason: &'static str) {
        let first = self
            .reported
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((id.to_owned(), reason));
        if first {
            let _ = self.audit("credential.rejected", id, json!({ "reason": reason }));
        }
    }

    fn audit(&self, kind: &str, subject: &str, data: Value) -> Result<(), RpcError> {
        self.engine
            .journal()
            .append_audit(self.clock.now(), kind, subject, &data)
            .map_err(|e| gateway_error("gateway.journal", e.to_string(), vec![]))?;
        Ok(())
    }
}

pub(crate) fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route(RPC_PATH, post(handle))
        .with_state(shared)
}

// ------------------------------------------------------------------ errors

fn gateway_error(code: &str, message: impl Into<String>, errors: Vec<ErrorObject>) -> RpcError {
    RpcError {
        code: rpc_code::GATEWAY,
        message: message.into(),
        data: Some(ErrorData {
            code: code.to_owned(),
            errors,
            command_id: None,
        }),
    }
}

fn rpc_error(code: i64, message: impl Into<String>) -> RpcError {
    RpcError {
        code,
        message: message.into(),
        data: None,
    }
}

fn engine_error(e: EngineError) -> RpcError {
    let code = e.code().to_owned();
    // The code travels in `data.code`; a denial's own Display would repeat it.
    let message = match &e {
        EngineError::Denied(error) => error.message.clone(),
        other => other.to_string(),
    };
    let mut command_id = None;
    let errors = match e {
        EngineError::InvalidRequest(errors) => errors,
        EngineError::Denied(error) => vec![error],
        EngineError::IdempotencyConflict(id) => {
            command_id = Some(id);
            vec![]
        }
        _ => vec![],
    };
    RpcError {
        code: rpc_code::GATEWAY,
        message,
        data: Some(ErrorData {
            code,
            errors,
            command_id,
        }),
    }
}

fn forbidden(what: &str) -> RpcError {
    gateway_error(
        "gateway.forbidden",
        format!("{what} requires an operator token"),
        vec![],
    )
}

// ------------------------------------------------------------------ transport

fn reply(id: Value, result: Result<Value, RpcError>) -> RpcResponse {
    let (result, error) = match result {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e)),
    };
    RpcResponse {
        jsonrpc: "2.0".into(),
        id,
        result,
        error,
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

async fn handle(State(shared): State<Arc<Shared>>, headers: HeaderMap, body: Bytes) -> Response {
    // Requiring JSON forces a CORS preflight, which this server never answers, so a web
    // page in the user's browser cannot drive the gateway even if it guessed a token.
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if !is_json {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let rejection = match bearer(&headers) {
        Some(token) => match shared.authenticate(token) {
            Ok(session) => Ok(session),
            Err(r) => Err(r),
        },
        None => Err(Rejection::Unknown),
    };
    let session = match rejection {
        Ok(session) => session,
        Err(r) => {
            // Expired and revoked are only distinguishable to someone holding the secret.
            let err = match r {
                Rejection::Unknown => gateway_error(
                    "gateway.unauthorized",
                    "missing or invalid bearer token",
                    vec![],
                ),
                Rejection::Expired(id) => {
                    shared.report_rejection(&id, "expired");
                    gateway_error("credential.expired", "this credential has expired", vec![])
                }
                Rejection::Revoked(id) => {
                    shared.report_rejection(&id, "revoked");
                    gateway_error("credential.revoked", "this credential was revoked", vec![])
                }
            };
            let body = reply(Value::Null, Err(err));
            return (StatusCode::UNAUTHORIZED, axum::Json(body)).into_response();
        }
    };
    let request: RpcRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            let body = reply(
                Value::Null,
                Err(rpc_error(rpc_code::PARSE_ERROR, e.to_string())),
            );
            return axum::Json(body).into_response();
        }
    };
    let result = if request.jsonrpc == "2.0" {
        dispatch(&shared, &session, &request.method, request.params).await
    } else {
        Err(rpc_error(
            rpc_code::INVALID_REQUEST,
            "jsonrpc must be \"2.0\"",
        ))
    };
    axum::Json(reply(request.id, result)).into_response()
}

fn params<T: DeserializeOwned>(v: Value) -> Result<T, RpcError> {
    let v = if v.is_null() { json!({}) } else { v };
    serde_json::from_value(v).map_err(|e| rpc_error(rpc_code::INVALID_PARAMS, e.to_string()))
}

fn ok<T: Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(|e| gateway_error("gateway.internal", e.to_string(), vec![]))
}

/// Run engine work in its own task, so a client that disconnects mid-request cannot
/// cancel a command half-way through its lifecycle.
async fn detached<T: Send + 'static>(
    work: impl Future<Output = Result<T, EngineError>> + Send + 'static,
) -> Result<T, RpcError> {
    match tokio::spawn(work).await {
        Ok(r) => r.map_err(engine_error),
        Err(e) => Err(gateway_error("gateway.internal", e.to_string(), vec![])),
    }
}

// ------------------------------------------------------------------ methods

async fn dispatch(
    shared: &Arc<Shared>,
    session: &Session,
    method_name: &str,
    raw: Value,
) -> Result<Value, RpcError> {
    let operator = session.role == Role::Operator;
    let require_operator = |what: &str| {
        if operator {
            Ok(())
        } else {
            Err(forbidden(what))
        }
    };
    let engine = &shared.engine;
    match method_name {
        method::GATEWAY_INFO => ok(GatewayInfo {
            version: env!("CARGO_PKG_VERSION").into(),
            spec_version: "0.1".into(),
            principal: session.principal.clone(),
            role: session.role,
            credential_id: session.caller.credential_id().map(str::to_owned),
            scopes: session.caller.scopes().to_vec(),
            simulation: shared.simulation,
            devices: engine.devices().len(),
        }),

        method::DEVICES_LIST => ok(engine.devices()),

        // Visible to every caller, like the device list it is built from.
        method::GRAPH_GET => ok(engine.graph()),

        method::LEASES_ACQUIRE => {
            let p: AcquireLeaseParams = params(raw)?;
            ok(engine
                .acquire_lease(
                    &session.caller,
                    openmhs_engine::LeaseRequest {
                        devices: p.devices,
                        resources: p.resources,
                        ttl_ms: p.ttl_ms,
                        reason: p.reason,
                        plan_id: None,
                    },
                )
                .map_err(engine_error)?)
        }

        // Every caller sees every lease: who holds a device is how callers coordinate.
        method::LEASES_GET => {
            let p: IdParams = params(raw)?;
            ok(engine.lease(&p.id).map_err(engine_error)?)
        }

        method::LEASES_LIST => {
            let p: ListLeasesParams = params(raw)?;
            let limit = p.limit.unwrap_or(DEFAULT_LIST_LIMIT).min(MAX_LIST_LIMIT);
            ok(engine.leases(p.active, limit).map_err(engine_error)?)
        }

        method::LEASES_RENEW => {
            let p: RenewLeaseParams = params(raw)?;
            ok(engine
                .renew_lease(&session.caller, &p.id, p.ttl_ms)
                .map_err(engine_error)?)
        }

        // The holder, or an approver freeing devices a holder left leased.
        method::LEASES_RELEASE => {
            let p: ReleaseLeaseParams = params(raw)?;
            ok(engine
                .release_lease(&session.caller, &p.id, p.reason.as_deref())
                .map_err(engine_error)?)
        }

        method::DEVICES_DESCRIBE => {
            let p: DeviceParams = params(raw)?;
            match engine.descriptor(&p.device) {
                Some(d) => ok(d),
                None => Err(unknown_device(&p.device)),
            }
        }

        method::STATE_READ => {
            let p: ReadParams = params(raw)?;
            let state = engine
                .read(&session.caller, &p.device, &p.capability)
                .await
                .map_err(engine_error)?;
            let now = shared.clock.now();
            let max_age_ms = engine
                .descriptor(&p.device)
                .and_then(|d| d.capability(&p.capability).cloned())
                .and_then(|c| c.freshness)
                .map(|f| f.max_age_ms);
            ok(ReadResult {
                age_ms: state.age_ms(now),
                usable: state.is_usable(now, max_age_ms),
                max_age_ms,
                device: p.device,
                capability: p.capability,
                state,
            })
        }

        method::COMMANDS_SUBMIT => {
            let p: SubmitParams = params(raw)?;
            let engine = engine.clone();
            let caller = session.caller.clone();
            ok(detached(async move {
                match p.approval_id {
                    Some(approval) => engine.submit_approved(&caller, &p.request, &approval).await,
                    None => engine.submit(&caller, &p.request).await,
                }
            })
            .await?)
        }

        method::COMMANDS_PREFLIGHT => {
            let p: SubmitParams = params(raw)?;
            let engine = engine.clone();
            let caller = session.caller.clone();
            ok(detached(async move { engine.preflight(&caller, &p.request).await }).await?)
        }

        method::COMMANDS_GET => {
            let p: CommandIdParams = params(raw)?;
            let receipt = engine
                .receipt(&p.command_id)
                .map_err(engine_error)?
                .filter(|r| operator || r.principal == session.principal)
                .ok_or_else(|| unknown_command(&p.command_id))?;
            ok(receipt)
        }

        method::COMMANDS_LIST => {
            let p: ListCommandsParams = params(raw)?;
            let limit = p.limit.unwrap_or(DEFAULT_LIST_LIMIT).min(MAX_LIST_LIMIT);
            let principal = (!operator).then_some(session.principal.as_str());
            ok(engine
                .commands(principal, &p.states, limit)
                .map_err(engine_error)?)
        }

        method::COMMANDS_APPROVE => {
            require_operator("approving commands")?;
            let p: CommandIdParams = params(raw)?;
            let engine = engine.clone();
            let approver = session.caller.clone();
            ok(detached(async move { engine.approve(&approver, &p.command_id).await }).await?)
        }

        method::APPROVALS_REQUEST => {
            let p: ApprovalRequestParams = params(raw)?;
            let engine = engine.clone();
            let caller = session.caller.clone();
            ok(detached(
                async move { engine.request_approval(&caller, &p.request, p.ttl_ms).await },
            )
            .await?)
        }

        method::APPROVALS_GET => {
            let p: IdParams = params(raw)?;
            ok(engine
                .approval(&session.caller, &p.id)
                .map_err(engine_error)?)
        }

        method::APPROVALS_LIST => {
            let p: ListApprovalsParams = params(raw)?;
            let limit = p.limit.unwrap_or(DEFAULT_LIST_LIMIT).min(MAX_LIST_LIMIT);
            ok(engine
                .approvals(&session.caller, &p.statuses, limit)
                .map_err(engine_error)?)
        }

        method::APPROVALS_APPROVE => {
            require_operator("approving")?;
            let p: ApprovalActionParams = params(raw)?;
            let engine = engine.clone();
            let caller = session.caller.clone();
            ok(detached(async move { engine.approve_approval(&caller, &p.id).await }).await?)
        }

        method::APPROVALS_DENY => {
            require_operator("denying approvals")?;
            let p: ApprovalActionParams = params(raw)?;
            ok(engine
                .deny_approval(&session.caller, &p.id, p.reason)
                .map_err(engine_error)?)
        }

        // The requester may withdraw their own; the engine lets approvers revoke any.
        method::APPROVALS_REVOKE => {
            let p: ApprovalActionParams = params(raw)?;
            ok(engine
                .revoke_approval(&session.caller, &p.id, p.reason)
                .map_err(engine_error)?)
        }

        method::COMMANDS_CANCEL => {
            let p: CommandIdParams = params(raw)?;
            ok(engine
                .cancel(&session.caller, &p.command_id)
                .map_err(engine_error)?)
        }

        method::COMMANDS_RECONCILE => {
            let p: CommandIdParams = params(raw)?;
            // Reconciliation only reads, so a requester may resolve its own command.
            let owned = engine
                .receipt(&p.command_id)
                .map_err(engine_error)?
                .is_some_and(|r| operator || r.principal == session.principal);
            if !owned {
                return Err(unknown_command(&p.command_id));
            }
            let engine = engine.clone();
            ok(detached(async move { engine.reconcile(&p.command_id).await }).await?)
        }

        method::GRANTS_LIST => {
            require_operator("listing grants")?;
            ok(engine.policy().grants)
        }

        method::GRANTS_ADD => {
            require_operator("changing grants")?;
            let p: GrantParams = params(raw)?;
            validate_grant(&p.grant)
                .map_err(|e| gateway_error(&e.code.clone(), e.message.clone(), vec![e]))?;
            let store = shared.policy_store.lock().await;
            let mut policy: Policy = engine.policy();
            if policy.grants.iter().any(|g| g.id == p.grant.id) {
                return Err(gateway_error(
                    "policy.grant.duplicate",
                    format!("a grant with id '{}' already exists", p.grant.id),
                    vec![],
                ));
            }
            policy.grants.push(p.grant.clone());
            store.save(&policy).map_err(io_error)?;
            engine.set_policy(policy).map_err(engine_error)?;
            shared.audit(
                "grant.added",
                &p.grant.id,
                json!({ "by": session.principal, "grant": p.grant }),
            )?;
            ok(p.grant)
        }

        method::GRANTS_REVOKE => {
            require_operator("changing grants")?;
            let p: IdParams = params(raw)?;
            let store = shared.policy_store.lock().await;
            let mut policy = engine.policy();
            let Some(pos) = policy.grants.iter().position(|g| g.id == p.id) else {
                return Err(gateway_error(
                    "policy.grant.unknown",
                    format!("no grant with id '{}'", p.id),
                    vec![],
                ));
            };
            let removed = policy.grants.remove(pos);
            store.save(&policy).map_err(io_error)?;
            engine.set_policy(policy).map_err(engine_error)?;
            shared.audit(
                "grant.revoked",
                &removed.id,
                json!({ "by": session.principal, "grant": removed }),
            )?;
            ok(removed)
        }

        method::TOKENS_CREATE => {
            require_operator("creating tokens")?;
            let p: CreateTokenParams = params(raw)?;
            let (info, token) = shared
                .tokens()
                .create(
                    NewToken {
                        principal: &p.principal,
                        role: p.role,
                        scopes: p.scopes,
                        ttl_ms: p.ttl_ms,
                    },
                    shared.clock.now(),
                )
                .map_err(token_error)?;
            shared.audit(
                "token.created",
                &info.id,
                json!({
                    "by": session.principal, "principal": info.principal, "role": info.role,
                    "scopes": info.scopes, "expires_at": info.expires_at,
                }),
            )?;
            ok(CreatedToken { info, token })
        }

        method::TOKENS_LIST => {
            require_operator("listing tokens")?;
            ok(shared.tokens().list())
        }

        method::TOKENS_REVOKE => {
            require_operator("revoking tokens")?;
            let p: IdParams = params(raw)?;
            let revoked = shared
                .tokens()
                .revoke(&p.id, shared.clock.now())
                .map_err(io_error)?;
            let Some((info, newly)) = revoked else {
                return Err(gateway_error(
                    "gateway.token.unknown",
                    format!("no token with id '{}'", p.id),
                    vec![],
                ));
            };
            if newly {
                // What the credential asked for and nobody has yet approved or used is
                // withdrawn with it. Commands already dispatched are not recalled.
                let withdrawn = engine
                    .revoke_credential_approvals(&info.id, &session.principal)
                    .map_err(engine_error)?;
                let cancelled = engine
                    .cancel_credential_plans(&info.id, &session.principal)
                    .await
                    .map_err(engine_error)?;
                let released = engine
                    .release_credential_leases(&info.id, &session.principal)
                    .map_err(engine_error)?;
                shared.audit(
                    "token.revoked",
                    &info.id,
                    json!({
                        "by": session.principal, "principal": info.principal,
                        "approvals_revoked": withdrawn, "leases_released": released,
                        "plans_cancelled": cancelled,
                    }),
                )?;
            }
            ok(info)
        }

        method::TOKENS_ROTATE => {
            require_operator("rotating tokens")?;
            let p: RotateTokenParams = params(raw)?;
            let (info, token, old) = shared
                .tokens()
                .rotate(&p.id, p.ttl_ms, shared.clock.now())
                .map_err(token_error)?;
            let withdrawn = engine
                .revoke_credential_approvals(&old.id, &session.principal)
                .map_err(engine_error)?;
            let cancelled = engine
                .cancel_credential_plans(&old.id, &session.principal)
                .await
                .map_err(engine_error)?;
            let released = engine
                .release_credential_leases(&old.id, &session.principal)
                .map_err(engine_error)?;
            shared.audit(
                "token.rotated",
                &info.id,
                json!({
                    "by": session.principal, "replaces": old.id, "principal": info.principal,
                    "approvals_revoked": withdrawn, "leases_released": released,
                    "plans_cancelled": cancelled,
                }),
            )?;
            ok(CreatedToken { info, token })
        }

        method::AUDIT_VERIFY => {
            require_operator("verifying the audit log")?;
            let v = engine.verify_audit().map_err(engine_error)?;
            ok(AuditVerifyResult {
                records: v.records,
                valid: v.is_valid(),
                first_invalid: v.first_invalid,
            })
        }

        method::AUDIT_TAIL => {
            require_operator("reading the audit log")?;
            let p: AuditTailParams = params(raw)?;
            let limit = p.limit.unwrap_or(20).min(MAX_LIST_LIMIT);
            let records = engine
                .journal()
                .audit_records()
                .map_err(|e| gateway_error("gateway.journal", e.to_string(), vec![]))?;
            let start = records.len().saturating_sub(limit);
            ok(records[start..]
                .iter()
                .map(|r| AuditEntry {
                    seq: r.seq,
                    at: openmhs_types::Timestamp::from_millis(r.at_ms),
                    kind: r.kind.clone(),
                    subject: r.subject.clone(),
                    data: serde_json::from_str(&r.data).unwrap_or(Value::String(r.data.clone())),
                    hash: r.hash.clone(),
                })
                .collect::<Vec<_>>())
        }

        method::SIM_FAULT => {
            require_operator("injecting faults")?;
            if !shared.simulation {
                return Err(gateway_error(
                    "gateway.not_simulation",
                    "fault injection is only available with `openmhs serve --sim`",
                    vec![],
                ));
            }
            let p: SimFaultParams = params(raw)?;
            let sim = shared
                .sims
                .get(&p.device)
                .ok_or_else(|| unknown_device(&p.device))?;
            let message = openmhs_sim::apply_fault(sim.as_ref(), &p.fault)
                .map_err(|m| gateway_error("sim.fault.unknown", m, vec![]))?;
            shared.audit(
                "sim.fault",
                &p.device,
                json!({ "by": session.principal, "fault": p.fault }),
            )?;
            ok(SimFaultResult { message })
        }

        other => Err(rpc_error(
            rpc_code::METHOD_NOT_FOUND,
            format!("unknown method '{other}'"),
        )),
    }
}

fn unknown_device(device: &str) -> RpcError {
    gateway_error(
        "command.device.unknown",
        format!("device '{device}' is not registered"),
        vec![],
    )
}

/// Also used for commands the caller may not see, so their existence isn't revealed.
fn unknown_command(id: &str) -> RpcError {
    gateway_error("command.unknown", format!("unknown command {id}"), vec![])
}

fn token_error(e: TokenError) -> RpcError {
    match e {
        TokenError::Rejected(error) => {
            gateway_error(&error.code.clone(), error.message.clone(), vec![error])
        }
        TokenError::Storage(e) => io_error(e),
    }
}

fn io_error(e: crate::GatewayError) -> RpcError {
    gateway_error("gateway.storage", e.to_string(), vec![])
}
