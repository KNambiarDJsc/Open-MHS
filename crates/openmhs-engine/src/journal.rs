//! Durable command journal and hash-chained audit log (SQLite).
//!
//! Every lifecycle transition is one SQLite transaction that updates the command's
//! receipt and appends an audit record, so the journal and the audit log can't disagree.
//! The audit log is append-only by convention and tamper-evident by construction: each
//! record's hash covers the previous record's hash.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use openmhs_policy::Expectation;
pub use openmhs_types::evidence::AuditRecord;
use openmhs_types::evidence::{
    AuditCheckpoint, EvidenceBundle, GENESIS, audit_hash as hash_record, receipt_digest,
};
use openmhs_types::receipt::TransitionRecord;
use openmhs_types::{Approval, ApprovalStatus, CommandState, ErrorObject, Receipt, Timestamp};
use rusqlite::{Connection, OptionalExtension, ToSql, Transaction, params};
use sha2::{Digest, Sha256};

mod leases;
mod plans;

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

/// Where an injected fault fires relative to its transaction's commit.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultPhase {
    /// The transaction is rolled back: nothing of it is stored.
    BeforeCommit,
    /// The transaction is stored, and the caller is told it failed.
    AfterCommit,
}

/// What an injected fault does.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultEffect {
    /// One I/O error (`SQLITE_IOERR`); later writes succeed.
    IoError,
    /// An I/O error, after which the journal refuses every write, as a stopped process
    /// would make none. Reopening the file models the restart.
    Crash,
    /// Abort the process on the spot: no unwinding, no orderly close of the database.
    Abort,
}

/// A fault armed at one durable boundary, fired once (persistence drills only). Sites:
/// `command.created`, `transition:<STATE>`, `expectation`, `approval.consume`,
/// `audit:<kind>`.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    pub site: String,
    pub phase: FaultPhase,
    pub effect: FaultEffect,
}

/// Called at every durable boundary with its site and phase, while the journal is locked
/// (persistence drills only).
#[doc(hidden)]
pub type CommitObserver = Box<dyn Fn(&str, FaultPhase) + Send + Sync>;

pub struct Journal {
    conn: Mutex<Connection>,
    fault: Mutex<Option<Fault>>,
    observer: Mutex<Option<CommitObserver>>,
}

