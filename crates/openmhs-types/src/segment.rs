//! Sealed audit segments (RFC 0002, milestone 3): a closed, contiguous range of the audit
//! chain, written once, and a streaming verifier that needs nothing but the file.
//!
//! A segment is UTF-8 text, one JSON object per line, in this order:
//!
//! ```text
//! header   format, segment number, first and last sequence, the hash the range chains
//!          from, and the hash of the chain's first record (which names the chain)
//! audit    every record of the range, in sequence order, exactly as stored
//! command  the receipt (and readback target) of every command whose last lifecycle
//!          record is in the range, in command-id order
//! index    every 256th sequence number and every command id with the byte offset of its
//!          line, so a lookup reads one line instead of the file
//! footer   record and command counts, the last hash, and the SHA-256 of every byte above
//! ```
//!
//! The chain itself proves order and content inside a segment, and `prev_hash` proves
//! continuity between segments. Hashes are the ones every journal already uses, byte for
//! byte ([`audit_hash`]).

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Receipt;
use crate::evidence::{AuditRecord, Binding, GENESIS, audit_hash, sha256_hex};

pub const SEGMENT_FORMAT: u32 = 1;
/// One index entry per this many sequence numbers.
pub const INDEX_EVERY: i64 = 256;
/// Longest line a reader accepts: a line is one record or one receipt.
pub const MAX_LINE: usize = 256 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentHeader {
    pub format: u32,
    pub number: u64,
    pub first_seq: i64,
    pub last_seq: i64,
    /// The hash of record `first_seq - 1` ([`GENESIS`] for the first segment).
    pub prev_hash: String,
    /// The hash of the chain's first record: the same in every segment of one journal.
    pub chain_root: String,
}

