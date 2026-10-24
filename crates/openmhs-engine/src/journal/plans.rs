//! Plans in the journal (ADR-0018). A plan is one record, rewritten on every change with
//! an audit entry in the same transaction; the commands it submitted are linked to it so
//! the engine can find the plan a held or reconciled command belongs to.

use openmhs_types::Receipt;
use openmhs_types::plan::Plan;
use rusqlite::{OptionalExtension, params};

use super::{Journal, JournalError};

fn decode(rows: impl Iterator<Item = rusqlite::Result<String>>) -> Result<Vec<Plan>, JournalError> {
    let mut out = Vec::new();
    for json in rows {
        out.push(serde_json::from_str(&json?)?);
    }
    Ok(out)
}

impl Journal {
    /// Record a new plan and the caller it runs as.
    pub fn insert_plan(&self, plan: &Plan, caller: &str) -> Result<(), JournalError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO plans (id, principal, credential_id, state, created_ms, updated_ms, caller, record)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7)",
            params![
                plan.id,
                plan.principal,
                plan.credential_id,
                plan.state.name(),
                plan.created_at.millis(),
                caller,
                serde_json::to_string(plan)?
            ],
        )?;
        Self::append_audit_tx(
            &tx,
            plan.created_at,
            "plan.submitted",
            &plan.id,
            &serde_json::json!({
                "principal": plan.principal, "credential_id": plan.credential_id,
                "steps": plan.order, "lease_id": plan.lease_id,
            }),
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Store a changed plan with the audit record describing the change.
    pub fn update_plan(
        &self,
        plan: &Plan,
        kind: &str,
        detail: &serde_json::Value,
    ) -> Result<(), JournalError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE plans SET state = ?2, updated_ms = ?3, record = ?4 WHERE id = ?1",
            params![
                plan.id,
                plan.state.name(),
                plan.updated_at.millis(),
                serde_json::to_string(plan)?
            ],
        )?;
        Self::append_audit_tx(&tx, plan.updated_at, kind, &plan.id, detail)?;
        tx.commit()?;
        Ok(())
    }

    pub fn plan(&self, id: &str) -> Result<Option<Plan>, JournalError> {
        let json: Option<String> = self
            .lock()
            .query_row("SELECT record FROM plans WHERE id = ?1", [id], |r| r.get(0))
            .optional()?;
        Ok(match json {
            Some(j) => Some(serde_json::from_str(&j)?),
            None => None,
        })
    }

    /// The stored caller (JSON) a plan runs as.
    pub fn plan_caller(&self, id: &str) -> Result<Option<String>, JournalError> {
        Ok(self
            .lock()
            .query_row("SELECT caller FROM plans WHERE id = ?1", [id], |r| r.get(0))
            .optional()?)
    }

    /// Recent plans, newest first.
    pub fn plans(&self, limit: usize) -> Result<Vec<Plan>, JournalError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT record FROM plans ORDER BY created_ms DESC LIMIT ?1")?;
        let rows = stmt.query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |r| r.get(0))?;
        decode(rows)
    }

    /// Plans that have not ended, oldest first.
    pub fn open_plans(&self) -> Result<Vec<Plan>, JournalError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT record FROM plans
             WHERE state NOT IN ('SUCCEEDED', 'HALTED', 'COMPENSATED', 'ABORTED', 'CANCELLED')
             ORDER BY created_ms",
        )?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        decode(rows)
    }

    /// Remember which plan step a command was submitted for.
    pub fn link_plan_command(
        &self,
        command_id: &str,
        plan_id: &str,
        step_id: &str,
    ) -> Result<(), JournalError> {
        self.lock().execute(
            "INSERT OR IGNORE INTO plan_commands (command_id, plan_id, step_id) VALUES (?1, ?2, ?3)",
            params![command_id, plan_id, step_id],
        )?;
        Ok(())
    }

    /// The plan (and step) a command belongs to, if any.
    pub fn plan_of_command(
        &self,
        command_id: &str,
    ) -> Result<Option<(String, String)>, JournalError> {
        Ok(self
            .lock()
            .query_row(
                "SELECT plan_id, step_id FROM plan_commands WHERE command_id = ?1",
                [command_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// The command a principal submitted to a device under an idempotency key, if any.
    pub fn command_by_key(
        &self,
        principal: &str,
        device: &str,
        key: &str,
    ) -> Result<Option<Receipt>, JournalError> {
        let json: Option<String> = self
            .lock()
            .query_row(
                "SELECT receipt FROM commands WHERE principal = ?1 AND device = ?2 AND idempotency_key = ?3",
                params![principal, device, key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(match json {
            Some(j) => Some(serde_json::from_str(&j)?),
            None => None,
        })
    }
}
