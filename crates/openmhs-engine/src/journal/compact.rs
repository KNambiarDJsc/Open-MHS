//! Compaction and archived reads (RFC 0002, milestone 5): finished history moves from the
//! journal into its sealed segments, the identity of every command stays, and a command that
//! was archived is still answered from its segment.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::sync::Arc;

use openmhs_types::evidence::{AuditCheckpoint, AuditRecord, GENESIS};
use openmhs_types::segment::{
    SegmentIndex, for_each_command, parse_audit, parse_command, read_index, read_line_at,
    verify_segment, verify_segment_with,
};
use openmhs_types::{Receipt, Timestamp};
use rusqlite::{Connection, OptionalExtension, ToSql, params};

use super::archive::{COMPACTED_KIND, CompactionData};
use super::segments::{
    SEAL_KIND, SealData, SegmentRow, SegmentState, from_segment, gone, segment_by_number,
};
use super::{
    AUDIT_COLUMNS, FaultPhase, Journal, JournalError, audit_record, placeholders, refuse,
    unfinished_states,
};

/// What one compaction did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compaction {
    pub segment: SegmentRow,
    /// Audit rows deleted from the journal: they are in the segment.
    pub audit_rows_deleted: u64,
    /// Rows at or before the segment's end that stay, because live work still needs them.
    pub audit_rows_kept: u64,
    /// Finished commands whose rows moved to the segments.
    pub commands_archived: u64,
}

/// Segments read once in this process: their files verified against their seals, and their
/// indexes.
#[derive(Default)]
pub(super) struct ArchiveCache {
    verified: HashMap<i64, String>,
    indexes: HashMap<i64, Arc<SegmentIndex>>,
}

fn unavailable(number: i64, why: impl std::fmt::Display) -> JournalError {
    refuse(
        "journal.archive_unavailable",
        format!("the archived history in segment {number} cannot be read: {why}"),
    )
}

impl Journal {
    /// The file of a segment whose rows have left the journal, verified once against the
    /// seal in the chain and the manifest. Reading it again later trusts that check.
    fn archived_segment(&self, number: i64) -> Result<(PathBuf, Arc<SegmentIndex>), JournalError> {
        let (row, seal) = {
            let conn = self.lock();
            let row = segment_by_number(&conn, number)?.ok_or_else(|| gone(number))?;
            let seal = row
                .seal_seq
                .map(|seq| {
                    conn.query_row("SELECT data FROM audit WHERE seq = ?1", [seq], |r| {
                        r.get::<_, String>(0)
                    })
                    .optional()
                })
                .transpose()?
                .flatten();
            (row, seal)
        };
        let seal: SealData = seal
            .ok_or_else(|| unavailable(number, "the chain holds no seal for it"))
            .and_then(|json| serde_json::from_str(&json).map_err(Into::into))?;
        let path = self.segments_dir()?.join(&seal.file);
        if row.sha256.as_deref() != Some(seal.file_sha256.as_str()) || seal.segment != number {
            return Err(unavailable(number, "the manifest and the seal disagree"));
        }
        {
            let cache = self
                .archive
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if cache.verified.get(&number) == Some(&seal.file_sha256)
                && let Some(index) = cache.indexes.get(&number)
            {
                return Ok((path, index.clone()));
            }
        }
        let file = File::open(&path).map_err(|e| unavailable(number, e))?;
        let summary = verify_segment(BufReader::new(file)).map_err(|e| unavailable(number, e))?;
        if summary.footer.sha256 != seal.file_sha256 {
            return Err(unavailable(number, "it is not the file its seal names"));
        }
        let index = read_index(File::open(&path)?).map_err(from_segment)?;
        let index = Arc::new(index);
        let mut cache = self
            .archive
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.verified.insert(number, seal.file_sha256);
        cache.indexes.insert(number, index.clone());
        Ok((path, index))
    }

