//! Leases in the engine (ADR-0018, spec/topology.md).
//!
//! A lease gives its holder exclusive use of what it claims: while it is live, writes and
//! invokes on a claimed device by anyone else are denied (`lease.held`). Reads are not
//! affected, and stops are never blocked (the stop lane). A caller may only lease
//! devices it could operate, so a lease cannot be used to lock others out of devices the
//! caller has no business with.

use super::*;

use openmhs_types::lease::{Lease, MAX_LEASE_TTL_MS};

fn denied(code: &str, message: impl Into<String>) -> EngineError {
    EngineError::Denied(ErrorObject::new(code, message))
}

/// What a caller asks to lease.
#[derive(Debug, Clone, Default)]
pub struct LeaseRequest {
    pub devices: Vec<String>,
    pub resources: Vec<String>,
    pub ttl_ms: u64,
    pub reason: Option<String>,
    /// The plan the lease is for, if the plan executor takes it.
    pub plan_id: Option<String>,
}

impl Engine {
    /// Whether `caller` could write or invoke something on `device`: a capability its
    /// scopes reach and its grants do not deny.
    fn can_operate(&self, caller: &Caller, device: &str) -> bool {
        let Some(d) = self.device(device) else {
            return false;
        };
        let policy = self.policy();
        d.descriptor.capabilities.iter().any(|cap| {
            cap.operations
                .iter()
                .filter(|op| matches!(op, Operation::Write | Operation::Invoke))
                .any(|&op| {
                    caller.in_scope(device, &cap.name, op)
                        && policy
                            .authorize(caller.principal(), device, cap, op)
                            .decision
                            != Decision::Deny
                })
        })
    }

    /// Take a lease. All or nothing: refused if any part of its claim is held by a live
    /// lease (`lease.conflict`).
    pub fn acquire_lease(&self, caller: &Caller, req: LeaseRequest) -> Result<Lease, EngineError> {
        let lease = self.prepare_lease(caller, req)?;
        self.inner.journal.acquire_lease(&lease)?;
        Ok(lease)
    }

    /// Whether a lease could be taken now, without taking it: the same checks, and the
    /// live lease that would conflict, if any.
    pub(super) fn lease_would_conflict(
        &self,
        caller: &Caller,
        req: LeaseRequest,
    ) -> Result<(Lease, Option<Lease>), EngineError> {
        let lease = self.prepare_lease(caller, req)?;
        let other = self
            .inner
            .journal
            .active_leases(self.now())?
            .into_iter()
            .find(|l| l.claim.overlaps(&lease.claim));
        Ok((lease, other))
    }

    /// Check a lease request and build the lease it would be.
    fn prepare_lease(&self, caller: &Caller, req: LeaseRequest) -> Result<Lease, EngineError> {
        if req.ttl_ms == 0 || req.ttl_ms > MAX_LEASE_TTL_MS {
            return Err(denied(
                "lease.ttl.invalid",
                format!("ttl_ms must be between 1 and {MAX_LEASE_TTL_MS}"),
            ));
        }
        if req.devices.is_empty() && req.resources.is_empty() {
            return Err(denied(
                "lease.empty",
                "name at least one device or resource",
            ));
        }
        let graph = self.graph();
        for device in &req.devices {
            // Scope first, so a lease request cannot probe for devices.
            if !caller.in_scope_device(device) || self.device(device).is_none() {
                return Err(denied(
                    "lease.device.unknown",
                    format!("no device '{device}' you can reach"),
                ));
            }
            if !self.can_operate(caller, device) {
                return Err(denied(
                    "lease.not_authorized",
                    format!(
                        "'{}' cannot write or invoke anything on '{device}', so it cannot lease it",
                        caller.principal()
                    ),
                ));
            }
        }
        for resource in &req.resources {
            let Some(users) = graph.resources.get(resource) else {
                return Err(denied(
                    "lease.resource.unknown",
                    format!("no device shares a resource '{resource}'"),
                ));
            };
            if let Some(d) = users.iter().find(|d| !self.can_operate(caller, d)) {
                return Err(denied(
                    "lease.not_authorized",
                    format!(
                        "'{}' cannot operate '{d}', which shares '{resource}', so it cannot lease it",
                        caller.principal()
                    ),
                ));
            }
        }
        let claim = graph.claim(
            req.devices.iter().map(String::as_str),
            req.resources.iter().map(String::as_str),
        );
        let now = self.now();
        let lease = Lease {
            id: format!("lse_{}", uuid::Uuid::now_v7().simple()),
            holder: caller.principal().to_owned(),
            holder_credential: caller.credential_id().map(str::to_owned),
            devices: req.devices,
            resources: req.resources,
            claim,
            reason: req.reason,
            plan_id: req.plan_id,
            created_at: now,
            expires_at: now.plus_ms(req.ttl_ms),
            released_at: None,
            released_by: None,
        };
        Ok(lease)
    }

