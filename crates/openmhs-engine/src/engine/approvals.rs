//! Approvals in the engine (ADR-0017).
//!
//! Two ways in, one record. A command that policy holds for approval gets an approval
//! created for it (bound to that command). Or a requester asks first
//! ([`Engine::request_approval`]), an approver approves, and the requester then submits the
//! same command naming the approval ([`Engine::submit_approved`]). Either way the approval
//! covers one exact command (its fingerprint), runs out, and is spent exactly once,
//! atomically with the command moving on. Policy is still evaluated at the moment of use.

use super::*;

use openmhs_types::{Approval, ApprovalStatus, canonical_json};

/// How long an approval requested ahead of a command stays valid unless the requester asks
/// for less.
pub const DEFAULT_APPROVAL_TTL_MS: u64 = 15 * 60 * 1_000;
/// The longest an approval can be asked to live.
pub const MAX_APPROVAL_TTL_MS: u64 = 24 * 60 * 60 * 1_000;

/// What an approval covers: who asked, and the target, operation, value and parameters.
/// Idempotency key, correlation id and deadline are about delivery, not meaning, so they
/// are not part of it.
pub fn fingerprint(requester: &str, request: &CommandRequest) -> String {
    let covered = serde_json::json!({
        "v": 1,
        "requester": requester,
        "device": request.target.device,
        "capability": request.target.capability,
        "operation": request.operation,
        "value": request.value,
        "params": request.params,
    });
    sha256_hex(&canonical_json(&covered))
}

fn denied(code: &str, message: impl Into<String>) -> EngineError {
    EngineError::Denied(ErrorObject::new(code, message))
}

impl Engine {
    fn new_approval(
        &self,
        requester: &str,
        credential: Option<&str>,
        request: &CommandRequest,
        command_id: Option<String>,
        expires_at: Timestamp,
    ) -> Approval {
        Approval {
            id: format!("apr_{}", uuid::Uuid::now_v7().simple()),
            status: ApprovalStatus::Pending,
            requester: requester.to_owned(),
            requester_credential: credential.map(str::to_owned),
            decided_by: None,
            device: request.target.device.clone(),
            capability: request.target.capability.clone(),
            operation: request.operation,
            fingerprint: fingerprint(requester, request),
            command_id,
            reason: None,
            created_at: self.now(),
            expires_at,
            decided_at: None,
            consumed_at: None,
        }
    }

    /// The approval for a command that policy holds. Created before the command waits, so
    /// there is never a waiting command without one.
    pub(super) fn open_approval_for(
        &self,
        credential: Option<&str>,
        receipt: &Receipt,
    ) -> Result<Approval, EngineError> {
        let expires_at = receipt.deadline_at.unwrap_or_else(|| self.now());
        let a = self.new_approval(
            &receipt.principal,
            credential,
            &receipt.request,
            Some(receipt.command_id.clone()),
            expires_at,
        );
        self.inner.journal.insert_approval(&a)?;
        Ok(a)
    }