/// A finished command: its receipt as of its last lifecycle record in the range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentCommand {
    pub receipt: Receipt,
    /// The readback target stored with the command, as the journal held it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expectation: Option<String>,
    /// Sequence number of the command's last lifecycle record.
    pub last_seq: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentIndex {
    pub every: i64,
    /// `(seq, byte offset of that record's line)`.
    pub seq: Vec<(i64, u64)>,
    /// `(command id, byte offset of that command's line)`, in id order.
    pub commands: Vec<(String, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentFooter {
    pub records: u64,
    pub commands: u64,
    pub last_hash: String,
    /// SHA-256 of every byte before the footer line.
    pub sha256: String,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum LineRef<'a> {
    Header(&'a SegmentHeader),
    Audit(&'a AuditRecord),
    Command(&'a SegmentCommand),
    Index(&'a SegmentIndex),
    Footer(&'a SegmentFooter),
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Line {
    Header(SegmentHeader),
    Audit(AuditRecord),
    Command(Box<SegmentCommand>),
    Index(SegmentIndex),
    Footer(SegmentFooter),
}

/// Why a segment could not be written or does not verify.
#[derive(Debug)]
pub enum SegmentError {
    Io(io::Error),
    /// The content is wrong: damaged, edited, out of order, or not a segment.
    Invalid(String),
}

impl fmt::Display for SegmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Invalid(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for SegmentError {}

impl From<io::Error> for SegmentError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

fn invalid<T>(why: impl Into<String>) -> Result<T, SegmentError> {
    Err(SegmentError::Invalid(why.into()))
}

// ---------------------------------------------------------------------------- writer

/// Writes one segment, checking as it goes that the records chain: a segment is never
/// written from a range that does not verify.
pub struct SegmentWriter<W: Write> {
    out: W,
    hash: Sha256,
    offset: u64,
    header: SegmentHeader,
    last_hash: String,
    next_seq: i64,
    seq_index: Vec<(i64, u64)>,
    commands: Vec<(String, u64)>,
}

impl<W: Write> SegmentWriter<W> {
    pub fn new(out: W, header: SegmentHeader) -> Result<Self, SegmentError> {
        if header.format != SEGMENT_FORMAT
            || header.first_seq < 1
            || header.last_seq < header.first_seq
        {
            return invalid("segment header names an impossible range");
        }
        let mut writer = Self {
            out,
            hash: Sha256::new(),
            offset: 0,
            last_hash: header.prev_hash.clone(),
            next_seq: header.first_seq,
            header,
            seq_index: Vec::new(),
            commands: Vec::new(),
        };
        let header = writer.header.clone();
        writer.line(&LineRef::Header(&header))?;
        Ok(writer)
    }

    /// Write one line; returns the offset it starts at.
    fn line(&mut self, line: &LineRef<'_>) -> Result<u64, SegmentError> {
        let mut bytes = serde_json::to_vec(line).map_err(io::Error::from)?;
        bytes.push(b'\n');
        let at = self.offset;
        self.out.write_all(&bytes)?;
        self.hash.update(&bytes);
        self.offset += bytes.len() as u64;
        Ok(at)
    }

    /// The next record of the range, in sequence order. It must chain from the one before.
    pub fn record(&mut self, record: &AuditRecord) -> Result<(), SegmentError> {
        if !self.commands.is_empty() {
            return invalid("records must come before commands");
        }
        if self.next_seq > self.header.last_seq {
            return invalid("more records than the range holds");
        }
        let expected = audit_hash(
            &self.last_hash,
            record.seq,
            record.at_ms,
            &record.kind,
            &record.subject,
            &record.data,
        );
        if record.seq != self.next_seq
            || record.prev_hash != self.last_hash
            || record.hash != expected
        {
            return invalid(format!(
                "audit record {} does not continue the chain, so no segment is written from it",
                record.seq
            ));
        }
        if record.seq == 1 && record.hash != self.header.chain_root {
            return invalid("the header's chain root is not the first record's hash");
        }
        let at = self.line(&LineRef::Audit(record))?;
        if (record.seq - self.header.first_seq) % INDEX_EVERY == 0 {
            self.seq_index.push((record.seq, at));
        }
        self.last_hash.clone_from(&record.hash);
        self.next_seq += 1;
        Ok(())
    }

    /// The next command, in id order.
    pub fn command(&mut self, command: &SegmentCommand) -> Result<(), SegmentError> {
        if self.next_seq != self.header.last_seq + 1 {
            return invalid("commands must follow every record of the range");
        }
        let id = &command.receipt.command_id;
        if self.commands.last().is_some_and(|(last, _)| last >= id) {
            return invalid(format!("command {id} is out of order or repeated"));
        }
        let at = self.line(&LineRef::Command(command))?;
        self.commands.push((id.clone(), at));
        Ok(())
    }

    /// Write the index and footer and flush. Returns the footer, the file's length and the
    /// underlying writer (so the caller can sync it).
    pub fn finish(mut self) -> Result<(SegmentFooter, u64, W), SegmentError> {
        if self.next_seq != self.header.last_seq + 1 {
            return invalid("the range is not complete");
        }
        let index = SegmentIndex {
            every: INDEX_EVERY,
            seq: std::mem::take(&mut self.seq_index),
            commands: std::mem::take(&mut self.commands),
        };
        self.line(&LineRef::Index(&index))?;
        let footer = SegmentFooter {
            records: u64::try_from(self.header.last_seq - self.header.first_seq + 1)
                .unwrap_or(u64::MAX),
            commands: index.commands.len() as u64,
            last_hash: self.last_hash.clone(),
            sha256: hex(&self.hash.clone().finalize()),
        };
        let mut bytes = serde_json::to_vec(&LineRef::Footer(&footer)).map_err(io::Error::from)?;
        bytes.push(b'\n');
        self.out.write_all(&bytes)?;
        self.out.flush()?;
        Ok((footer, self.offset + bytes.len() as u64, self.out))
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

// ---------------------------------------------------------------------------- verifier

/// What a verified segment says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentSummary {
    pub header: SegmentHeader,
    pub footer: SegmentFooter,
    /// Bytes in the file.
    pub bytes: u64,
    /// Commands whose last record carries no receipt digest (journals before digests).
    pub unbound_commands: Vec<String>,
}

struct Lines<R> {
    reader: R,
    buf: Vec<u8>,
    offset: u64,
}

impl<R: BufRead> Lines<R> {
    /// The next line's bytes (with its newline) and the offset it starts at.
    fn next(&mut self) -> Result<Option<(u64, &[u8])>, SegmentError> {
        self.buf.clear();
        let mut limited = io::Read::take(&mut self.reader, (MAX_LINE + 1) as u64);
        let n = limited.read_until(b'\n', &mut self.buf)?;
        if n == 0 {
            return Ok(None);
        }
        if self.buf.last() != Some(&b'\n') {
            return invalid(if n > MAX_LINE {
                "a line is longer than any record"
            } else {
                "the file ends in the middle of a line"
            });
        }
        let at = self.offset;
        self.offset += n as u64;
        Ok(Some((at, &self.buf)))
    }
}

struct Latest {
    seq: i64,
    hash: String,
    binding: Binding,
    /// For a `DISPATCHED` record: the digest of the readback target it binds.
    expectation_sha256: Option<String>,
}

/// Verify a whole segment from its bytes alone: the header, every record's hash and link,
/// the receipts against the records that bind them, the index against the lines it points
/// at, the footer and the SHA-256 of the file. Memory grows with the commands in the
/// segment, not with its records.
pub fn verify_segment(reader: impl BufRead) -> Result<SegmentSummary, SegmentError> {
    verify_segment_with(reader, |_| {})
}

/// [`verify_segment`], showing each record to `seen` once its hash has checked out.
pub fn verify_segment_with(
    reader: impl BufRead,
    mut seen: impl FnMut(&AuditRecord),
) -> Result<SegmentSummary, SegmentError> {
    let mut lines = Lines {
        reader,
        buf: Vec::new(),
        offset: 0,
    };
    let mut hash = Sha256::new();
    let parse = |bytes: &[u8]| -> Result<Line, SegmentError> {
        serde_json::from_slice(bytes)
            .map_or_else(|e| invalid(format!("not a segment line: {e}")), Ok)
    };

    let Some((_, bytes)) = lines.next()? else {
        return invalid("empty file");
    };
    let Line::Header(header) = parse(bytes)? else {
        return invalid("the first line is not a segment header");
    };
    hash.update(bytes);
    if header.format != SEGMENT_FORMAT {
        return invalid(format!("unsupported segment format {}", header.format));
    }
    if header.first_seq < 1 || header.last_seq < header.first_seq {
        return invalid("segment header names an impossible range");
    }

    let mut prev = header.prev_hash.clone();
    let mut seq = header.first_seq;
    let mut seq_offsets: BTreeMap<i64, u64> = BTreeMap::new();
    let mut latest: BTreeMap<String, Latest> = BTreeMap::new();
    while seq <= header.last_seq {
        let Some((at, bytes)) = lines.next()? else {
            return invalid("the file ends before the range does");
        };
        let Line::Audit(record) = parse(bytes)? else {
            return invalid(format!("expected audit record {seq}"));
        };
        hash.update(bytes);
        if record.seq != seq
            || record.prev_hash != prev
            || record.hash
                != audit_hash(
                    &prev,
                    record.seq,
                    record.at_ms,
                    &record.kind,
                    &record.subject,
                    &record.data,
                )
        {
            return invalid(format!("invalid audit record at sequence {}", record.seq));
        }
        seen(&record);
        if seq == 1 && record.hash != header.chain_root {
            return invalid("the header's chain root is not the first record's hash");
        }
        if (seq - header.first_seq) % INDEX_EVERY == 0 {
            seq_offsets.insert(seq, at);
        }
        if matches!(
            record.kind.as_str(),
            "command.created" | "command.transition"
        ) {
            let data = serde_json::from_str::<serde_json::Value>(&record.data).ok();
            let binding = data.as_ref().map_or(Binding::Invalid, Binding::of_value);
            let expectation_sha256 = data
                .as_ref()
                .filter(|d| d["to"] == "DISPATCHED")
                .and_then(|d| d["expectation_sha256"].as_str())
                .map(str::to_owned);
            let previous = latest.get(&record.subject);
            // A readback target is bound once, by the DISPATCHED record; later records
            // do not repeat it.
            let expectation_sha256 =
                expectation_sha256.or_else(|| previous.and_then(|p| p.expectation_sha256.clone()));
            latest.insert(
                record.subject.clone(),
                Latest {
                    seq: record.seq,
                    hash: record.hash.clone(),
                    binding,
                    expectation_sha256,
                },
            );
        }
        prev.clone_from(&record.hash);
        seq += 1;
    }

    let mut command_offsets: Vec<(String, u64)> = Vec::new();
    let mut unbound = Vec::new();
    let index = loop {
        let Some((at, bytes)) = lines.next()? else {
            return invalid("the file ends before its index");
        };
        match parse(bytes)? {
            Line::Command(command) => {
                let command = *command;
                hash.update(bytes);
                let id = command.receipt.command_id.clone();
                if command_offsets.last().is_some_and(|(last, _)| *last >= id) {
                    return invalid(format!("command {id} is out of order or repeated"));
                }
                let Some(record) = latest.get(&id) else {
                    return invalid(format!("command {id} has no lifecycle record in the range"));
                };
                if command.last_seq != record.seq
                    || command.receipt.audit_hash.as_deref() != Some(record.hash.as_str())
                {
                    return invalid(format!(
                        "command {id} is not at its last record in the range"
                    ));
                }
                match record.binding.matches(&command.receipt) {
                    Some(true) => {}
                    Some(false) => {
                        return invalid(format!("receipt digest mismatch for command {id}"));
                    }
                    None => unbound.push(id.clone()),
                }
                // Journals written before the binding carry no digest to check.
                if let Some(bound) = &record.expectation_sha256
                    && command.expectation.as_deref().map(sha256_hex).as_ref() != Some(bound)
                {
                    return invalid(format!(
                        "readback target for command {id} differs from its audit record"
                    ));
                }
                command_offsets.push((id, at));
            }
            Line::Index(index) => {
                hash.update(bytes);
                break index;
            }
            _ => return invalid("expected a command or the index"),
        }
    };
    // A command whose last record lies in the range but whose receipt is absent would
    // have been moved to a later segment: absent is allowed. A listed command that
    // does not match was refused above.
    let expected_seq: Vec<(i64, u64)> = seq_offsets.into_iter().collect();
    if index.every != INDEX_EVERY || index.seq != expected_seq || index.commands != command_offsets
    {
        return invalid("the index does not point at the lines it names");
    }

    let sha256 = hex(&hash.finalize());
    let Some((_, bytes)) = lines.next()? else {
        return invalid("the file ends before its footer");
    };
    let Line::Footer(footer) = parse(bytes)? else {
        return invalid("expected the footer");
    };
    let total = lines.offset;
    if lines.next()?.is_some() {
        return invalid("data after the footer");
    }
    if footer.sha256 != sha256 {
        return invalid("the file's SHA-256 does not match its footer");
    }
    if footer.records != u64::try_from(header.last_seq - header.first_seq + 1).unwrap_or(0)
        || footer.commands != command_offsets.len() as u64
        || footer.last_hash != prev
    {
        return invalid("the footer disagrees with the file");
    }
    Ok(SegmentSummary {
        header,
        footer,
        bytes: total,
        unbound_commands: unbound,
    })
}

/// Walk the records and commands of a segment in file order, without verifying anything:
/// for comparing a segment with the journal it came from, after [`verify_segment`].
pub fn for_each(
    reader: impl BufRead,
    mut audit: impl FnMut(AuditRecord) -> Result<(), SegmentError>,
    mut command: impl FnMut(SegmentCommand) -> Result<(), SegmentError>,
) -> Result<SegmentHeader, SegmentError> {
    let mut lines = Lines {
        reader,
        buf: Vec::new(),
        offset: 0,
    };
    let mut header = None;
    while let Some((_, bytes)) = lines.next()? {
        match serde_json::from_slice(bytes)
            .map_err(|e| SegmentError::Invalid(format!("not a segment line: {e}")))?
        {
            Line::Header(h) => header = Some(h),
            Line::Audit(record) => audit(record)?,
            Line::Command(c) => command(*c)?,
            Line::Index(_) | Line::Footer(_) => {}
        }
    }
    header.map_or_else(|| invalid("no segment header"), Ok)
}

/// The header and footer of a segment, read from its first and last lines without reading
/// the body. The footer is not checked against the body: this is for start-up, which must
/// not read history (use [`verify_segment`] for that).
pub fn read_ends(
    mut file: impl io::Read + io::Seek,
) -> Result<(SegmentHeader, SegmentFooter), SegmentError> {
    use io::SeekFrom;
    let len = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(0))?;
    let mut first = Vec::new();
    io::BufReader::new(io::Read::take(&mut file, 1 << 16)).read_until(b'\n', &mut first)?;
    let Line::Header(header) = serde_json::from_slice(&first)
        .map_err(|e| SegmentError::Invalid(format!("not a segment header: {e}")))?
    else {
        return invalid("the first line is not a segment header");
    };
    // The footer line is a few hundred bytes.
    let tail = len.min(4096);
    file.seek(SeekFrom::Start(len - tail))?;
    let mut buf = vec![0; usize::try_from(tail).unwrap_or(0)];
    file.read_exact(&mut buf)?;
    let body = buf.strip_suffix(b"\n").unwrap_or(&buf);
    let start = body.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let Line::Footer(footer) = serde_json::from_slice(&body[start..])
        .map_err(|e| SegmentError::Invalid(format!("not a segment footer: {e}")))?
    else {
        return invalid("the last line is not a segment footer");
    };
    Ok((header, footer))
}

/// Read one line of a segment at a byte offset taken from its index.
pub fn read_line_at(
    mut file: impl io::Read + io::Seek,
    offset: u64,
) -> Result<Vec<u8>, SegmentError> {
    file.seek(io::SeekFrom::Start(offset))?;
    let mut line = Vec::new();
    io::BufReader::new(io::Read::take(&mut file, (MAX_LINE + 1) as u64))
        .read_until(b'\n', &mut line)?;
    if line.last() != Some(&b'\n') {
        return invalid("the line at this offset is cut short");
    }
    Ok(line)
}

/// Walk the commands of a segment without reading its records: the index names where they
/// start. Nothing is verified; use [`verify_segment`] for that.
pub fn for_each_command(
    mut file: impl io::Read + io::Seek,
    mut command: impl FnMut(SegmentCommand) -> Result<(), SegmentError>,
) -> Result<(), SegmentError> {
    let index = read_index(&mut file)?;
    let Some((_, first)) = index.commands.first() else {
        return Ok(());
    };
    file.seek(io::SeekFrom::Start(*first))?;
    let mut lines = Lines {
        reader: io::BufReader::new(file),
        buf: Vec::new(),
        offset: *first,
    };
    for _ in 0..index.commands.len() {
        let Some((_, bytes)) = lines.next()? else {
            return invalid("the file ends inside its commands");
        };
        command(parse_command(bytes)?)?;
    }
    Ok(())
}

/// Parse a command line read with [`read_line_at`].
pub fn parse_command(line: &[u8]) -> Result<SegmentCommand, SegmentError> {
    match serde_json::from_slice(line) {
        Ok(Line::Command(command)) => Ok(*command),
        Ok(_) => invalid("the line is not a command"),
        Err(e) => invalid(format!("not a segment line: {e}")),
    }
}

/// Parse an audit line read with [`read_line_at`].
pub fn parse_audit(line: &[u8]) -> Result<AuditRecord, SegmentError> {
    match serde_json::from_slice(line) {
        Ok(Line::Audit(record)) => Ok(record),
        Ok(_) => invalid("the line is not an audit record"),
        Err(e) => invalid(format!("not a segment line: {e}")),
    }
}

/// The index at the end of a segment, found through [`read_ends`]' footer: read from the
/// line before the footer.
pub fn read_index(mut file: impl io::Read + io::Seek) -> Result<SegmentIndex, SegmentError> {
    use io::SeekFrom;
    let len = file.seek(SeekFrom::End(0))?;
    // The index line holds one entry per 256 records and one per command: read backwards
    // in blocks until the line before the footer is whole.
    let mut size = 1u64 << 16;
    loop {
        let tail = len.min(size);
        file.seek(SeekFrom::Start(len - tail))?;
        let mut buf = vec![0; usize::try_from(tail).unwrap_or(0)];
        file.read_exact(&mut buf)?;
        let body = buf.strip_suffix(b"\n").unwrap_or(&buf);
        let footer_start = body.iter().rposition(|b| *b == b'\n');
        let Some(footer_start) = footer_start else {
            if tail == len {
                return invalid("no index");
            }
            size *= 4;
            continue;
        };
        let before = &body[..footer_start];
        let index_start = before.iter().rposition(|b| *b == b'\n');
        let (line, whole) = match index_start {
            Some(i) => (&before[i + 1..], true),
            None => (before, tail == len),
        };
        if !whole {
            size *= 4;
            continue;
        }
        return match serde_json::from_slice(line) {
            Ok(Line::Index(index)) => Ok(index),
            Ok(_) => invalid("the line before the footer is not the index"),
            Err(e) => invalid(format!("not a segment index: {e}")),
        };
    }
}

/// Chain facts a set of segments must agree on: each continues the one before.
pub fn check_continuity(
    previous: Option<&SegmentSummary>,
    next: &SegmentSummary,
) -> Result<(), SegmentError> {
    match previous {
        None => {
            if next.header.first_seq != 1 || next.header.prev_hash != GENESIS {
                return invalid("the first segment does not start at the chain's beginning");
            }
        }
        Some(previous) => {
            if next.header.number != previous.header.number + 1
                || next.header.first_seq != previous.header.last_seq + 1
                || next.header.prev_hash != previous.footer.last_hash
                || next.header.chain_root != previous.header.chain_root
            {
                return invalid(format!(
                    "segment {} does not continue segment {}",
                    next.header.number, previous.header.number
                ));
            }
        }
    }
    Ok(())
}