    /// The final receipt of a command whose rows were archived, and the request hash it was
    /// created with. `None` for a command that is not archived (or unknown).
    pub(super) fn archived_receipt(
        &self,
        command_id: &str,
    ) -> Result<Option<(Receipt, String)>, JournalError> {
        let located: Option<(i64, String)> = self
            .lock()
            .query_row(
                "SELECT segment, request_hash FROM command_keys \
                 WHERE command_id = ?1 AND segment IS NOT NULL",
                [command_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((segment, request_hash)) = located else {
            return Ok(None);
        };
        let (path, index) = self.archived_segment(segment)?;
        let Ok(at) = index
            .commands
            .binary_search_by(|(id, _)| id.as_str().cmp(command_id))
        else {
            return Err(unavailable(
                segment,
                format!("it holds no entry for command {command_id}"),
            ));
        };
        let line = read_line_at(File::open(path)?, index.commands[at].1).map_err(from_segment)?;
        let command = parse_command(&line).map_err(from_segment)?;
        Ok(Some((command.receipt, request_hash)))
    }

    /// The command a principal submitted to a device under a key, if its rows were archived.
    pub(super) fn archived_by_key(
        &self,
        principal: &str,
        device: &str,
        key: &str,
    ) -> Result<Option<(Receipt, String)>, JournalError> {
        let id: Option<String> = self
            .lock()
            .query_row(
                "SELECT command_id FROM command_keys WHERE principal = ?1 AND device = ?2 \
                 AND idempotency_key = ?3 AND segment IS NOT NULL",
                params![principal, device, key],
                |r| r.get(0),
            )
            .optional()?;
        match id {
            Some(id) => self.archived_receipt(&id),
            None => Ok(None),
        }
    }

    /// An audit record that has left the journal, read from its segment.
    pub(super) fn archived_record(&self, seq: i64) -> Result<Option<AuditRecord>, JournalError> {
        let number: Option<i64> = self
            .lock()
            .query_row(
                "SELECT number FROM segments WHERE first_seq <= ?1 AND last_seq >= ?1 \
                 AND state IN ('SEALED', 'COMPACTED')",
                [seq],
                |r| r.get(0),
            )
            .optional()?;
        let Some(number) = number else {
            return Ok(None);
        };
        let (path, index) = self.archived_segment(number)?;
        let start = index
            .seq
            .iter()
            .rev()
            .find(|(indexed, _)| *indexed <= seq)
            .map_or(0, |(_, offset)| *offset);
        let mut offset = start;
        for _ in 0..=openmhs_types::segment::INDEX_EVERY {
            let line = read_line_at(File::open(&path)?, offset).map_err(from_segment)?;
            offset += line.len() as u64;
            let record = parse_audit(&line).map_err(from_segment)?;
            if record.seq == seq {
                return Ok(Some(record));
            }
        }
        Ok(None)
    }

    /// Whether the journal has reached a record, including one that moved to a segment.
    pub(super) fn archived_checkpoint_matches(
        &self,
        checkpoint: &AuditCheckpoint,
    ) -> Result<bool, JournalError> {
        Ok(self
            .archived_record(checkpoint.seq)?
            .is_some_and(|r| r.hash == checkpoint.hash))
    }

    /// Move a sealed segment's finished history out of the journal (milestone 5).
    ///
    /// The segment's file is read again and compared with the rows first. Then, in one
    /// transaction: the audit rows at or before the segment's end are deleted, except those
    /// of unfinished commands, open approvals, leases and plans and the commands a live
    /// plan links to (and the seal and compaction records); each finished command whose
    /// records all lie in that range has its row deleted and its identity (`command_keys`)
    /// pointed at the segment holding its final receipt; and an `audit.segment_compacted`
    /// record is appended. A repeat of the same segment is refused. With `vacuum`, the file
    /// is then rewritten so it actually shrinks.
    pub fn compact_segment(
        &self,
        number: i64,
        at: Timestamp,
        vacuum: bool,
    ) -> Result<Compaction, JournalError> {
        let row = self.segment(number)?.ok_or_else(|| gone(number))?;
        match row.state {
            SegmentState::Sealed => {}
            SegmentState::Compacted => {
                return Err(refuse(
                    "journal.segment_invalid",
                    format!("segment {number} is already compacted"),
                ));
            }
            other => {
                return Err(refuse(
                    "journal.segment_invalid",
                    format!(
                        "segment {number} is {}; only a SEALED segment can be compacted",
                        other.name()
                    ),
                ));
            }
        }
        // The file is read in full, and compared with the rows it is about to replace.
        self.check_segment_file(&row)?;

        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let current = segment_by_number(&tx, number)?.ok_or_else(|| gone(number))?;
        if current != row {
            return Err(refuse(
                "journal.segment_invalid",
                format!("segment {number} changed while it was being compacted"),
            ));
        }
        if number > 1 {
            let before = segment_by_number(&tx, number - 1)?.ok_or_else(|| gone(number - 1))?;
            if before.state != SegmentState::Compacted {
                return Err(refuse(
                    "journal.segment_invalid",
                    format!(
                        "segment {} is not compacted yet; segments are compacted in order",
                        number - 1
                    ),
                ));
            }
        }
        let last_seq = row.last_seq;
        let unfinished = unfinished_states();
        let unfinished_params: Vec<&dyn ToSql> =
            unfinished.iter().map(|s| s as &dyn ToSql).collect();
        let open_plans: Vec<&str> = openmhs_types::plan::PlanState::ALL
            .into_iter()
            .filter(|s| !s.is_terminal())
            .map(openmhs_types::plan::PlanState::name)
            .collect();
        let plan_params: Vec<&dyn ToSql> = open_plans.iter().map(|s| s as &dyn ToSql).collect();
        let (unfinished_in, plans_in) = (
            placeholders(unfinished.len()),
            placeholders(open_plans.len()),
        );

        tx.execute_batch(
            "DROP TABLE IF EXISTS temp.compact_keep;
             DROP TABLE IF EXISTS temp.compact_archive;
             CREATE TEMP TABLE compact_keep (subject TEXT PRIMARY KEY);
             CREATE TEMP TABLE compact_archive (id TEXT PRIMARY KEY, last_seq INTEGER);",
        )?;
        // Live work: whatever recovery, an open approval, a lease or a plan still needs.
        tx.execute(
            &format!(
                "INSERT OR IGNORE INTO compact_keep SELECT command_id FROM commands \
                 WHERE state IN ({unfinished_in})"
            ),
            unfinished_params.as_slice(),
        )?;
        tx.execute_batch(
            "INSERT OR IGNORE INTO compact_keep SELECT id FROM approvals \
                 WHERE status IN ('PENDING', 'APPROVED');
             INSERT OR IGNORE INTO compact_keep SELECT id FROM leases WHERE ended_ms IS NULL;",
        )?;
        tx.execute(
            &format!(
                "INSERT OR IGNORE INTO compact_keep SELECT id FROM plans WHERE state IN ({plans_in})"
            ),
            plan_params.as_slice(),
        )?;
        tx.execute(
            &format!(
                "INSERT OR IGNORE INTO compact_keep SELECT command_id FROM plan_commands \
                 WHERE plan_id IN (SELECT id FROM plans WHERE state IN ({plans_in}))"
            ),
            plan_params.as_slice(),
        )?;
        // A finished command moves only if every record about it is in the range: then its
        // segment holds its final receipt.
        tx.execute(
            "INSERT INTO compact_archive SELECT c.command_id, \
                 (SELECT MAX(seq) FROM audit a WHERE a.subject = c.command_id) \
             FROM commands c WHERE c.command_id NOT IN (SELECT subject FROM compact_keep)",
            [],
        )?;
        tx.execute(
            "DELETE FROM compact_archive WHERE last_seq IS NULL OR last_seq > ?1",
            [last_seq],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO compact_keep SELECT command_id FROM commands \
             WHERE command_id NOT IN (SELECT id FROM compact_archive)",
            [],
        )?;
        let commands_archived: i64 =
            tx.query_row("SELECT COUNT(*) FROM compact_archive", [], |r| r.get(0))?;
        let in_range: i64 = tx.query_row(
            "SELECT COUNT(*) FROM audit WHERE seq <= ?1",
            [last_seq],
            |r| r.get(0),
        )?;
        // Identity first: a key must never be without a row, even inside the transaction.
        tx.execute(
            "UPDATE command_keys SET segment = (SELECT s.number FROM segments s, compact_archive a \
                 WHERE a.id = command_keys.command_id \
                 AND s.first_seq <= a.last_seq AND s.last_seq >= a.last_seq) \
             WHERE command_id IN (SELECT id FROM compact_archive)",
            [],
        )?;
        let unplaced: i64 = tx.query_row(
            "SELECT COUNT(*) FROM command_keys WHERE command_id IN (SELECT id FROM compact_archive) \
             AND segment IS NULL",
            [],
            |r| r.get(0),
        )?;
        if unplaced != 0 {
            return Err(refuse(
                "journal.segment_invalid",
                format!("{unplaced} finished commands lie in no segment, so nothing is compacted"),
            ));
        }
        self.fault(&tx, "segment:compacting", FaultPhase::BeforeCommit)?;
        tx.execute(
            "DELETE FROM commands WHERE command_id IN (SELECT id FROM compact_archive)",
            [],
        )?;
        let deleted = tx.execute(
            "DELETE FROM audit WHERE seq <= ?1 AND kind NOT IN (?2, ?3) \
             AND subject NOT IN (SELECT subject FROM compact_keep)",
            params![last_seq, SEAL_KIND, COMPACTED_KIND],
        )?;
        let deleted = i64::try_from(deleted).unwrap_or(i64::MAX);
        let kept = in_range - deleted;
        let data = CompactionData {
            segment: number,
            last_seq,
            last_hash: row.last_hash.clone().unwrap_or_default(),
            audit_rows_deleted: deleted,
            audit_rows_kept: kept,
            commands_archived,
        };
        Self::append_audit_tx(
            &tx,
            at,
            COMPACTED_KIND,
            &format!("segment-{number:08}"),
            &serde_json::to_value(&data)?,
        )?;
        tx.execute(
            "UPDATE segments SET state = 'COMPACTED' WHERE number = ?1 AND state = 'SEALED'",
            [number],
        )?;
        tx.execute_batch("DROP TABLE temp.compact_keep; DROP TABLE temp.compact_archive;")?;
        self.fault(&tx, "segment:compacted", FaultPhase::BeforeCommit)?;
        tx.commit()?;
        self.fault(&conn, "segment:compacted", FaultPhase::AfterCommit)?;
        if vacuum {
            conn.execute_batch("VACUUM")?;
        }
        let segment = segment_by_number(&conn, number)?.ok_or_else(|| gone(number))?;
        drop(conn);
        self.write_manifest()?;
        Ok(Compaction {
            segment,
            audit_rows_deleted: u64::try_from(deleted).unwrap_or(0),
            audit_rows_kept: u64::try_from(kept).unwrap_or(0),
            commands_archived: u64::try_from(commands_archived).unwrap_or(0),
        })
    }

    /// The evidence bundle of a journal whose old rows are in segments: the chain from the
    /// first record, read from the segments and then the journal, and every final receipt.
    /// Same bundle an uncompacted journal would give.
    pub(super) fn write_evidence_archived(
        &self,
        conn: &mut Connection,
        out: &mut dyn Write,
    ) -> Result<AuditCheckpoint, JournalError> {
        let tx = conn.transaction()?;
        let seals = super::archive::records_of::<SealData>(&tx, SEAL_KIND, None)?;
        let compactions = super::archive::records_of::<CompactionData>(&tx, COMPACTED_KIND, None)?;
        let boundary = compactions
            .iter()
            .map(|(_, c)| c.last_seq)
            .max()
            .unwrap_or(0);
        let compacted: Vec<&SealData> = seals
            .iter()
            .map(|(_, s)| s)
            .filter(|s| s.last_seq <= boundary)
            .collect();
        let dir = self.segments_dir()?;
        let head: Option<(i64, String)> = tx
            .query_row(
                "SELECT seq, hash FROM audit ORDER BY seq DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let checkpoint = head.map_or_else(
            || AuditCheckpoint {
                seq: 0,
                hash: GENESIS.into(),
            },
            |(seq, hash)| AuditCheckpoint { seq, hash },
        );
        out.write_all(b"{\"version\":1,\"checkpoint\":")?;
        serde_json::to_writer(&mut *out, &checkpoint)?;
        out.write_all(b",\"audit\":[")?;
        let mut first = true;
        for seal in &compacted {
            let file =
                File::open(dir.join(&seal.file)).map_err(|e| unavailable(seal.segment, e))?;
            let mut failure = None;
            let verified = verify_segment_with(BufReader::new(file), |record| {
                if failure.is_some() {
                    return;
                }
                let mut write = || -> Result<(), JournalError> {
                    if !std::mem::take(&mut first) {
                        out.write_all(b",")?;
                    }
                    serde_json::to_writer(&mut *out, record)?;
                    Ok(())
                };
                failure = write().err();
            });
            if let Some(e) = failure {
                return Err(e);
            }
            let summary = verified.map_err(|e| unavailable(seal.segment, e))?;
            if summary.footer.sha256 != seal.file_sha256 {
                return Err(unavailable(
                    seal.segment,
                    "it is not the file its seal names",
                ));
            }
        }
        {
            let mut stmt = tx.prepare(&format!("{AUDIT_COLUMNS} WHERE seq > ?1 ORDER BY seq"))?;
            let mut rows = stmt.query([boundary])?;
            while let Some(row) = rows.next()? {
                if !std::mem::take(&mut first) {
                    out.write_all(b",")?;
                }
                serde_json::to_writer(&mut *out, &audit_record(row)?)?;
            }
        }
        out.write_all(b"],\"receipts\":[")?;
        let mut first = true;
        {
            let mut stmt = tx.prepare("SELECT receipt FROM commands ORDER BY command_id")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                if !std::mem::take(&mut first) {
                    out.write_all(b",")?;
                }
                let receipt: Receipt = serde_json::from_str(&row.get::<_, String>(0)?)?;
                serde_json::to_writer(&mut *out, &receipt)?;
            }
        }
        // A command's entry in the segment that holds its final receipt. An earlier segment
        // may hold a mid-life copy of the same command; the identity index says which is final.
        for seal in &compacted {
            let file =
                File::open(dir.join(&seal.file)).map_err(|e| unavailable(seal.segment, e))?;
            let mut failure = None;
            for_each_command(file, |command| {
                if failure.is_some() {
                    return Ok(());
                }
                let id = &command.receipt.command_id;
                let final_here: Option<i64> = tx
                    .query_row(
                        "SELECT segment FROM command_keys WHERE command_id = ?1",
                        [id],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|e| {
                        openmhs_types::segment::SegmentError::Io(std::io::Error::other(e))
                    })?
                    .flatten();
                if final_here != Some(seal.segment) {
                    return Ok(());
                }
                let mut write = || -> Result<(), JournalError> {
                    if !std::mem::take(&mut first) {
                        out.write_all(b",")?;
                    }
                    serde_json::to_writer(&mut *out, &command.receipt)?;
                    Ok(())
                };
                failure = write().err();
                Ok(())
            })
            .map_err(from_segment)?;
            if let Some(e) = failure {
                return Err(e);
            }
        }
        out.write_all(b"]}")?;
        out.flush()?;
        Ok(checkpoint)
    }
}