    /// Ask for approval of a command before submitting it. Everything a submission would
    /// check is checked now, so an approval is only issued for a command that policy
    /// holds for approval and that is otherwise acceptable.
    pub async fn request_approval(
        &self,
        caller: &Caller,
        request: &serde_json::Value,
        ttl_ms: Option<u64>,
    ) -> Result<Approval, EngineError> {
        let issues = schema::validate(Schema::Command, request);
        if !issues.is_empty() {
            return Err(EngineError::InvalidRequest(
                issues.into_iter().map(Into::into).collect(),
            ));
        }
        let req: CommandRequest = serde_json::from_value(request.clone()).map_err(|e| {
            EngineError::InvalidRequest(vec![ErrorObject::new("command.schema", e.to_string())])
        })?;
        if req.operation == RequestOperation::Read {
            return Err(EngineError::NotACommand);
        }
        let ttl = ttl_ms.unwrap_or(DEFAULT_APPROVAL_TTL_MS);
        if ttl == 0 || ttl > MAX_APPROVAL_TTL_MS {
            return Err(denied(
                "approval.ttl.invalid",
                format!("ttl_ms must be between 1 and {MAX_APPROVAL_TTL_MS}"),
            ));
        }
        match self.evaluate(caller, &req).await {
            Evaluation::Denied { errors, .. } => {
                let first = errors
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| ErrorObject::new("policy.denied.no_grant", "not allowed"));
                return Err(EngineError::Denied(first));
            }
            Evaluation::Ready { decision, .. } if decision != Decision::RequireApproval => {
                return Err(denied(
                    "approval.not_required",
                    "policy allows this command without approval; submit it directly",
                ));
            }
            Evaluation::Ready { .. } => {}
        }
        let a = self.new_approval(
            caller.principal(),
            caller.credential_id(),
            &req,
            None,
            self.now().plus_ms(ttl),
        );
        self.inner.journal.insert_approval(&a)?;
        Ok(a)
    }

    /// Expire what is due: approvals past their time are `EXPIRED`, and a command still
    /// waiting on one is `EXPIRED` with it.
    pub fn sweep_approvals(&self) -> Result<Vec<Approval>, EngineError> {
        let expired = self.inner.journal.expire_approvals(self.now())?;
        for a in &expired {
            self.end_waiting_command(a, Expired, "approval expired while waiting", None);
        }
        Ok(expired)
    }

    /// Move the command an approval was bound to out of `AWAITING_APPROVAL`, if it is
    /// still there.
    fn end_waiting_command(
        &self,
        a: &Approval,
        to: CommandState,
        detail: &str,
        error: Option<ErrorObject>,
    ) {
        let Some(command_id) = a.command_id.as_deref() else {
            return;
        };
        let waiting =
            matches!(self.receipt(command_id), Ok(Some(r)) if r.state == AwaitingApproval);
        if waiting {
            let result = self.transition(command_id, to, Some(detail.to_owned()), |r| {
                if let Some(e) = error {
                    r.policy.decision = Decision::Deny;
                    r.policy.errors = vec![e];
                }
            });
            if let Err(e) = result {
                warn!(command = %command_id, error = %e, "could not move a command whose approval ended");
            }
        }
    }

    fn visible(&self, caller: &Caller, a: &Approval) -> bool {
        a.requester == caller.principal() || self.policy().is_approver(caller.principal())
    }

    /// One approval. Requesters see their own; approvers see all. Anything else looks
    /// like it does not exist.
    pub fn approval(&self, caller: &Caller, id: &str) -> Result<Approval, EngineError> {
        self.sweep_approvals()?;
        self.inner
            .journal
            .approval(id)?
            .filter(|a| self.visible(caller, a))
            .ok_or_else(|| denied("approval.unknown", format!("unknown approval {id}")))
    }

    pub fn approvals(
        &self,
        caller: &Caller,
        statuses: &[ApprovalStatus],
        limit: usize,
    ) -> Result<Vec<Approval>, EngineError> {
        self.sweep_approvals()?;
        let all = self.policy().is_approver(caller.principal());
        let own = (!all).then_some(caller.principal());
        Ok(self.inner.journal.approvals(own, statuses, limit)?)
    }

    /// Who may approve or deny: an approver, not an agent, not the requester, and with a
    /// credential that reaches what is being decided.
    fn check_can_decide(&self, caller: &Caller, a: &Approval) -> Result<(), EngineError> {
        if caller.kind() == PrincipalKind::Agent {
            return Err(denied(
                "policy.approval.not_human",
                "an agent credential cannot decide approvals",
            ));
        }
        if !self.policy().is_approver(caller.principal()) {
            return Err(denied(
                "policy.approval.not_approver",
                format!("'{}' is not an approver", caller.principal()),
            ));
        }
        if caller.principal() == a.requester {
            return Err(denied(
                "policy.approval.self",
                "a principal cannot approve its own command",
            ));
        }
        let operation: Operation = a.operation.into();
        if !caller.in_scope(&a.device, &a.capability, operation) {
            return Err(EngineError::Denied(super::scope_denied(
                caller,
                &a.device,
                &a.capability,
                operation,
            )));
        }
        Ok(())
    }

    /// Approve. For an approval bound to a waiting command this is
    /// [`Engine::approve`]: the command runs, spending the approval. For one requested
    /// ahead of time it only records the decision; the requester spends it by submitting.
    pub async fn approve_approval(
        &self,
        caller: &Caller,
        id: &str,
    ) -> Result<Approval, EngineError> {
        self.sweep_approvals()?;
        let a = self
            .inner
            .journal
            .approval(id)?
            .ok_or_else(|| denied("approval.unknown", format!("unknown approval {id}")))?;
        self.check_can_decide(caller, &a)?;
        if a.status.is_final() {
            let (code, why) = match a.status {
                ApprovalStatus::Denied => ("approval.denied", "was denied"),
                ApprovalStatus::Expired => ("approval.expired", "has expired"),
                ApprovalStatus::Revoked => ("approval.revoked", "was revoked"),
                _ => ("approval.consumed", "was already used"),
            };
            return Err(denied(code, format!("approval {id} {why}")));
        }
        if let Some(command_id) = a.command_id.as_deref() {
            self.approve(caller, command_id).await?;
        } else {
            self.inner.journal.decide_approval(
                id,
                ApprovalStatus::Approved,
                caller.principal(),
                self.now(),
                None,
            )?;
        }
        Ok(self.inner.journal.approval(id)?.unwrap_or(a))
    }

    /// Deny a pending approval. A command waiting on it is denied too.
    pub fn deny_approval(
        &self,
        caller: &Caller,
        id: &str,
        reason: Option<String>,
    ) -> Result<Approval, EngineError> {
        self.sweep_approvals()?;
        let a = self
            .inner
            .journal
            .approval(id)?
            .ok_or_else(|| denied("approval.unknown", format!("unknown approval {id}")))?;
        self.check_can_decide(caller, &a)?;
        let a = self.inner.journal.decide_approval(
            id,
            ApprovalStatus::Denied,
            caller.principal(),
            self.now(),
            reason,
        )?;
        let why = ErrorObject::new(
            "approval.denied",
            format!("approval {id} was denied by {}", caller.principal()),
        );
        self.end_waiting_command(&a, Denied, &why.message.clone(), Some(why));
        Ok(a)
    }

    /// Withdraw an approval that is pending or approved but not yet spent. The requester
    /// can withdraw their own; an approver can revoke any. A command waiting on it is
    /// cancelled.
    pub fn revoke_approval(
        &self,
        caller: &Caller,
        id: &str,
        reason: Option<String>,
    ) -> Result<Approval, EngineError> {
        self.sweep_approvals()?;
        let a = self
            .inner
            .journal
            .approval(id)?
            .ok_or_else(|| denied("approval.unknown", format!("unknown approval {id}")))?;
        let is_requester = a.requester == caller.principal();
        if !is_requester {
            self.check_can_decide(caller, &a)?;
        }
        let a = self.inner.journal.decide_approval(
            id,
            ApprovalStatus::Revoked,
            caller.principal(),
            self.now(),
            reason,
        )?;
        self.end_waiting_command(
            &a,
            Cancelled,
            &format!("approval revoked by {}", caller.principal()),
            None,
        );
        Ok(a)
    }

    /// Withdraw everything a credential asked for, because the credential was revoked:
    /// its open approvals are revoked and the commands waiting on them cancelled.
    /// Commands already dispatched are not recalled.
    pub fn revoke_credential_approvals(
        &self,
        credential_id: &str,
        by: &str,
    ) -> Result<Vec<String>, EngineError> {
        let mut revoked = Vec::new();
        for open in self
            .inner
            .journal
            .open_approvals_of_credential(credential_id)?
        {
            let a = self.inner.journal.decide_approval(
                &open.id,
                ApprovalStatus::Revoked,
                by,
                self.now(),
                Some(format!("credential {credential_id} revoked")),
            );
            if let Ok(a) = a {
                self.end_waiting_command(
                    &a,
                    Cancelled,
                    "the requester's credential was revoked",
                    None,
                );
                revoked.push(a.id);
            }
        }
        Ok(revoked)
    }
}
