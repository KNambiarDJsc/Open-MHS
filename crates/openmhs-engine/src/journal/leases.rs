//! Leases in the journal (ADR-0018). Taking one is a single transaction that checks every
//! live lease for an overlapping claim, so two holders can never get overlapping claims.

use openmhs_types::Timestamp;
use openmhs_types::lease::Lease;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::{Journal, JournalError, refuse};

impl Journal {
    fn load_lease(conn: &Connection, id: &str) -> Result<Lease, JournalError> {
        let json: Option<String> = conn
            .query_row("SELECT record FROM leases WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        match json {
            Some(j) => Ok(serde_json::from_str(&j)?),
            None => Err(refuse("lease.unknown", format!("unknown lease {id}"))),
        }
    }

    fn store_lease(
        tx: &Transaction<'_>,
        l: &Lease,
        ended: Option<Timestamp>,
    ) -> Result<(), JournalError> {
        tx.execute(
            "UPDATE leases SET expires_ms = ?2, ended_ms = ?3, record = ?4 WHERE id = ?1",
            params![
                l.id,
                l.expires_at.millis(),
                ended.map(Timestamp::millis),
                serde_json::to_string(l)?
            ],
        )?;
        Ok(())
    }

    fn live_leases(conn: &Connection, at: Timestamp) -> Result<Vec<Lease>, JournalError> {
        let mut stmt = conn.prepare(
            "SELECT record FROM leases WHERE ended_ms IS NULL AND expires_ms > ?1 ORDER BY created_ms",
        )?;
        let rows = stmt.query_map([at.millis()], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for json in rows {
            out.push(serde_json::from_str(&json?)?);
        }
        Ok(out)
    }

    /// Record a new lease, unless a live lease's claim overlaps its claim
    /// (`lease.conflict`, naming that lease and its holder).
    pub fn acquire_lease(&self, lease: &Lease) -> Result<(), JournalError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        if let Some(other) = Self::live_leases(&tx, lease.created_at)?
            .into_iter()
            .find(|l| l.claim.overlaps(&lease.claim))
        {
            let shared: Vec<&str> = other
                .claim
                .devices
                .intersection(&lease.claim.devices)
                .chain(other.claim.resources.intersection(&lease.claim.resources))
                .map(String::as_str)
                .collect();
            return Err(refuse(
                "lease.conflict",
                format!(
                    "{} is held by {} under lease {} until {}",
                    shared.join(", "),
                    other.holder,
                    other.id,
                    other.expires_at
                ),
            ));
        }
        tx.execute(
            "INSERT INTO leases (id, holder, holder_credential, created_ms, expires_ms, ended_ms, record)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6)",
            params![
                lease.id,
                lease.holder,
                lease.holder_credential,
                lease.created_at.millis(),
                lease.expires_at.millis(),
                serde_json::to_string(lease)?
            ],
        )?;
        Self::append_audit_tx(
            &tx,
            lease.created_at,
            "lease.acquired",
            &lease.id,
            &serde_json::json!({
                "holder": lease.holder, "credential_id": lease.holder_credential,
                "devices": lease.devices, "resources": lease.resources, "claim": lease.claim,
                "plan_id": lease.plan_id, "reason": lease.reason, "expires_at": lease.expires_at,
            }),
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn lease(&self, id: &str) -> Result<Option<Lease>, JournalError> {
        match Self::load_lease(&self.lock(), id) {
            Ok(l) => Ok(Some(l)),
            Err(JournalError::Refused(e)) if e.code == "lease.unknown" => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Leases holding something at `at`.
    pub fn active_leases(&self, at: Timestamp) -> Result<Vec<Lease>, JournalError> {
        Self::live_leases(&self.lock(), at)
    }

    /// Recent leases, newest first.
    pub fn leases(&self, limit: usize) -> Result<Vec<Lease>, JournalError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT record FROM leases ORDER BY created_ms DESC LIMIT ?1")?;
        let rows = stmt.query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |r| {
            r.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for json in rows {
            out.push(serde_json::from_str(&json?)?);
        }
        Ok(out)
    }

    /// Live leases taken with a credential.
    pub fn active_leases_of_credential(
        &self,
        credential_id: &str,
        at: Timestamp,
    ) -> Result<Vec<Lease>, JournalError> {
        Ok(self
            .active_leases(at)?
            .into_iter()
            .filter(|l| l.holder_credential.as_deref() == Some(credential_id))
            .collect())
    }

    /// End a live lease. `lease.ended` if it was already released or had run out.
    pub fn release_lease(
        &self,
        id: &str,
        by: &str,
        at: Timestamp,
        reason: Option<&str>,
    ) -> Result<Lease, JournalError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let mut l = Self::load_lease(&tx, id)?;
        if !l.is_active_at(at) {
            return Err(refuse(
                "lease.ended",
                format!("lease {id} has already ended"),
            ));
        }
        l.released_at = Some(at);
        l.released_by = Some(by.to_owned());
        Self::store_lease(&tx, &l, Some(at))?;
        Self::append_audit_tx(
            &tx,
            at,
            "lease.released",
            id,
            &serde_json::json!({ "holder": l.holder, "by": by, "reason": reason }),
        )?;
        tx.commit()?;
        Ok(l)
    }

    /// Move a live lease's deadline. Only `holder` may.
    pub fn renew_lease(
        &self,
        id: &str,
        holder: &str,
        expires_at: Timestamp,
        at: Timestamp,
    ) -> Result<Lease, JournalError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let mut l = Self::load_lease(&tx, id)?;
        if !l.is_active_at(at) {
            return Err(refuse(
                "lease.ended",
                format!("lease {id} has already ended"),
            ));
        }
        if l.holder != holder {
            return Err(refuse(
                "lease.not_holder",
                format!("lease {id} is held by {}", l.holder),
            ));
        }
        l.expires_at = expires_at;
        Self::store_lease(&tx, &l, None)?;
        Self::append_audit_tx(
            &tx,
            at,
            "lease.renewed",
            id,
            &serde_json::json!({ "holder": holder, "expires_at": expires_at }),
        )?;
        tx.commit()?;
        Ok(l)
    }

    /// Record the end of every lease whose deadline has passed. Leases stop holding at
    /// their deadline whether or not this runs; this only makes the ending visible in the
    /// audit log.
    pub fn expire_leases(&self, at: Timestamp) -> Result<Vec<Lease>, JournalError> {
        let mut conn = self.lock();
        let due: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT record FROM leases WHERE ended_ms IS NULL AND expires_ms <= ?1")?;
            let rows = stmt.query_map([at.millis()], |r| r.get::<_, String>(0))?;
            rows.collect::<Result<_, _>>()?
        };
        if due.is_empty() {
            return Ok(Vec::new());
        }
        let tx = conn.transaction()?;
        let mut out = Vec::new();
        for json in due {
            let l: Lease = serde_json::from_str(&json)?;
            Self::store_lease(&tx, &l, Some(l.expires_at))?;
            Self::append_audit_tx(
                &tx,
                at,
                "lease.expired",
                &l.id,
                &serde_json::json!({ "holder": l.holder, "expired_at": l.expires_at }),
            )?;
            out.push(l);
        }
        tx.commit()?;
        Ok(out)
    }
}
