//! Durable command journal and hash-chained audit log (SQLite).
//!
//! Every lifecycle transition is one SQLite transaction that updates the command's
//! receipt and appends an audit record, so the journal and the audit log can't disagree.
//! The audit log is append-only by convention and tamper-evident by construction: each
//! record's hash covers the previous record's hash.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use openmhs_policy::Expectation;
use openmhs_types::receipt::TransitionRecord;
use openmhs_types::{Approval, ApprovalStatus, CommandState, ErrorObject, Receipt, Timestamp};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};

mod leases;
mod plans;

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("illegal transition {from:?} -> {to:?}")]
    IllegalTransition {
        from: CommandState,
        to: CommandState,
    },
    #[error("unknown command {0}")]
    NotFound(String),
    /// An approval was refused: unknown, not approved, expired, already spent, or it does
    /// not match the command. The error says which.
    #[error("{0}")]
    Refused(ErrorObject),
}

fn refuse(code: &str, message: impl Into<String>) -> JournalError {
    JournalError::Refused(ErrorObject::new(code, message))
}

/// Why an approval in `status` cannot be used (or moved) right now.
fn unusable(id: &str, status: ApprovalStatus) -> JournalError {
    let (code, why) = match status {
        ApprovalStatus::Pending => ("approval.pending", "has not been approved yet"),
        ApprovalStatus::Approved => ("approval.invalid_transition", "is already approved"),
        ApprovalStatus::Denied => ("approval.denied", "was denied"),
        ApprovalStatus::Expired => ("approval.expired", "has expired"),
        ApprovalStatus::Revoked => ("approval.revoked", "was revoked"),
        ApprovalStatus::Consumed => ("approval.consumed", "was already used"),
    };
    refuse(code, format!("approval {id} {why}"))
}