pub fn sha256_hex(data: &str) -> String {
    Sha256::digest(data.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Journal {
    /// Open (or create) the journal at `path`. A journal that fails SQLite's integrity
    /// check, or whose audit chain does not verify, is refused: the gateway does not start
    /// on a record it cannot trust, and does not extend a broken chain.
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        let conn = Connection::open(path)?;
        check_integrity(&conn, "PRAGMA quick_check")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let journal = Self::init(conn)?;
        // The unique indexes that stop a command being created twice under one idempotency
        // key, or an approval being spent on two commands, get the full check: quick_check
        // does not compare an index with its table, and a damaged index lets a duplicate in.
        {
            let conn = journal.lock();
            for table in ["commands", "approvals"] {
                check_integrity(&conn, &format!("PRAGMA integrity_check({table})"))?;
            }
        }
        let mut unfinished = HashSet::new();
        let audit = journal.scan_audit(Some(&mut unfinished))?;
        if let Some(seq) = audit.first_invalid {
            return Err(refuse(
                "journal.audit_invalid",
                format!(
                    "the journal's audit chain is broken at record {seq} of {}: it was edited or \
                     damaged, so the gateway will not start on it. Keep the file for \
                     investigation and start from a verified copy",
                    audit.records
                ),
            ));
        }
        journal.check_unfinished(&unfinished)?;
        Ok(journal)
    }

    /// Recovery and the capability holds find unfinished commands through the `state`
    /// index, so a command whose stored state was changed to a final one would be skipped:
    /// not recovered, and no longer holding its capability. At open, the indexed set must
    /// therefore equal the set the hash-chained lifecycle records leave unfinished.
    fn check_unfinished(&self, from_chain: &HashSet<String>) -> Result<(), JournalError> {
        let conn = self.lock();
        let unfinished = unfinished_states();
        let mut stmt = conn.prepare(&format!(
            "SELECT command_id, state FROM commands WHERE state IN ({})",
            placeholders(unfinished.len())
        ))?;
        let mut indexed = HashSet::new();
        let mut rows = stmt.query(rusqlite::params_from_iter(unfinished))?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            if !from_chain.contains(&id) {
                let state: String = row.get(1)?;
                return Err(refuse(
                    "journal.receipt_integrity",
                    format!("command {id} is stored as {state}, but its audit records end it"),
                ));
            }
            indexed.insert(id);
        }
        if let Some(id) = from_chain.iter().find(|id| !indexed.contains(*id)) {
            return Err(refuse(
                "journal.receipt_integrity",
                format!(
                    "command {id} is unfinished in its audit records, but its stored state \
                     says it is finished"
                ),
            ));
        }
        Ok(())
    }

    pub fn open_in_memory() -> Result<Self, JournalError> {
        Self::init(Connection::open_in_memory()?)
    }

    /// Size of the database in bytes: page count times page size, free pages included.
    /// The write-ahead log, which SQLite checkpoints into the file, is not counted.
    pub fn size_bytes(&self) -> Result<u64, JournalError> {
        let conn = self.lock();
        let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        Ok(u64::try_from(pages.saturating_mul(page_size)).unwrap_or(0))
    }

    /// Make SQLite refuse any write that would grow the database past `bytes` (rounded up
    /// to whole pages; never below its current size). A hard limit: it applies to every
    /// write, critical or not.
    pub fn set_max_bytes(&self, bytes: u64) -> Result<(), JournalError> {
        let conn = self.lock();
        let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        let bytes = i64::try_from(bytes).unwrap_or(i64::MAX);
        let pages = bytes.saturating_add(page_size - 1) / page_size;
        conn.pragma_update_and_check(None, "max_page_count", pages, |r| r.get::<_, i64>(0))?;
        Ok(())
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
            );
            CREATE INDEX IF NOT EXISTS audit_subject ON audit(subject, seq);",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
            fault: Mutex::new(None),
            observer: Mutex::new(None),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        // Poisoning means a panic while holding the lock; SQLite transactions roll back
        // on drop, so the database itself is consistent.
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Arm a fault for the persistence drills (tests/persistence_adversary.rs). It fires
    /// once, at the first write that reaches its site and phase.
    #[doc(hidden)]
    pub fn inject(&self, fault: Fault) {
        *self
            .fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(fault);
    }

    /// Watch every durable boundary (persistence drills: the torn-write drill copies the
    /// files after each commit). The observer runs with the journal locked and must not
    /// call back into it.
    #[doc(hidden)]
    pub fn observe_commits(&self, observer: CommitObserver) {
        *self
            .observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(observer);
    }

    /// Fire the armed fault if it is for `site` at `phase`.
    fn fault(&self, conn: &Connection, site: &str, phase: FaultPhase) -> Result<(), JournalError> {
        if let Some(observe) = self
            .observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            observe(site, phase);
        }
        let mut armed = self
            .fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(fault) = armed.take_if(|f| f.site == site && f.phase == phase) else {
            return Ok(());
        };
        match fault.effect {
            FaultEffect::Abort => std::process::abort(),
            FaultEffect::Crash => conn.pragma_update(None, "query_only", true)?,
            FaultEffect::IoError => {}
        }
        Err(JournalError::Sqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
            Some(format!("injected fault at {site} ({phase:?})")),
        )))
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
        let site = format!("audit:{kind}");
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let hash = Self::append_audit_tx(&tx, at, kind, subject, data)?;
        self.fault(&tx, &site, FaultPhase::BeforeCommit)?;
        tx.commit()?;
        self.fault(&conn, &site, FaultPhase::AfterCommit)?;
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
                let receipt: Receipt = serde_json::from_str(&json)?;
                Self::check_receipt(&tx, &receipt)?;
                return Ok(Inserted::Existing {
                    receipt,
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
            &serde_json::json!({ "principal": r.principal, "request": r.request, "request_hash": request_hash, "origin": origin,
                "receipt_digest": { "version": 1, "sha256": receipt_digest(&r)? } }),
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
        self.fault(&tx, "command.created", FaultPhase::BeforeCommit)?;
        tx.commit()?;
        self.fault(&conn, "command.created", FaultPhase::AfterCommit)?;
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
        let site = format!("transition:{}", state_name(to));
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let r = Self::transition_tx(&tx, command_id, to, at, detail, update)?;
        self.fault(&tx, &site, FaultPhase::BeforeCommit)?;
        tx.commit()?;
        self.fault(&conn, &site, FaultPhase::AfterCommit)?;
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
        // Never bind a changed stored receipt into a fresh, apparently valid digest.
        Self::check_receipt(tx, &r)?;
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
        let mut data = serde_json::json!({ "from": from, "to": to, "detail": detail,
            "receipt_digest": { "version": 1, "sha256": receipt_digest(&r)? } });
        if to == CommandState::Dispatched {
            let expectation: Option<Option<String>> = tx
                .query_row(
                    "SELECT expectation FROM commands WHERE command_id = ?1",
                    [command_id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(json) = expectation.flatten() {
                data["expectation_sha256"] = sha256_hex(&json).into();
            }
        }
        let hash = Self::append_audit_tx(tx, at, "command.transition", command_id, &data)?;
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
        self.fault(&tx, "approval.consume", FaultPhase::BeforeCommit)?;
        tx.commit()?;
        self.fault(&conn, "approval.consume", FaultPhase::AfterCommit)?;
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
        let conn = self.lock();
        match Self::load(&conn, command_id) {
            Ok(r) => {
                Self::check_receipt(&conn, &r)?;
                Ok(Some(r))
            }
            Err(JournalError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn set_expectation(
        &self,
        command_id: &str,
        expectation: &Expectation,
    ) -> Result<(), JournalError> {
        // Bound to the audit chain by the DISPATCHED record that follows (transition_tx).
        let conn = self.lock();
        self.fault(&conn, "expectation", FaultPhase::BeforeCommit)?;
        conn.execute(
            "UPDATE commands SET expectation = ?1 WHERE command_id = ?2",
            params![serde_json::to_string(expectation)?, command_id],
        )?;
        self.fault(&conn, "expectation", FaultPhase::AfterCommit)?;
        Ok(())
    }

    pub fn expectation(&self, command_id: &str) -> Result<Option<Expectation>, JournalError> {
        let conn = self.lock();
        let json: Option<Option<String>> = conn
            .query_row(
                "SELECT expectation FROM commands WHERE command_id = ?1",
                [command_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(json) = json.flatten() else {
            return Ok(None);
        };
        // Verification and reconciliation compare the device against this, so it is bound
        // to the chain by the DISPATCHED record: a changed value is refused, never used.
        let bound: Option<String> = conn
            .query_row(
                "SELECT data FROM audit WHERE subject = ?1 AND kind = 'command.transition' \
                 AND json_extract(data, '$.to') = 'DISPATCHED' ORDER BY seq DESC LIMIT 1",
                [command_id],
                |r| r.get(0),
            )
            .optional()?;
        let bound: Option<serde_json::Value> =
            bound.map(|b| serde_json::from_str(&b)).transpose()?;
        // Journals written before the binding carry no digest to check against.
        if let Some(digest) = bound.as_ref().and_then(|b| b.get("expectation_sha256"))
            && digest.as_str() != Some(sha256_hex(&json).as_str())
        {
            return Err(refuse(
                "journal.receipt_integrity",
                format!("stored expectation differs from its audit record for {command_id}"),
            ));
        }
        Ok(Some(serde_json::from_str(&json)?))
    }

    /// All commands currently in one of `states`, oldest first. Found through the `state`
    /// index: the cost grows with the commands returned, not with the journal's history
    /// (recovery asks for the unfinished ones; `open` checks that index against the chain).
    pub fn commands_in_states(
        &self,
        states: &[CommandState],
    ) -> Result<Vec<Receipt>, JournalError> {
        if states.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.lock();
        let mut stmt = conn.prepare(&commands_in_states_sql(states.len()))?;
        let names = states.iter().map(|s| state_name(*s));
        let mut out = Vec::new();
        for row in stmt.query_map(rusqlite::params_from_iter(names), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (json, state) = row?;
            let receipt: Receipt = serde_json::from_str(&json)?;
            Self::check_state(&receipt, &state)?;
            Self::check_receipt(&conn, &receipt)?;
            out.push(receipt);
        }
        Ok(out)
    }

    /// Ids of the commands on `device`'s `capability` still in `UNKNOWN_OUTCOME`, oldest
    /// first.
    pub fn unresolved_commands(
        &self,
        device: &str,
        capability: &str,
    ) -> Result<Vec<String>, JournalError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT command_id FROM commands
             WHERE state = ?1 AND device = ?2 AND capability = ?3
             ORDER BY created_ms, command_id",
        )?;
        let ids = stmt
            .query_map(
                params![state_name(CommandState::UnknownOutcome), device, capability],
                |r| r.get(0),
            )?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(ids)
    }

    /// Receipts on a set of devices whose physical effect may still be open, oldest first:
    /// `UNKNOWN_OUTCOME`, and `DISPATCHED`, `ACCEPTED` or `TIMED_OUT`. This is used to hold
    /// shared resources whose physical state may still be affected by an old command.
    /// Called under the devices' command locks, where nothing but a stop is in flight, any
    /// other command found in flight was abandoned: its outcome could not be recorded.
    pub fn open_commands_on_devices(
        &self,
        devices: &[String],
    ) -> Result<Vec<Receipt>, JournalError> {
        if devices.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.lock();
        let sql = format!(
            "SELECT receipt, state FROM commands WHERE state IN (?, ?, ?, ?) \
             AND device IN ({}) ORDER BY created_ms, command_id",
            placeholders(devices.len())
        );
        let states = [
            CommandState::UnknownOutcome,
            CommandState::Dispatched,
            CommandState::Accepted,
            CommandState::TimedOut,
        ]
        .map(state_name);
        let mut values: Vec<&dyn ToSql> = Vec::with_capacity(devices.len() + states.len());
        values.extend(states.iter().map(|state| state as &dyn ToSql));
        values.extend(devices.iter().map(|device| device as &dyn ToSql));
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(values), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (json, state) = row?;
            let receipt: Receipt = serde_json::from_str(&json)?;
            Self::check_state(&receipt, &state)?;
            Self::check_receipt(&conn, &receipt)?;
            out.push(receipt);
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
            let receipt: Receipt = serde_json::from_str(&json)?;
            Self::check_state(&receipt, &state)?;
            if names.is_empty() || names.contains(&state.as_str()) {
                Self::check_receipt(&conn, &receipt)?;
                out.push(receipt);
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    pub fn audit_records(&self) -> Result<Vec<AuditRecord>, JournalError> {
        let conn = self.lock();
        Self::read_audit(&conn)
    }

    fn read_audit(conn: &Connection) -> Result<Vec<AuditRecord>, JournalError> {
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

    fn check_state(receipt: &Receipt, state: &str) -> Result<(), JournalError> {
        if state_name(receipt.state) != state {
            return Err(refuse(
                "journal.receipt_integrity",
                format!(
                    "stored state differs from receipt for {}",
                    receipt.command_id
                ),
            ));
        }
        Ok(())
    }

    fn check_receipt(conn: &Connection, receipt: &Receipt) -> Result<(), JournalError> {
        let latest: Option<(String, String)> = conn.query_row(
            "SELECT data, hash FROM audit WHERE subject = ?1 AND kind IN ('command.created', 'command.transition') ORDER BY seq DESC LIMIT 1",
            [&receipt.command_id], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        if let Some((data, hash)) = latest {
            let data: serde_json::Value = serde_json::from_str(&data)?;
            if let Some(digest) = data.get("receipt_digest")
                && (digest["version"] != 1
                    || digest["sha256"].as_str() != Some(receipt_digest(receipt)?.as_str())
                    || receipt.audit_hash.as_deref() != Some(hash.as_str()))
            {
                return Err(refuse(
                    "journal.receipt_integrity",
                    format!("receipt integrity failed for {}", receipt.command_id),
                ));
            }
        } else {
            return Err(refuse(
                "journal.receipt_integrity",
                format!("missing audit record for {}", receipt.command_id),
            ));
        }
        Ok(())
    }

    /// Export audit and receipt rows from one SQLite read transaction. No truncation or
    /// filtering: omitted receipts or chain records must be visible to the offline verifier.
    pub fn export_evidence(&self) -> Result<EvidenceBundle, JournalError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let audit = Self::read_audit(&tx)?;
        let receipts = {
            let mut stmt = tx.prepare("SELECT receipt FROM commands ORDER BY command_id")?;
            let mut receipts = Vec::new();
            for row in stmt.query_map([], |row| row.get::<_, String>(0))? {
                receipts.push(serde_json::from_str(&row?)?);
            }
            receipts
        };
        let checkpoint = AuditCheckpoint::at_head(&audit);
        tx.commit()?;
        Ok(EvidenceBundle {
            version: 1,
            audit,
            receipts,
            checkpoint,
        })
    }

    /// The newest audit record's sequence number and hash (genesis when empty).
    pub fn head_checkpoint(&self) -> Result<AuditCheckpoint, JournalError> {
        let head: Option<(i64, String)> = self
            .lock()
            .query_row(
                "SELECT seq, hash FROM audit ORDER BY seq DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(head.map_or_else(
            || AuditCheckpoint {
                seq: 0,
                hash: GENESIS.into(),
            },
            |(seq, hash)| AuditCheckpoint { seq, hash },
        ))
    }

    /// Whether the chain contains the record a checkpoint names: the journal has reached
    /// it, on this same history.
    pub fn contains_checkpoint(&self, checkpoint: &AuditCheckpoint) -> Result<bool, JournalError> {
        if checkpoint.seq == 0 {
            return Ok(checkpoint.hash == GENESIS);
        }
        let hash: Option<String> = self
            .lock()
            .query_row(
                "SELECT hash FROM audit WHERE seq = ?1",
                [checkpoint.seq],
                |r| r.get(0),
            )
            .optional()?;
        Ok(hash.as_deref() == Some(checkpoint.hash.as_str()))
    }

    /// Recompute the hash chain. Detects edited, reordered, inserted or deleted records
    /// (deleting the newest records is only detectable against a separately kept head hash).
    ///
    /// Streams the records: memory stays constant however long the chain is (the gateway
    /// runs this on every start; docs/benchmarks.md, "Journal start-up").
    pub fn verify_audit(&self) -> Result<AuditVerification, JournalError> {
        self.scan_audit(None)
    }

    /// `verify_audit`, optionally also collecting the commands the lifecycle records leave
    /// unfinished (in the same pass: `open` reads the chain once).
    fn scan_audit(
        &self,
        mut unfinished: Option<&mut HashSet<String>>,
    ) -> Result<AuditVerification, JournalError> {
        #[derive(serde::Deserialize)]
        struct Lifecycle {
            to: CommandState,
        }
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT seq, at_ms, kind, subject, data, prev_hash, hash FROM audit ORDER BY seq",
        )?;
        let mut rows = stmt.query([])?;
        let mut prev = GENESIS.to_owned();
        let mut records = 0u64;
        let mut first_invalid = None;
        while let Some(row) = rows.next()? {
            records += 1;
            if first_invalid.is_some() {
                continue;
            }
            let seq: i64 = row.get(0)?;
            let field = |i: usize| {
                row.get_ref(i)?.as_str().map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        i,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })
            };
            let (kind, subject, data) = (field(2)?, field(3)?, field(4)?);
            let recomputed = hash_record(&prev, seq, row.get(1)?, kind, subject, data);
            let (prev_hash, hash) = (field(5)?, field(6)?);
            if seq != i64::try_from(records).unwrap_or(i64::MAX)
                || prev_hash != prev
                || hash != recomputed
            {
                first_invalid = Some(seq);
                continue;
            }
            if let Some(unfinished) = unfinished.as_deref_mut() {
                let open = match kind {
                    "command.created" => Some(true),
                    "command.transition" => {
                        Some(!serde_json::from_str::<Lifecycle>(data)?.to.is_terminal())
                    }
                    _ => None,
                };
                match open {
                    Some(false) => {
                        unfinished.remove(subject);
                    }
                    Some(true) if !unfinished.contains(subject) => {
                        unfinished.insert(subject.to_owned());
                    }
                    _ => {}
                }
            }
            prev.clear();
            prev.push_str(hash);
        }
        Ok(AuditVerification {
            records,
            first_invalid,
        })
    }

    /// Run raw SQL against the journal. Tests use this to simulate tampering.
    #[doc(hidden)]
    pub fn execute_raw(&self, sql: &str) -> Result<usize, JournalError> {
        Ok(self.lock().execute(sql, [])?)
    }
}

/// One of SQLite's own structural checks (`quick_check`, or `integrity_check` of a table and
/// its indexes): pages, records, free lists and indexes, not the meaning of the data.
/// Damage is `journal.corrupt`; other errors (a locked file) pass through as they are.
fn check_integrity(conn: &Connection, pragma: &str) -> Result<(), JournalError> {
    let corrupt = |why: String| {
        refuse(
            "journal.corrupt",
            format!(
                "the journal failed SQLite's integrity check ({why}), so the gateway will not \
                 start on it. Keep the file for investigation and start from a verified copy"
            ),
        )
    };
    let rows = conn.prepare(pragma).and_then(|mut stmt| {
        stmt.query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()
    });
    match rows {
        Ok(rows) if rows == ["ok"] => Ok(()),
        Ok(rows) => Err(corrupt(rows.join("; "))),
        Err(rusqlite::Error::SqliteFailure(e, msg))
            if matches!(
                e.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
            ) =>
        {
            Err(corrupt(msg.unwrap_or_else(|| e.to_string())))
        }
        Err(e) => Err(e.into()),
    }
}

fn placeholders(n: usize) -> String {
    std::iter::repeat_n("?", n).collect::<Vec<_>>().join(",")
}

/// The names of the states a command can still leave.
fn unfinished_states() -> Vec<&'static str> {
    CommandState::ALL
        .into_iter()
        .filter(|s| !s.is_terminal())
        .map(state_name)
        .collect()
}

fn commands_in_states_sql(states: usize) -> String {
    format!(
        "SELECT receipt, state FROM commands WHERE state IN ({}) ORDER BY created_ms, command_id",
        placeholders(states)
    )
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

    /// Recovery's query must stay on the `state` index: a full scan makes every restart
    /// read the whole command history (docs/benchmarks.md, "Journal start-up").
    #[test]
    fn unfinished_commands_are_found_through_the_state_index() {
        let j = Journal::open_in_memory().unwrap();
        let states = unfinished_states();
        let plan: Vec<String> = j
            .lock()
            .prepare(&format!(
                "EXPLAIN QUERY PLAN {}",
                commands_in_states_sql(states.len())
            ))
            .unwrap()
            .query_map(rusqlite::params_from_iter(&states), |r| r.get(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|step| step.contains("USING INDEX commands_state")),
            "{plan:?}"
        );
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
