//! Sealed audit segments in the journal (RFC 0002, milestones 3 to 5): the manifest table,
//! writing a closed range of the chain to a file, and checking that file against the
//! rows it came from. Nothing here deletes a row.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use openmhs_types::evidence::GENESIS;
use openmhs_types::segment::{
    SegmentCommand, SegmentError, SegmentHeader, SegmentWriter, for_each, verify_segment,
};
use openmhs_types::{Receipt, Timestamp};
use rusqlite::{Connection, OptionalExtension, ToSql, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    AUDIT_COLUMNS, FaultPhase, Journal, JournalError, audit_record, placeholders, refuse,
    unfinished_states,
};

/// The audit kind of a seal record.
pub const SEAL_KIND: &str = "audit.segment_sealed";

/// Where a segment is in its life (RFC 0002, "Archival state machine").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SegmentState {
    /// A range is reserved; no file is relied on.
    Planned,
    /// The file is written, synced and renamed into place.
    Written,
    /// The file was re-read and agrees with the journal's rows.
    Verified,
    /// A seal record in the audit chain commits to the file.
    Sealed,
    /// The rows the file replaces have been deleted.
    Compacted,
}

impl SegmentState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Planned => "PLANNED",
            Self::Written => "WRITTEN",
            Self::Verified => "VERIFIED",
            Self::Sealed => "SEALED",
            Self::Compacted => "COMPACTED",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        [
            Self::Planned,
            Self::Written,
            Self::Verified,
            Self::Sealed,
            Self::Compacted,
        ]
        .into_iter()
        .find(|s| s.name() == name)
    }
}

/// One row of the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentRow {
    pub number: i64,
    pub first_seq: i64,
    pub last_seq: i64,
    pub prev_hash: String,
    pub chain_root: String,
    pub last_hash: Option<String>,
    pub records: Option<i64>,
    pub commands: Option<i64>,
    /// File name inside the segments directory.
    pub file: Option<String>,
    pub sha256: Option<String>,
    pub bytes: Option<i64>,
    pub state: SegmentState,
    /// Sequence number of the seal record, once sealed.
    pub seal_seq: Option<i64>,
}

/// Live state at the position of a seal record in the chain: what start-up needs from
/// history. Ids only; sorted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// The newest record the snapshot reflects: the one before the seal record.
    pub seq: i64,
    /// Commands in a state they can still leave.
    pub commands: Vec<String>,
    /// Approvals that are `PENDING` or `APPROVED`.
    pub approvals: Vec<String>,
    /// Leases not ended.
    pub leases: Vec<String>,
    /// Plans not in a final state.
    pub plans: Vec<String>,
    /// SHA-256 over the five fields above, as JSON.
    pub sha256: String,
}

impl Snapshot {
    fn digest(&self) -> String {
        let body = serde_json::json!({
            "seq": self.seq, "commands": self.commands, "approvals": self.approvals,
            "leases": self.leases, "plans": self.plans,
        });
        Sha256::digest(body.to_string().as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    pub fn is_consistent(&self) -> bool {
        self.sha256 == self.digest()
    }
}

/// The data of an `audit.segment_sealed` record: what the chain commits to about a segment
/// file, and the live state at that point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealData {
    pub segment: i64,
    pub first_seq: i64,
    pub last_seq: i64,
    pub prev_hash: String,
    pub last_hash: String,
    pub chain_root: String,
    pub records: i64,
    pub commands: i64,
    pub file: String,
    pub file_sha256: String,
    pub bytes: i64,
    pub snapshot: Snapshot,
}

/// How much of the chain a new segment takes.
#[derive(Debug, Clone, Copy)]
pub struct SegmentLimits {
    /// Close the segment once its records reach about this many bytes.
    pub max_bytes: u64,
    /// Close it at this sequence number instead (or the head, if that is lower), however
    /// small the segment is.
    pub through_seq: Option<i64>,
}

pub(super) const SEGMENT_COLUMNS: &str = "SELECT number, first_seq, last_seq, prev_hash, chain_root, \
     last_hash, records, commands, file, sha256, bytes, state, seal_seq FROM segments";

pub(super) fn segment_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SegmentRow> {
    let state: String = r.get(11)?;
    Ok(SegmentRow {
        number: r.get(0)?,
        first_seq: r.get(1)?,
        last_seq: r.get(2)?,
        prev_hash: r.get(3)?,
        chain_root: r.get(4)?,
        last_hash: r.get(5)?,
        records: r.get(6)?,
        commands: r.get(7)?,
        file: r.get(8)?,
        sha256: r.get(9)?,
        bytes: r.get(10)?,
        state: SegmentState::parse(&state).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                11,
                rusqlite::types::Type::Text,
                format!("unknown segment state {state}").into(),
            )
        })?,
        seal_seq: r.get(12)?,
    })
}

