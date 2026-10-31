//! Reading the chain across sealed segments (RFC 0002, milestones 4 and 5): start-up from
//! the newest seal instead of from genesis, and verification of a journal whose old rows
//! have moved into segment files.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;

use openmhs_types::CommandState;
use openmhs_types::evidence::{AuditCheckpoint, GENESIS, audit_hash};
use openmhs_types::segment::{SegmentSummary, check_continuity, read_ends, verify_segment_with};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::segments::{SEAL_KIND, SealData, SegmentRow, segment_by_number};
use super::{AUDIT_COLUMNS, AuditVerification, Journal, JournalError};

/// The audit kind of a compaction record.
pub const COMPACTED_KIND: &str = "audit.segment_compacted";

/// The data of an `audit.segment_compacted` record: rows left the journal for this segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionData {
    pub segment: i64,
    pub last_seq: i64,
    pub last_hash: String,
    pub audit_rows_deleted: i64,
    pub audit_rows_kept: i64,
    pub commands_archived: i64,
}

/// What reading the whole history found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveReport {
    /// The newest sequence number: journal rows and segments together.
    pub records: u64,
    pub segments: u64,
    /// The first journal row that does not verify, if any.
    pub first_invalid: Option<i64>,
    /// Everything that did not check out, one line each.
    pub problems: Vec<String>,
}

/// A command's lifecycle record moves the set of commands the chain leaves unfinished.
pub(super) fn apply_lifecycle(
    kind: &str,
    subject: &str,
    data: &str,
    unfinished: &mut HashSet<String>,
) -> Result<(), JournalError> {
    #[derive(Deserialize)]
    struct Lifecycle {
        to: CommandState,
    }
    let open = match kind {
        "command.created" => Some(true),
        "command.transition" => Some(!serde_json::from_str::<Lifecycle>(data)?.to.is_terminal()),
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
    Ok(())
}

/// Every record of `kind`, oldest first, with its data parsed. These kinds are rare, and a
/// partial index holds only them.
pub(super) fn records_of<T: for<'de> Deserialize<'de>>(
    conn: &Connection,
    kind: &str,
    up_to: Option<i64>,
) -> Result<Vec<(i64, T)>, JournalError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT seq, data FROM audit WHERE kind = '{kind}' AND seq <= ?1 ORDER BY seq"
    ))?;
    let rows = stmt.query_map([up_to.unwrap_or(i64::MAX)], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (seq, data) = row?;
        out.push((seq, serde_json::from_str(&data)?));
    }
    Ok(out)
}

/// The result of reading a run of rows.
pub(super) struct Scan {
    pub rows: u64,
    pub first_invalid: Option<i64>,
}

impl Journal {
    pub(super) fn seals(&self) -> Result<Vec<(i64, SealData)>, JournalError> {
        records_of(&self.lock(), SEAL_KIND, None)
    }

    pub(super) fn compactions(&self) -> Result<Vec<(i64, CompactionData)>, JournalError> {
        records_of(&self.lock(), COMPACTED_KIND, None)
    }