/// Result of inserting a new command.
#[derive(Debug)]
pub enum Inserted {
    New(Receipt),
    /// A command with the same (principal, device, idempotency key) already exists.
    Existing {
        receipt: Receipt,
        request_hash: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    pub seq: i64,
    pub at_ms: i64,
    pub kind: String,
    pub subject: String,
    pub data: String,
    pub prev_hash: String,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditVerification {
    pub records: u64,
    /// Sequence number of the first record that fails verification, if any.
    pub first_invalid: Option<i64>,
}

impl AuditVerification {
    pub fn is_valid(&self) -> bool {
        self.first_invalid.is_none()
    }
}

pub struct Journal {
    conn: Mutex<Connection>,
}

fn hash_record(prev: &str, seq: i64, at_ms: i64, kind: &str, subject: &str, data: &str) -> String {
    let mut h = Sha256::new();
    h.update(format!("{prev}\n{seq}\n{at_ms}\n{kind}\n{subject}\n{data}").as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(data: &str) -> String {
    Sha256::digest(data.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Journal {
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self, JournalError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, JournalError> {
        // FULL: an intent record must survive power loss once the call returns.
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS commands (
                command_id      TEXT PRIMARY KEY,
                principal       TEXT NOT NULL,
                device          TEXT NOT NULL,
                capability      TEXT NOT NULL,
                idempotency_key TEXT,
                request_hash    TEXT NOT NULL,
                state           TEXT NOT NULL,
                created_ms      INTEGER NOT NULL,
                receipt         TEXT NOT NULL,
                expectation     TEXT
            );
            CREATE UNIQUE INDEX IF NOT EXISTS commands_idempotency
                ON commands(principal, device, idempotency_key)
                WHERE idempotency_key IS NOT NULL;
            CREATE INDEX IF NOT EXISTS commands_state ON commands(state);
            CREATE TABLE IF NOT EXISTS approvals (
                id                   TEXT PRIMARY KEY,
                status               TEXT NOT NULL,
                requester            TEXT NOT NULL,
                requester_credential TEXT,
                command_id           TEXT,
                created_ms           INTEGER NOT NULL,
                expires_ms           INTEGER NOT NULL,
                record               TEXT NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS approvals_command
                ON approvals(command_id) WHERE command_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS approvals_status ON approvals(status);
            CREATE TABLE IF NOT EXISTS leases (
                id                TEXT PRIMARY KEY,
                holder            TEXT NOT NULL,
                holder_credential TEXT,
                created_ms        INTEGER NOT NULL,
                expires_ms        INTEGER NOT NULL,
                ended_ms          INTEGER,
                record            TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS leases_open ON leases(ended_ms, expires_ms);
            CREATE TABLE IF NOT EXISTS plans (
                id            TEXT PRIMARY KEY,
                principal     TEXT NOT NULL,
                credential_id TEXT,
                state         TEXT NOT NULL,
                created_ms    INTEGER NOT NULL,
                updated_ms    INTEGER NOT NULL,
                caller        TEXT NOT NULL,
                record        TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS plans_state ON plans(state);
            CREATE TABLE IF NOT EXISTS plan_commands (
                command_id TEXT PRIMARY KEY,
                plan_id    TEXT NOT NULL,
                step_id    TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS audit (
                seq       INTEGER PRIMARY KEY,
                at_ms     INTEGER NOT NULL,
                kind      TEXT NOT NULL,
                subject   TEXT NOT NULL,
                data      TEXT NOT NULL,
                prev_hash TEXT NOT NULL,
                hash      TEXT NOT NULL
            );",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        // Poisoning means a panic while holding the lock; SQLite transactions roll back
        // on drop, so the database itself is consistent.
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn append_audit_tx(
        tx: &Transaction<'_>,
        at: Timestamp,
        kind: &str,
        subject: &str,
        data: &serde_json::Value,
    ) -> Result<String, JournalError> {
        let last: Option<(i64, String)> = tx
            .query_row(
                "SELECT seq, hash FROM audit ORDER BY seq DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (seq, prev) = match last {
            Some((s, h)) => (s + 1, h),
            None => (1, GENESIS.to_owned()),
        };
        let data = serde_json::to_string(data)?;
        let hash = hash_record(&prev, seq, at.millis(), kind, subject, &data);
        tx.execute(
            "INSERT INTO audit (seq, at_ms, kind, subject, data, prev_hash, hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![seq, at.millis(), kind, subject, data, prev, hash],
        )?;
        Ok(hash)
    }

    /// Append a standalone audit record (reads, registrations, rejected requests).
    pub fn append_audit(
        &self,
        at: Timestamp,
        kind: &str,
        subject: &str,
        data: &serde_json::Value,
    ) -> Result<String, JournalError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let hash = Self::append_audit_tx(&tx, at, kind, subject, data)?;
        tx.commit()?;
        Ok(hash)
    }

    /// Insert a new command in `CREATED`, or return the existing one for the same
    /// (principal, device, idempotency key). `origin` records how the requester was
    /// authenticated and goes into the `command.created` audit record.
    pub fn insert_command(
        &self,
        receipt: &Receipt,
        request_hash: &str,
        origin: &serde_json::Value,
    ) -> Result<Inserted, JournalError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let target = &receipt.request.target;
        if let Some(key) = &receipt.idempotency_key {
            let existing: Option<(String, String)> = tx
                .query_row(
                    "SELECT receipt, request_hash FROM commands WHERE principal = ?1 AND device = ?2 AND idempotency_key = ?3",
                    params![receipt.principal, target.device, key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((json, request_hash)) = existing {
                return Ok(Inserted::Existing {
                    receipt: serde_json::from_str(&json)?,
                    request_hash,
                });
            }
        }
        let mut r = receipt.clone();
        let hash = Self::append_audit_tx(
            &tx,
            r.created_at,
            "command.created",
            &r.command_id,
            &serde_json::json!({ "principal": r.principal, "request": r.request, "request_hash": request_hash, "origin": origin }),
        )?;
        r.audit_hash = Some(hash);
        tx.execute(
            "INSERT INTO commands (command_id, principal, device, capability, idempotency_key, request_hash, state, created_ms, receipt)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                r.command_id,
                r.principal,
                target.device,
                target.capability,
                r.idempotency_key,
                request_hash,
                state_name(r.state),
                r.created_at.millis(),
                serde_json::to_string(&r)?
            ],
        )?;
        tx.commit()?;
        Ok(Inserted::New(r))
    }

    /// Apply one lifecycle transition atomically: legality check, receipt update, audit.
    pub fn transition(
        &self,
        command_id: &str,
        to: CommandState,
        at: Timestamp,
        detail: Option<String>,
        update: impl FnOnce(&mut Receipt),
    ) -> Result<Receipt, JournalError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let r = Self::transition_tx(&tx, command_id, to, at, detail, update)?;
        tx.commit()?;
        Ok(r)
    }

    fn transition_tx(
        tx: &Transaction<'_>,
        command_id: &str,
        to: CommandState,
        at: Timestamp,
        detail: Option<String>,
        update: impl FnOnce(&mut Receipt),
    ) -> Result<Receipt, JournalError> {
        let mut r = Self::load(tx, command_id)?;
        let from = r.state;
        if !from.can_transition_to(to) {
            return Err(JournalError::IllegalTransition { from, to });
        }
        update(&mut r);
        r.state = to;
        r.outcome = to.outcome();
        r.transitions.push(TransitionRecord {
            state: to,
            at,
            detail: detail.clone(),
        });
        let hash = Self::append_audit_tx(
            tx,
            at,
            "command.transition",
            command_id,
            &serde_json::json!({ "from": from, "to": to, "detail": detail }),
        )?;
        r.audit_hash = Some(hash);
        tx.execute(
            "UPDATE commands SET state = ?1, receipt = ?2 WHERE command_id = ?3",
            params![state_name(to), serde_json::to_string(&r)?, command_id],
        )?;
        Ok(r)
    }

    // ---------------------------------------------------------------- approvals
    //
    // Every change to an approval is one SQLite transaction under the connection mutex:
    // the status check, the update and its audit record happen together or not at all.
    // Spending an approval additionally moves the command in the same transaction, so
    // "approved once" and "command proceeds" cannot come apart, and a second attempt to
    // spend it sees CONSUMED.

    fn load_approval(conn: &Connection, id: &str) -> Result<Approval, JournalError> {
        let json: Option<String> = conn
            .query_row("SELECT record FROM approvals WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        let json =
            json.ok_or_else(|| refuse("approval.unknown", format!("unknown approval {id}")))?;
        Ok(serde_json::from_str(&json)?)
    }

    fn store_approval(tx: &Transaction<'_>, a: &Approval) -> Result<(), JournalError> {
        tx.execute(
            "UPDATE approvals SET status = ?1, command_id = ?2, record = ?3 WHERE id = ?4",
            params![
                a.status.name(),
                a.command_id,
                serde_json::to_string(a)?,
                a.id
            ],
        )?;
        Ok(())
    }

    /// Move an approval to `to`, stamping who and when, and audit it. The caller has
    /// already checked legality.
    fn apply_approval(
        tx: &Transaction<'_>,
        a: &mut Approval,
        to: ApprovalStatus,
        at: Timestamp,
        by: Option<&str>,
        reason: Option<String>,
    ) -> Result<(), JournalError> {
        a.status = to;
        match to {
            ApprovalStatus::Approved | ApprovalStatus::Denied | ApprovalStatus::Revoked => {
                a.decided_by = by.map(str::to_owned);
                a.decided_at = Some(at);
            }
            ApprovalStatus::Consumed => a.consumed_at = Some(at),
            ApprovalStatus::Pending | ApprovalStatus::Expired => {}
        }
        if reason.is_some() {
            a.reason = reason;
        }
        Self::store_approval(tx, a)?;
        Self::append_audit_tx(
            tx,
            at,
            &format!("approval.{}", to.name().to_lowercase()),
            &a.id,
            &serde_json::json!({
                "by": by, "requester": a.requester, "command_id": a.command_id,
                "device": a.device, "capability": a.capability, "fingerprint": a.fingerprint,
                "reason": a.reason,
            }),
        )?;
        Ok(())
    }

    /// Record a new approval request in `PENDING`.
    pub fn insert_approval(&self, a: &Approval) -> Result<(), JournalError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO approvals (id, status, requester, requester_credential, command_id, created_ms, expires_ms, record)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                a.id,
                a.status.name(),
                a.requester,
                a.requester_credential,
                a.command_id,
                a.created_at.millis(),
                a.expires_at.millis(),
                serde_json::to_string(a)?
            ],
        )?;
        Self::append_audit_tx(
            &tx,
            a.created_at,
            "approval.requested",
            &a.id,
            &serde_json::json!({
                "requester": a.requester, "credential_id": a.requester_credential,
                "command_id": a.command_id, "device": a.device, "capability": a.capability,
                "operation": a.operation, "fingerprint": a.fingerprint, "expires_at": a.expires_at,
            }),
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn approval(&self, id: &str) -> Result<Option<Approval>, JournalError> {
        match Self::load_approval(&self.lock(), id) {
            Ok(a) => Ok(Some(a)),
            Err(JournalError::Refused(e)) if e.code == "approval.unknown" => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn approval_for_command(&self, command_id: &str) -> Result<Option<Approval>, JournalError> {
        let json: Option<String> = self
            .lock()
            .query_row(
                "SELECT record FROM approvals WHERE command_id = ?1",
                [command_id],
                |r| r.get(0),
            )
            .optional()?;
        json.map(|j| serde_json::from_str(&j).map_err(Into::into))
            .transpose()
    }

    /// Newest first, optionally only one requester's and only the given statuses.
    pub fn approvals(
        &self,
        requester: Option<&str>,
        statuses: &[ApprovalStatus],
        limit: usize,
    ) -> Result<Vec<Approval>, JournalError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT record, status, requester FROM approvals ORDER BY created_ms DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let wanted: Vec<&str> = statuses.iter().map(|s| s.name()).collect();
        let mut out = Vec::new();
        for row in rows {
            let (json, status, who) = row?;
            if requester.is_some_and(|p| p != who)
                || (!wanted.is_empty() && !wanted.contains(&status.as_str()))
            {
                continue;
            }
            out.push(serde_json::from_str(&json)?);
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Approvals not yet final that a credential asked for.
    pub fn open_approvals_of_credential(
        &self,
        credential_id: &str,
    ) -> Result<Vec<Approval>, JournalError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT record FROM approvals WHERE requester_credential = ?1 AND status IN ('PENDING', 'APPROVED')",
        )?;
        let rows = stmt.query_map([credential_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for json in rows {
            out.push(serde_json::from_str(&json?)?);
        }
        Ok(out)
    }

    /// Decide an approval: `Approved`, `Denied` or `Revoked`. An approval past its expiry
    /// is marked `Expired` instead and the decision refused.
    pub fn decide_approval(
        &self,
        id: &str,
        to: ApprovalStatus,
        by: &str,
        at: Timestamp,
        reason: Option<String>,
    ) -> Result<Approval, JournalError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let mut a = Self::load_approval(&tx, id)?;
        if !a.status.is_final() && a.is_expired_at(at) {
            Self::apply_approval(&tx, &mut a, ApprovalStatus::Expired, at, None, None)?;
            tx.commit()?;
            return Err(unusable(id, ApprovalStatus::Expired));
        }
        if !a.status.can_transition_to(to) {
            return Err(unusable(id, a.status));
        }
        Self::apply_approval(&tx, &mut a, to, at, Some(by), reason)?;
        tx.commit()?;
        Ok(a)
    }

    /// Mark every open approval past its expiry as `EXPIRED`; returns them.
    pub fn expire_approvals(&self, at: Timestamp) -> Result<Vec<Approval>, JournalError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let ids: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM approvals WHERE status IN ('PENDING', 'APPROVED') AND expires_ms <= ?1",
            )?;
            let rows = stmt.query_map([at.millis()], |r| r.get::<_, String>(0))?;
            rows.collect::<Result<_, _>>()?
        };
        let mut out = Vec::new();
        for id in ids {
            let mut a = Self::load_approval(&tx, &id)?;
            Self::apply_approval(&tx, &mut a, ApprovalStatus::Expired, at, None, None)?;
            out.push(a);
        }
        tx.commit()?;
        Ok(out)
    }

    /// Spend an approved approval on a command and move that command, atomically.
    ///
    /// Everything is checked against the stored records inside the transaction: the
    /// approval is `APPROVED` and unexpired, it belongs to this requester, it is not bound
    /// to another command, and `fingerprint_of` (computed from the command as stored
    /// now, not as it was when asked about) equals the fingerprint that was approved.
    /// Any mismatch spends nothing. A second call finds `CONSUMED` and fails.
    #[allow(clippy::too_many_arguments)]
    pub fn consume_approval(
        &self,
        approval_id: &str,
        command_id: &str,
        at: Timestamp,
        fingerprint_of: impl FnOnce(&Receipt) -> String,
        to: CommandState,
        detail: Option<String>,
        update: impl FnOnce(&mut Receipt, &Approval),
    ) -> Result<(Approval, Receipt), JournalError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let mut a = Self::load_approval(&tx, approval_id)?;
        if a.status != ApprovalStatus::Approved {
            return Err(unusable(approval_id, a.status));
        }
        if a.is_expired_at(at) {
            Self::apply_approval(&tx, &mut a, ApprovalStatus::Expired, at, None, None)?;
            tx.commit()?;
            return Err(unusable(approval_id, ApprovalStatus::Expired));
        }
        let receipt = Self::load(&tx, command_id)?;
        let bound_elsewhere = a.command_id.as_deref().is_some_and(|c| c != command_id);
        if bound_elsewhere
            || receipt.principal != a.requester
            || fingerprint_of(&receipt) != a.fingerprint
        {
            return Err(refuse(
                "approval.mismatch",
                format!("approval {approval_id} was not given for this command"),
            ));
        }
        a.command_id = Some(command_id.to_owned());
        Self::apply_approval(&tx, &mut a, ApprovalStatus::Consumed, at, None, None)?;
        let approved = a.clone();
        let r = Self::transition_tx(&tx, command_id, to, at, detail, |r| update(r, &approved))?;
        tx.commit()?;
        Ok((a, r))
    }

    fn load(conn: &Connection, command_id: &str) -> Result<Receipt, JournalError> {
        let json: Option<String> = conn
            .query_row(
                "SELECT receipt FROM commands WHERE command_id = ?1",
                [command_id],
                |r| r.get(0),
            )
            .optional()?;
        let json = json.ok_or_else(|| JournalError::NotFound(command_id.to_owned()))?;
        Ok(serde_json::from_str(&json)?)
    }

    pub fn receipt(&self, command_id: &str) -> Result<Option<Receipt>, JournalError> {
        match Self::load(&self.lock(), command_id) {
            Ok(r) => Ok(Some(r)),
            Err(JournalError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn set_expectation(
        &self,
        command_id: &str,
        expectation: &Expectation,
    ) -> Result<(), JournalError> {
        self.lock().execute(
            "UPDATE commands SET expectation = ?1 WHERE command_id = ?2",
            params![serde_json::to_string(expectation)?, command_id],
        )?;
        Ok(())
    }

    pub fn expectation(&self, command_id: &str) -> Result<Option<Expectation>, JournalError> {
        let json: Option<Option<String>> = self
            .lock()
            .query_row(
                "SELECT expectation FROM commands WHERE command_id = ?1",
                [command_id],
                |r| r.get(0),
            )
            .optional()?;
        match json.flatten() {
            Some(j) => Ok(Some(serde_json::from_str(&j)?)),
            None => Ok(None),
        }
    }

    /// All commands currently in one of `states`, oldest first.
    pub fn commands_in_states(
        &self,
        states: &[CommandState],
    ) -> Result<Vec<Receipt>, JournalError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT receipt, state FROM commands ORDER BY created_ms, command_id")?;
        let names: Vec<&str> = states.iter().map(|s| state_name(*s)).collect();
        let mut out = Vec::new();
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (json, state) = row?;
            if names.contains(&state.as_str()) {
                out.push(serde_json::from_str(&json)?);
            }
        }
        Ok(out)
    }

    /// Most recent commands first, optionally filtered by principal and state (an empty
    /// `states` means any state).
    pub fn list_commands(
        &self,
        principal: Option<&str>,
        states: &[CommandState],
        limit: usize,
    ) -> Result<Vec<Receipt>, JournalError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT receipt, state FROM commands
             WHERE (?1 IS NULL OR principal = ?1)
             ORDER BY created_ms DESC, command_id DESC",
        )?;
        let names: Vec<&str> = states.iter().map(|s| state_name(*s)).collect();
        let mut out = Vec::new();
        for row in stmt.query_map(params![principal], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (json, state) = row?;
            if names.is_empty() || names.contains(&state.as_str()) {
                out.push(serde_json::from_str(&json)?);
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    pub fn audit_records(&self) -> Result<Vec<AuditRecord>, JournalError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT seq, at_ms, kind, subject, data, prev_hash, hash FROM audit ORDER BY seq",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(AuditRecord {
                seq: r.get(0)?,
                at_ms: r.get(1)?,
                kind: r.get(2)?,
                subject: r.get(3)?,
                data: r.get(4)?,
                prev_hash: r.get(5)?,
                hash: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Recompute the hash chain. Detects edited, reordered, inserted or deleted records
    /// (deleting the newest records is only detectable against a separately kept head hash).
    pub fn verify_audit(&self) -> Result<AuditVerification, JournalError> {
        let records = self.audit_records()?;
        let mut prev = GENESIS.to_owned();
        for (expected_seq, rec) in (1i64..).zip(&records) {
            let recomputed = hash_record(
                &prev,
                rec.seq,
                rec.at_ms,
                &rec.kind,
                &rec.subject,
                &rec.data,
            );
            if rec.seq != expected_seq || rec.prev_hash != prev || rec.hash != recomputed {
                return Ok(AuditVerification {
                    records: records.len() as u64,
                    first_invalid: Some(rec.seq),
                });
            }
            prev = rec.hash.clone();
        }
        Ok(AuditVerification {
            records: records.len() as u64,
            first_invalid: None,
        })
    }

    /// Run raw SQL against the journal. Tests use this to simulate tampering.
    #[doc(hidden)]
    pub fn execute_raw(&self, sql: &str) -> Result<usize, JournalError> {
        Ok(self.lock().execute(sql, [])?)
    }
}

fn state_name(s: CommandState) -> &'static str {
    use CommandState::*;
    match s {
        Created => "CREATED",
        Validating => "VALIDATING",
        Denied => "DENIED",
        AwaitingApproval => "AWAITING_APPROVAL",
        Approved => "APPROVED",
        Expired => "EXPIRED",
        Cancelled => "CANCELLED",
        Dispatched => "DISPATCHED",
        Accepted => "ACCEPTED",
        Rejected => "REJECTED",
        Failed => "FAILED",
        TimedOut => "TIMED_OUT",
        Applied => "APPLIED",
        Verified => "VERIFIED",
        VerificationFailed => "VERIFICATION_FAILED",
        Unverifiable => "UNVERIFIABLE",
        UnknownOutcome => "UNKNOWN_OUTCOME",
        ReconciledApplied => "RECONCILED_APPLIED",
        ReconciledNotApplied => "RECONCILED_NOT_APPLIED",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_names_match_serde() {
        for s in CommandState::ALL {
            assert_eq!(
                serde_json::to_value(s).unwrap(),
                serde_json::json!(state_name(s))
            );
        }
    }

    #[test]
    fn audit_chain_detects_tampering() {
        let j = Journal::open_in_memory().unwrap();
        for i in 0..5 {
            j.append_audit(
                Timestamp::from_millis(i),
                "test",
                "subject",
                &serde_json::json!({ "i": i }),
            )
            .unwrap();
        }
        assert!(j.verify_audit().unwrap().is_valid());

        j.execute_raw("UPDATE audit SET data = '{\"i\":99}' WHERE seq = 3")
            .unwrap();
        assert_eq!(j.verify_audit().unwrap().first_invalid, Some(3));
    }

    #[test]
    fn audit_chain_detects_deletion() {
        let j = Journal::open_in_memory().unwrap();
        for i in 0..5 {
            j.append_audit(
                Timestamp::from_millis(i),
                "test",
                "s",
                &serde_json::json!(i),
            )
            .unwrap();
        }
        j.execute_raw("DELETE FROM audit WHERE seq = 2").unwrap();
        assert_eq!(j.verify_audit().unwrap().first_invalid, Some(3));
    }
}