pub(super) fn from_segment(e: SegmentError) -> JournalError {
    match e {
        SegmentError::Io(e) => JournalError::Io(e),
        SegmentError::Invalid(why) => refuse("journal.segment_invalid", why),
    }
}

fn file_name(number: i64) -> String {
    format!("seg-{number:08}.jsonl")
}

/// Fails the segment writer after a number of bytes (persistence drills: a full disk).
struct Flaky<W> {
    inner: W,
    left: Option<u64>,
}

impl<W: Write> Write for Flaky<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(left) = &mut self.left {
            if buf.len() as u64 > *left {
                return Err(io::Error::other("injected: no space left on device"));
            }
            *left -= buf.len() as u64;
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Journal {
    /// Where closed segments are kept: `audit/segments` next to the journal file.
    pub fn segments_dir(&self) -> Result<PathBuf, JournalError> {
        let path = self.path.as_ref().ok_or_else(|| {
            refuse(
                "journal.segments_unavailable",
                "an in-memory journal has no segment files",
            )
        })?;
        let home = path.parent().filter(|p| !p.as_os_str().is_empty());
        Ok(home
            .unwrap_or_else(|| Path::new("."))
            .join("audit")
            .join("segments"))
    }

    /// The copy of the manifest kept next to the segments for offline verifiers.
    pub fn manifest_path(&self) -> Result<PathBuf, JournalError> {
        let dir = self.segments_dir()?;
        Ok(dir.parent().map_or_else(
            || PathBuf::from("manifest.json"),
            |d| d.join("manifest.json"),
        ))
    }

    /// Open a read-only connection of its own (file journals only).
    pub(super) fn reader(&self) -> Result<Connection, JournalError> {
        let path = self.path.as_ref().ok_or_else(|| {
            refuse(
                "journal.segments_unavailable",
                "an in-memory journal has no segment files",
            )
        })?;
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(conn)
    }

    /// The manifest, oldest segment first.
    pub fn segments(&self) -> Result<Vec<SegmentRow>, JournalError> {
        let conn = self.lock();
        segment_rows(&conn)
    }

    pub fn segment(&self, number: i64) -> Result<Option<SegmentRow>, JournalError> {
        let conn = self.lock();
        segment_by_number(&conn, number)
    }

    /// Drills only: make the next segment file fail after this many bytes, as a full disk
    /// would.
    #[doc(hidden)]
    pub fn fail_segment_writes_after(&self, bytes: Option<u64>) {
        *self
            .segment_fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = bytes;
    }

    /// Write the next segment (milestone 3): reserve a range, write the file, check it
    /// against the journal. Returns the manifest row, `VERIFIED`. Nothing is deleted.
    ///
    /// Safe to repeat after a crash at any step: a reserved range with no file is
    /// discarded and planned again; a written file is checked again, not rewritten.
    pub fn create_segment(&self, limits: &SegmentLimits) -> Result<SegmentRow, JournalError> {
        let mut row = self.plan_segment(limits)?;
        if row.state == SegmentState::Planned {
            row = self.write_segment_file(&row)?;
        }
        if row.state == SegmentState::Written {
            row = self.verify_segment_file(row.number)?;
        }
        Ok(row)
    }

    /// Seal a `VERIFIED` segment (milestone 4): append the `audit.segment_sealed` record that
    /// commits the chain to the file's SHA-256 and to the live state at this point, and
    /// mark the row `SEALED`, in one transaction. Segments are sealed in order.
    pub fn seal_segment(&self, number: i64, at: Timestamp) -> Result<SegmentRow, JournalError> {
        let row = self.segment(number)?.ok_or_else(|| gone(number))?;
        match row.state {
            SegmentState::Sealed | SegmentState::Compacted => return Ok(row),
            SegmentState::Verified => {}
            other => {
                return Err(refuse(
                    "journal.segment_invalid",
                    format!(
                        "segment {number} is {}, not VERIFIED, so it cannot be sealed",
                        other.name()
                    ),
                ));
            }
        }
        // The file is read again just before the chain commits to it.
        self.check_segment_file(&row)?;
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let current = segment_by_number(&tx, number)?.ok_or_else(|| gone(number))?;
        if current != row {
            return Err(refuse(
                "journal.segment_invalid",
                format!("segment {number} changed while it was being sealed"),
            ));
        }
        if number > 1 {
            let before = segment_by_number(&tx, number - 1)?.ok_or_else(|| gone(number - 1))?;
            if !matches!(before.state, SegmentState::Sealed | SegmentState::Compacted) {
                return Err(refuse(
                    "journal.segment_invalid",
                    format!(
                        "segment {} is not sealed yet; segments are sealed in order",
                        number - 1
                    ),
                ));
            }
        }
        let head: i64 =
            tx.query_row("SELECT COALESCE(MAX(seq), 0) FROM audit", [], |r| r.get(0))?;
        let ids = |sql: &str, params: &[&dyn ToSql]| -> Result<Vec<String>, JournalError> {
            let mut stmt = tx.prepare(sql)?;
            let rows = stmt.query_map(params, |r| r.get::<_, String>(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        };
        let unfinished = unfinished_states();
        let open_plans: Vec<&str> = openmhs_types::plan::PlanState::ALL
            .into_iter()
            .filter(|s| !s.is_terminal())
            .map(openmhs_types::plan::PlanState::name)
            .collect();
        fn as_params<'a>(names: &'a [&'a str]) -> Vec<&'a dyn ToSql> {
            names.iter().map(|s| s as &dyn ToSql).collect()
        }
        let mut snapshot = Snapshot {
            seq: head,
            commands: ids(
                &format!(
                    "SELECT command_id FROM commands WHERE state IN ({}) ORDER BY command_id",
                    placeholders(unfinished.len())
                ),
                &as_params(&unfinished),
            )?,
            approvals: ids(
                "SELECT id FROM approvals WHERE status IN ('PENDING', 'APPROVED') ORDER BY id",
                &[],
            )?,
            leases: ids(
                "SELECT id FROM leases WHERE ended_ms IS NULL ORDER BY id",
                &[],
            )?,
            plans: ids(
                &format!(
                    "SELECT id FROM plans WHERE state IN ({}) ORDER BY id",
                    placeholders(open_plans.len())
                ),
                &as_params(&open_plans),
            )?,
            sha256: String::new(),
        };
        snapshot.sha256 = snapshot.digest();
        let data = SealData {
            segment: row.number,
            first_seq: row.first_seq,
            last_seq: row.last_seq,
            prev_hash: row.prev_hash.clone(),
            last_hash: row.last_hash.clone().unwrap_or_default(),
            chain_root: row.chain_root.clone(),
            records: row.records.unwrap_or(0),
            commands: row.commands.unwrap_or(0),
            file: row.file.clone().unwrap_or_default(),
            file_sha256: row.sha256.clone().unwrap_or_default(),
            bytes: row.bytes.unwrap_or(0),
            snapshot,
        };
        Self::append_audit_tx(
            &tx,
            at,
            SEAL_KIND,
            &format!("segment-{:08}", row.number),
            &serde_json::to_value(&data)?,
        )?;
        let seal_seq = head + 1;
        tx.execute(
            "UPDATE segments SET state = 'SEALED', seal_seq = ?2 WHERE number = ?1 AND state = 'VERIFIED'",
            params![number, seal_seq],
        )?;
        self.fault(&tx, "segment:sealed", FaultPhase::BeforeCommit)?;
        tx.commit()?;
        self.fault(&conn, "segment:sealed", FaultPhase::AfterCommit)?;
        let row = segment_by_number(&conn, number)?.ok_or_else(|| gone(number))?;
        drop(conn);
        self.write_manifest()?;
        Ok(row)
    }

    /// The last row of the manifest decides where the next segment starts.
    fn plan_segment(&self, limits: &SegmentLimits) -> Result<SegmentRow, JournalError> {
        let row = {
            let mut conn = self.lock();
            let tx = conn.transaction()?;
            let last: Option<SegmentRow> = tx
                .query_row(
                    &format!("{SEGMENT_COLUMNS} ORDER BY number DESC LIMIT 1"),
                    [],
                    segment_row,
                )
                .optional()?;
            // A range reserved or a file written before a crash is finished first, as it
            // was planned: its file may already exist under its name.
            if let Some(row) = &last
                && matches!(row.state, SegmentState::Planned | SegmentState::Written)
            {
                return Ok(row.clone());
            }
            let (number, first_seq, prev_hash) = match &last {
                Some(row) => (
                    row.number + 1,
                    row.last_seq + 1,
                    row.last_hash.clone().ok_or_else(|| {
                        refuse(
                            "journal.segment_invalid",
                            format!("segment {} has no last hash", row.number),
                        )
                    })?,
                ),
                None => (1, 1, GENESIS.to_owned()),
            };
            let last_seq = end_of_range(&tx, first_seq, limits)?;
            let chain_root = chain_root_in(&tx)?;
            let row = SegmentRow {
                number,
                first_seq,
                last_seq,
                prev_hash,
                chain_root,
                last_hash: None,
                records: None,
                commands: None,
                file: None,
                sha256: None,
                bytes: None,
                state: SegmentState::Planned,
                seal_seq: None,
            };
            tx.execute(
                "INSERT INTO segments (number, first_seq, last_seq, prev_hash, chain_root, state)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'PLANNED')",
                params![
                    row.number,
                    row.first_seq,
                    row.last_seq,
                    row.prev_hash,
                    row.chain_root
                ],
            )?;
            self.fault(&tx, "segment:planned", FaultPhase::BeforeCommit)?;
            tx.commit()?;
            row
        };
        self.fault(&self.lock(), "segment:planned", FaultPhase::AfterCommit)?;
        Ok(row)
    }

    /// Write a `PLANNED` segment's file from one read snapshot of the journal, sync it,
    /// rename it into place and sync the directory, then mark the row `WRITTEN`.
    fn write_segment_file(&self, plan: &SegmentRow) -> Result<SegmentRow, JournalError> {
        let dir = self.segments_dir()?;
        fs::create_dir_all(&dir)?;
        let name = file_name(plan.number);
        let (tmp, final_path) = (dir.join(format!("{name}.tmp")), dir.join(&name));
        let budget = *self
            .segment_fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (footer, bytes) = if final_path.exists() {
            // A file under this name that the manifest does not know: either this very
            // segment, written just before a crash, or a segment of a newer history that
            // this journal (restored from an older copy?) has no record of. Only the
            // first is taken up; the second is never overwritten.
            let same = File::open(&final_path)
                .ok()
                .and_then(|f| verify_segment(BufReader::new(f)).ok())
                .filter(|s| {
                    let h = &s.header;
                    h.number == u64::try_from(plan.number).unwrap_or(0)
                        && h.first_seq == plan.first_seq
                        && h.last_seq == plan.last_seq
                        && h.prev_hash == plan.prev_hash
                        && h.chain_root == plan.chain_root
                });
            match same {
                Some(summary) => (summary.footer, summary.bytes),
                None => {
                    return Err(refuse(
                        "journal.segment_exists",
                        format!(
                            "{} exists but is not the segment this journal would write: the \
                             journal may have been restored from an older copy. Nothing was \
                             overwritten; move the file aside to go on",
                            final_path.display()
                        ),
                    ));
                }
            }
        } else {
            let written = self.write_file(plan, &tmp, budget);
            let done = match written {
                Ok(done) => done,
                Err(e) => {
                    let _ = fs::remove_file(&tmp);
                    return Err(e);
                }
            };
            fs::rename(&tmp, &final_path)?;
            sync_dir(&dir);
            done
        };
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE segments SET state = 'WRITTEN', last_hash = ?2, records = ?3, commands = ?4,
                 file = ?5, sha256 = ?6, bytes = ?7 WHERE number = ?1 AND state = 'PLANNED'",
            params![
                plan.number,
                footer.last_hash,
                i64::try_from(footer.records).unwrap_or(i64::MAX),
                i64::try_from(footer.commands).unwrap_or(i64::MAX),
                name,
                footer.sha256,
                i64::try_from(bytes).unwrap_or(i64::MAX),
            ],
        )?;
        self.fault(&tx, "segment:written", FaultPhase::BeforeCommit)?;
        tx.commit()?;
        self.fault(&conn, "segment:written", FaultPhase::AfterCommit)?;
        let row = segment_by_number(&conn, plan.number)?.ok_or_else(|| gone(plan.number))?;
        drop(conn);
        self.write_manifest()?;
        Ok(row)
    }

    fn write_file(
        &self,
        plan: &SegmentRow,
        tmp: &Path,
        budget: Option<u64>,
    ) -> Result<(openmhs_types::segment::SegmentFooter, u64), JournalError> {
        let mut conn = self.reader()?;
        let tx = conn.transaction()?;
        let file = File::create(tmp)?;
        let out = BufWriter::new(Flaky {
            inner: file,
            left: budget,
        });
        let mut writer = SegmentWriter::new(
            out,
            SegmentHeader {
                format: openmhs_types::segment::SEGMENT_FORMAT,
                number: u64::try_from(plan.number).unwrap_or(0),
                first_seq: plan.first_seq,
                last_seq: plan.last_seq,
                prev_hash: plan.prev_hash.clone(),
                chain_root: plan.chain_root.clone(),
            },
        )
        .map_err(from_segment)?;
        // A command goes in the segment that holds its last lifecycle record, with the
        // receipt as of that record: if it moved on after this range, a later segment has it.
        let mut last_lifecycle: BTreeMap<String, (i64, String)> = BTreeMap::new();
        {
            let mut stmt = tx.prepare(&format!(
                "{AUDIT_COLUMNS} WHERE seq BETWEEN ?1 AND ?2 ORDER BY seq"
            ))?;
            let mut rows = stmt.query([plan.first_seq, plan.last_seq])?;
            while let Some(row) = rows.next()? {
                let record = audit_record(row)?;
                if matches!(
                    record.kind.as_str(),
                    "command.created" | "command.transition"
                ) {
                    last_lifecycle
                        .insert(record.subject.clone(), (record.seq, record.hash.clone()));
                }
                writer.record(&record).map_err(from_segment)?;
            }
        }
        for (id, (last_seq, hash)) in &last_lifecycle {
            let row: Option<(String, Option<String>)> = tx
                .query_row(
                    "SELECT receipt, expectation FROM commands WHERE command_id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((json, expectation)) = row else {
                continue;
            };
            let receipt: Receipt = serde_json::from_str(&json)?;
            if receipt.audit_hash.as_deref() != Some(hash.as_str()) {
                continue;
            }
            writer
                .command(&SegmentCommand {
                    receipt,
                    expectation,
                    last_seq: *last_seq,
                })
                .map_err(from_segment)?;
        }
        let (footer, bytes, out) = writer.finish().map_err(from_segment)?;
        // `finish` flushed the buffer into the file; sync the file itself.
        out.into_inner()
            .map_err(|e| JournalError::Io(e.into_error()))?
            .inner
            .sync_all()?;
        Ok((footer, bytes))
    }

    /// Re-read a segment's file and check it: the file verifies on its own, agrees with its
    /// manifest row, and matches the journal's audit rows byte for byte and its commands'
    /// receipts. A `WRITTEN` segment becomes `VERIFIED`.
    pub fn verify_segment_file(&self, number: i64) -> Result<SegmentRow, JournalError> {
        let row = self.segment(number)?.ok_or_else(|| gone(number))?;
        if row.state == SegmentState::Planned {
            return Err(refuse(
                "journal.segment_invalid",
                format!("segment {number} has no file yet"),
            ));
        }
        self.check_segment_file(&row)?;
        if row.state != SegmentState::Written {
            return Ok(row);
        }
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE segments SET state = 'VERIFIED' WHERE number = ?1 AND state = 'WRITTEN'",
            [number],
        )?;
        self.fault(&tx, "segment:verified", FaultPhase::BeforeCommit)?;
        tx.commit()?;
        self.fault(&conn, "segment:verified", FaultPhase::AfterCommit)?;
        let row = segment_by_number(&conn, number)?.ok_or_else(|| gone(number))?;
        drop(conn);
        self.write_manifest()?;
        Ok(row)
    }

    /// The file alone, against its row, and (while the rows are still there) against the
    /// journal.
    pub(super) fn check_segment_file(&self, row: &SegmentRow) -> Result<(), JournalError> {
        let bad = |why: String| refuse("journal.segment_invalid", why);
        let name = row
            .file
            .as_deref()
            .ok_or_else(|| bad(format!("segment {} has no file", row.number)))?;
        let path = self.segments_dir()?.join(name);
        let file = File::open(&path).map_err(|e| {
            refuse(
                "journal.archive_unavailable",
                format!(
                    "segment {} cannot be read: {}: {e}",
                    row.number,
                    path.display()
                ),
            )
        })?;
        let summary = verify_segment(BufReader::new(file)).map_err(|e| match e {
            SegmentError::Io(e) => JournalError::Io(e),
            SegmentError::Invalid(why) => {
                bad(format!("segment {} does not verify: {why}", row.number))
            }
        })?;
        let (h, f) = (&summary.header, &summary.footer);
        if h.number != u64::try_from(row.number).unwrap_or(0)
            || h.first_seq != row.first_seq
            || h.last_seq != row.last_seq
            || h.prev_hash != row.prev_hash
            || h.chain_root != row.chain_root
            || Some(&f.last_hash) != row.last_hash.as_ref()
            || Some(&f.sha256) != row.sha256.as_ref()
            || Some(i64::try_from(summary.bytes).unwrap_or(i64::MAX)) != row.bytes
        {
            return Err(bad(format!(
                "segment {} does not match its manifest row",
                row.number
            )));
        }
        if row.state != SegmentState::Compacted {
            self.compare_with_rows(row, &path)?;
        }
        Ok(())
    }

    /// Every record and command in the file equals the journal's row for it.
    fn compare_with_rows(&self, row: &SegmentRow, path: &Path) -> Result<(), JournalError> {
        let mismatch = |why: String| refuse("journal.segment_invalid", why);
        let mut conn = self.reader()?;
        let tx = conn.transaction()?;
        let mut stmt = tx.prepare(&format!(
            "{AUDIT_COLUMNS} WHERE seq BETWEEN ?1 AND ?2 ORDER BY seq"
        ))?;
        let mut rows = stmt.query([row.first_seq, row.last_seq])?;
        let io = |e: rusqlite::Error| SegmentError::Io(io::Error::other(e));
        for_each(
            BufReader::new(File::open(path)?),
            |record| match rows.next().map_err(io)? {
                Some(live) if audit_record(live).map_err(io)? == record => Ok(()),
                _ => Err(SegmentError::Invalid(format!(
                    "audit record {} differs from the journal's",
                    record.seq
                ))),
            },
            |command| {
                let id = &command.receipt.command_id;
                let live: Option<(String, Option<String>)> = tx
                    .query_row(
                        "SELECT receipt, expectation FROM commands WHERE command_id = ?1",
                        [id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .map_err(io)?;
                let Some((json, expectation)) = live else {
                    return Err(SegmentError::Invalid(format!(
                        "command {id} has no row in the journal"
                    )));
                };
                let live: Receipt = serde_json::from_str(&json)
                    .map_err(|e| SegmentError::Invalid(e.to_string()))?;
                // A command that moved on since is the later segment's.
                if live.audit_hash == command.receipt.audit_hash
                    && (live != command.receipt || expectation != command.expectation)
                {
                    return Err(SegmentError::Invalid(format!(
                        "command {id} differs from the journal's"
                    )));
                }
                Ok(())
            },
        )
        .map_err(|e| match e {
            SegmentError::Io(e) => JournalError::Io(e),
            SegmentError::Invalid(why) => mismatch(format!(
                "segment {} does not match the journal: {why}",
                row.number
            )),
        })?;
        if rows.next()?.is_some() {
            return Err(mismatch(format!(
                "segment {} holds fewer records than the journal's range",
                row.number
            )));
        }
        Ok(())
    }

    /// The manifest as a file next to the segments, for verifiers that have no journal.
    pub(super) fn write_manifest(&self) -> Result<(), JournalError> {
        let path = self.manifest_path()?;
        let rows = self.segments()?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        let mut file = File::create(&tmp)?;
        file.write_all(&serde_json::to_vec_pretty(&rows)?)?;
        file.sync_all()?;
        drop(file);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        fs::rename(&tmp, &path)?;
        Ok(())
    }
}

pub(super) fn gone(number: i64) -> JournalError {
    refuse(
        "journal.segment_invalid",
        format!("segment {number} is not in the manifest"),
    )
}

pub(super) fn segment_rows(conn: &Connection) -> Result<Vec<SegmentRow>, JournalError> {
    let mut stmt = conn.prepare(&format!("{SEGMENT_COLUMNS} ORDER BY number"))?;
    let rows = stmt.query_map([], segment_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub(super) fn segment_by_number(
    conn: &Connection,
    number: i64,
) -> Result<Option<SegmentRow>, JournalError> {
    Ok(conn
        .query_row(
            &format!("{SEGMENT_COLUMNS} WHERE number = ?1"),
            [number],
            segment_row,
        )
        .optional()?)
}

/// The hash of the chain's first record, which names the chain: from the manifest once
/// the first record may have been compacted away, otherwise from the record itself.
fn chain_root_in(conn: &Connection) -> Result<String, JournalError> {
    let known: Option<String> = conn
        .query_row(
            "SELECT chain_root FROM segments WHERE number = 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(root) = known {
        return Ok(root);
    }
    conn.query_row("SELECT hash FROM audit WHERE seq = 1", [], |r| r.get(0))
        .optional()?
        .ok_or_else(|| refuse("journal.segment_empty", "the audit log is empty"))
}

/// The last sequence number of a segment starting at `first_seq`.
fn end_of_range(
    conn: &Connection,
    first_seq: i64,
    limits: &SegmentLimits,
) -> Result<i64, JournalError> {
    let head: i64 = conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM audit", [], |r| r.get(0))?;
    if head < first_seq {
        return Err(refuse(
            "journal.segment_empty",
            format!("there are no audit records after {}", first_seq - 1),
        ));
    }
    if let Some(through) = limits.through_seq {
        let last = through.min(head);
        if last < first_seq {
            return Err(refuse(
                "journal.segment_empty",
                format!(
                    "--through-seq {through} is before the next segment's first record {first_seq}"
                ),
            ));
        }
        return Ok(last);
    }
    let mut stmt = conn.prepare(
        "SELECT seq, length(kind) + length(subject) + length(data) + length(prev_hash) \
         + length(hash) + 48 FROM audit WHERE seq >= ?1 ORDER BY seq",
    )?;
    let mut rows = stmt.query([first_seq])?;
    let mut bytes = 0u64;
    while let Some(row) = rows.next()? {
        bytes += u64::try_from(row.get::<_, i64>(1)?).unwrap_or(0);
        if bytes >= limits.max_bytes {
            return Ok(row.get(0)?);
        }
    }
    Err(refuse(
        "journal.segment_not_full",
        format!(
            "the open part of the audit log (records {first_seq} to {head}, about {bytes} bytes) \
             is smaller than the segment size of {} bytes; name --through-seq to close it anyway",
            limits.max_bytes
        ),
    ))
}

/// Make a rename durable. Directories cannot be synced on Windows.
pub(super) fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}