    /// Whether rows have been moved out of the journal. Decided by the chain's own
    /// compaction records, not by the manifest table.
    pub(super) fn is_compacted(&self) -> Result<bool, JournalError> {
        Ok(self.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM audit WHERE kind = 'audit.segment_compacted')",
            [],
            |r| r.get(0),
        )?)
    }

    /// Verify rows from `from_seq` on, each chaining from the one before. With `seed_prev`
    /// the first must chain from that hash; without, its own stored link is accepted (it is
    /// covered by an anchor kept elsewhere). Rows must be consecutive.
    pub(super) fn scan_range(
        &self,
        from_seq: i64,
        seed_prev: Option<String>,
        mut unfinished: Option<&mut HashSet<String>>,
    ) -> Result<Scan, JournalError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("{AUDIT_COLUMNS} WHERE seq >= ?1 ORDER BY seq"))?;
        let mut rows = stmt.query([from_seq])?;
        let mut prev = seed_prev;
        let mut count = 0u64;
        let mut first_invalid = None;
        while let Some(row) = rows.next()? {
            count += 1;
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
            let (prev_hash, hash) = (field(5)?, field(6)?);
            let chains_from = prev.as_deref().unwrap_or(prev_hash);
            let recomputed = audit_hash(chains_from, seq, row.get(1)?, kind, subject, data);
            if seq != from_seq + i64::try_from(count - 1).unwrap_or(i64::MAX)
                || prev_hash != chains_from
                || hash != recomputed
            {
                first_invalid = Some(seq);
                continue;
            }
            if let Some(unfinished) = unfinished.as_deref_mut() {
                apply_lifecycle(kind, subject, data, unfinished)?;
            }
            match &mut prev {
                Some(prev) => {
                    prev.clear();
                    prev.push_str(hash);
                }
                None => prev = Some(hash.to_owned()),
            }
        }
        Ok(Scan {
            rows: count,
            first_invalid,
        })
    }

    /// Start-up from the newest seal at or behind `anchor` (RFC 0002, "Trust"). Applicable
    /// only when the journal holds the anchor's record. Then history before the seal is not
    /// read: the chain is verified from the seal record on, and the unfinished commands are
    /// the seal's snapshot plus what the records after it change. Returns `None` whenever
    /// anything is in doubt, and the caller verifies everything instead.
    pub(super) fn fast_start(
        &self,
        anchor: &AuditCheckpoint,
        unfinished: &mut HashSet<String>,
    ) -> Result<Option<AuditVerification>, JournalError> {
        if anchor.seq <= 0 || !self.contains_checkpoint(anchor)? {
            return Ok(None);
        }
        let seal = {
            let conn = self.lock();
            let mut found = records_of::<SealData>(&conn, SEAL_KIND, Some(anchor.seq))?;
            found.pop()
        };
        let Some((seal_seq, seal)) = seal else {
            return Ok(None);
        };
        // The manifest row and the file's own footer must agree with the seal: if they do
        // not, the sealed history cannot be relied on and everything is read instead.
        let row = {
            let conn = self.lock();
            segment_by_number(&conn, seal.segment)?
        };
        let Some(row) = row else { return Ok(None) };
        if row.sha256.as_deref() != Some(seal.file_sha256.as_str())
            || row.last_hash.as_deref() != Some(seal.last_hash.as_str())
            || !seal.snapshot.is_consistent()
            || seal.snapshot.seq != seal_seq - 1
        {
            return Ok(None);
        }
        let file_ok = self.segments_dir().ok().is_some_and(|dir| {
            File::open(dir.join(&seal.file))
                .ok()
                .and_then(|f| read_ends(f).ok())
                .is_some_and(|(header, footer)| {
                    footer.sha256 == seal.file_sha256
                        && footer.last_hash == seal.last_hash
                        && header.number == u64::try_from(seal.segment).unwrap_or(0)
                })
        });
        if !file_ok {
            return Ok(None);
        }
        unfinished.extend(seal.snapshot.commands.iter().cloned());
        let scan = self.scan_range(seal_seq, None, Some(unfinished))?;
        Ok(Some(AuditVerification {
            records: u64::try_from(seal_seq - 1).unwrap_or(0) + scan.rows,
            first_invalid: scan.first_invalid,
        }))
    }

    /// The whole history: journal rows and every sealed segment. Reports everything that
    /// does not check out instead of stopping at the first problem.
    pub fn verify_archive(&self) -> Result<ArchiveReport, JournalError> {
        self.archive_scan(None)
    }

    /// Verify the chain across segments and journal rows. The rows that remain before the
    /// last compaction must each appear, unchanged, in a segment; the rows after it must
    /// follow on from the last segment without a gap.
    pub(super) fn archive_scan(
        &self,
        mut unfinished: Option<&mut HashSet<String>>,
    ) -> Result<ArchiveReport, JournalError> {
        let mut problems = Vec::new();
        let seals = self.seals()?;
        let compactions = self.compactions()?;
        let boundary = compactions
            .iter()
            .map(|(_, c)| c.last_seq)
            .max()
            .unwrap_or(0);

        // Rows still in the journal at or before the boundary: each must hash on its own.
        let mut retained: HashMap<i64, String> = HashMap::new();
        let mut first_invalid = None;
        if boundary > 0 {
            let conn = self.lock();
            let mut stmt =
                conn.prepare(&format!("{AUDIT_COLUMNS} WHERE seq <= ?1 ORDER BY seq"))?;
            let mut rows = stmt.query([boundary])?;
            while let Some(row) = rows.next()? {
                let record = super::audit_record(row)?;
                let recomputed = audit_hash(
                    &record.prev_hash,
                    record.seq,
                    record.at_ms,
                    &record.kind,
                    &record.subject,
                    &record.data,
                );
                if recomputed != record.hash {
                    first_invalid.get_or_insert(record.seq);
                    problems.push(format!("invalid audit record at sequence {}", record.seq));
                }
                if let Some(unfinished) = unfinished.as_deref_mut() {
                    apply_lifecycle(&record.kind, &record.subject, &record.data, unfinished)?;
                }
                retained.insert(record.seq, record.hash);
            }
        }

        // Every sealed segment, in order.
        let dir = self.segments_dir().ok();
        let manifest: BTreeMap<i64, SegmentRow> = {
            let conn = self.lock();
            super::segments::segment_rows(&conn)?
                .into_iter()
                .map(|r| (r.number, r))
                .collect()
        };
        let mut previous: Option<SegmentSummary> = None;
        let mut found: HashSet<i64> = HashSet::new();
        let mut segments = 0u64;
        for (seal_seq, seal) in &seals {
            segments += 1;
            let number = seal.segment;
            if i64::try_from(segments).ok() != Some(number) {
                problems.push(format!(
                    "segment {number} was sealed out of order (record {seal_seq})"
                ));
            }
            let opened = dir
                .as_ref()
                .map(|d| d.join(&seal.file))
                .map(|p| File::open(&p).map_err(|e| format!("{}: {e}", p.display())));
            let file = match opened {
                Some(Ok(file)) => file,
                Some(Err(e)) => {
                    problems.push(format!("segment {number} cannot be read: {e}"));
                    previous = None;
                    continue;
                }
                None => {
                    problems.push(format!("segment {number} has no place to be read from"));
                    continue;
                }
            };
            let result = verify_segment_with(BufReader::new(file), |record| {
                if let Some(hash) = retained.get(&record.seq) {
                    if *hash == record.hash {
                        found.insert(record.seq);
                    } else {
                        problems.push(format!(
                            "audit record {} differs between the journal and segment {number}",
                            record.seq
                        ));
                    }
                }
            });
            match result {
                Err(e) => {
                    problems.push(format!("segment {number} does not verify: {e}"));
                    previous = None;
                }
                Ok(summary) => {
                    let (h, f) = (&summary.header, &summary.footer);
                    if f.sha256 != seal.file_sha256
                        || f.last_hash != seal.last_hash
                        || h.first_seq != seal.first_seq
                        || h.last_seq != seal.last_seq
                        || h.prev_hash != seal.prev_hash
                        || h.chain_root != seal.chain_root
                    {
                        problems.push(format!("segment {number} is not the file its seal names"));
                    }
                    if let Err(e) = check_continuity(previous.as_ref(), &summary) {
                        problems.push(e.to_string());
                    }
                    if let Some(row) = manifest.get(&number)
                        && (row.sha256.as_deref() != Some(f.sha256.as_str())
                            || row.last_hash.as_deref() != Some(f.last_hash.as_str()))
                    {
                        problems.push(format!("segment {number} differs from its manifest row"));
                    }
                    previous = Some(summary);
                }
            }
        }
        for (seq, _) in retained.iter().filter(|(seq, _)| !found.contains(seq)) {
            problems.push(format!(
                "audit record {seq} is in the journal but in no segment, though rows up to \
                 {boundary} were compacted"
            ));
        }

        // The rows after the boundary follow on from the last compacted segment.
        let seed = if boundary == 0 {
            GENESIS.to_owned()
        } else {
            let last = compactions.iter().find(|(_, c)| c.last_seq == boundary);
            let hash = last.map(|(_, c)| c.last_hash.clone());
            if hash.is_none() {
                problems.push("no compaction record names the boundary".into());
            }
            hash.unwrap_or_default()
        };
        let scan = self.scan_range(boundary + 1, Some(seed), unfinished)?;
        if let Some(seq) = scan.first_invalid {
            first_invalid.get_or_insert(seq);
            problems.push(format!("invalid audit record at sequence {seq}"));
        }
        Ok(ArchiveReport {
            records: u64::try_from(boundary).unwrap_or(0) + scan.rows,
            segments,
            first_invalid,
            problems,
        })
    }
}