    /// One lease. Every caller can see every lease: knowing who holds a device is how
    /// callers coordinate.
    pub fn lease(&self, id: &str) -> Result<Lease, EngineError> {
        self.inner.journal.expire_leases(self.now())?;
        self.inner
            .journal
            .lease(id)?
            .ok_or_else(|| denied("lease.unknown", format!("unknown lease {id}")))
    }

    /// Live leases, or the most recent ones of any status.
    pub fn leases(&self, active_only: bool, limit: usize) -> Result<Vec<Lease>, EngineError> {
        let now = self.now();
        self.inner.journal.expire_leases(now)?;
        Ok(if active_only {
            self.inner.journal.active_leases(now)?
        } else {
            self.inner.journal.leases(limit)?
        })
    }

    /// Move a live lease's deadline to `ttl_ms` from now. Holder only.
    pub fn renew_lease(
        &self,
        caller: &Caller,
        id: &str,
        ttl_ms: u64,
    ) -> Result<Lease, EngineError> {
        if ttl_ms == 0 || ttl_ms > MAX_LEASE_TTL_MS {
            return Err(denied(
                "lease.ttl.invalid",
                format!("ttl_ms must be between 1 and {MAX_LEASE_TTL_MS}"),
            ));
        }
        let now = self.now();
        Ok(self
            .inner
            .journal
            .renew_lease(id, caller.principal(), now.plus_ms(ttl_ms), now)?)
    }

    /// End a live lease: its holder, or a human approver (to free devices a holder left
    /// leased).
    pub fn release_lease(
        &self,
        caller: &Caller,
        id: &str,
        reason: Option<&str>,
    ) -> Result<Lease, EngineError> {
        let lease = self.lease(id)?;
        let approver =
            caller.kind() != PrincipalKind::Agent && self.policy().is_approver(caller.principal());
        if lease.holder != caller.principal() && !approver {
            return Err(denied(
                "lease.not_holder",
                format!("lease {id} is held by {}", lease.holder),
            ));
        }
        Ok(self
            .inner
            .journal
            .release_lease(id, caller.principal(), self.now(), reason)?)
    }

    /// Release every live lease taken with a credential, because it was revoked.
    pub fn release_credential_leases(
        &self,
        credential_id: &str,
        by: &str,
    ) -> Result<Vec<String>, EngineError> {
        let now = self.now();
        let mut released = Vec::new();
        for l in self
            .inner
            .journal
            .active_leases_of_credential(credential_id, now)?
        {
            let reason = format!("credential {credential_id} revoked");
            if self
                .inner
                .journal
                .release_lease(&l.id, by, now, Some(&reason))
                .is_ok()
            {
                released.push(l.id);
            }
        }
        Ok(released)
    }

    /// The live lease that keeps `principal` from changing `device`, if any.
    fn blocking_lease(&self, principal: &str, device: &str) -> Result<Option<Lease>, EngineError> {
        Ok(self
            .inner
            .journal
            .active_leases(self.now())?
            .into_iter()
            .find(|l| l.holder != principal && l.claim.devices.contains(device)))
    }

    /// Why `principal` may not change `device` now because of a lease: someone else's
    /// lease claims it, or the leases cannot be read (then nothing runs).
    pub(super) fn lease_refusal(&self, principal: &str, device: &str) -> Option<ErrorObject> {
        match self.blocking_lease(principal, device) {
            Ok(None) => None,
            Ok(Some(l)) => Some(ErrorObject::new(
                "lease.held",
                format!(
                    "'{device}' is leased to {} (lease {}) until {}",
                    l.holder, l.id, l.expires_at
                ),
            )),
            Err(e) => Some(ErrorObject::new(
                "gateway.journal",
                format!("could not check leases: {e}"),
            )),
        }
    }
}
