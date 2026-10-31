//! Export files and offline evidence verification. Never opens a gateway or journal: an
//! export reads through a running gateway. Neither an export nor a verification holds the
//! bundle in memory.

use std::io::Write;
use std::path::Path;

use openmhs_api::{Client, ClientError};
use openmhs_types::evidence::{AuditCheckpoint, verify_evidence_reader};
use openmhs_types::segment::{SegmentSummary, check_continuity, verify_segment_with};
use serde::Serialize;
use serde::de::{DeserializeOwned, IgnoredAny};

use crate::{exit, show};

/// A new file only the user can read: evidence names principals and devices.
fn create(path: &Path) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub fn save(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let data = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let mut file = create(path)?;
    file.write_all(&data)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Stream the gateway's evidence bundle into a new file at `path`, then read it back to
/// check it is one whole bundle and return its checkpoint. Memory stays flat however long
/// the chain is. A failed export leaves no file behind.
pub async fn export(client: &Client, path: &Path) -> Result<AuditCheckpoint, ClientError> {
    let file = create(path).map_err(ClientError::Protocol)?;
    let result = async {
        let mut out = std::io::BufWriter::new(file);
        client.export_evidence_to(&mut out).await?;
        let file = out
            .into_inner()
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        file.sync_all()
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        checkpoint_of(path).map_err(ClientError::Protocol)
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

/// The checkpoint of the bundle in `path`, parsing the whole file without keeping it.
fn checkpoint_of(path: &Path) -> Result<AuditCheckpoint, String> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Shape {
        version: u32,
        audit: IgnoredAny,
        receipts: IgnoredAny,
        checkpoint: AuditCheckpoint,
    }
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let shape: Shape = serde_json::from_reader(std::io::BufReader::new(file))
        .map_err(|e| format!("{}: not an evidence bundle: {e}", path.display()))?;
    if shape.version != 1 {
        return Err(format!(
            "{}: evidence bundle version {} is not supported",
            path.display(),
            shape.version
        ));
    }
    Ok(shape.checkpoint)
}

/// Whether the file begins like a segment: a header line.
fn is_segment(path: &Path) -> bool {
    use std::io::Read as _;
    let mut head = [0u8; 11];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && &head == b"{\"header\":{"
}

/// The parts of a manifest row an offline verifier checks.
#[derive(serde::Deserialize)]
struct ManifestRow {
    number: i64,
    file: Option<String>,
    sha256: Option<String>,
    last_hash: Option<String>,
}

/// What verifying segment files found.
#[derive(Debug, Serialize)]
pub struct SegmentsVerification {
    pub valid: bool,
    pub segments: usize,
    pub records: i64,
    pub commands: u64,
    pub problems: Vec<String>,
    /// How a retained checkpoint relates to the segments, when one was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
}

/// Verify every `seg-*.jsonl` in a directory, in order, and that they follow on from one
/// another; and, when the manifest copy is next to them, that it agrees. Reads no journal.
pub fn verify_segments(json: bool, dir: &Path, checkpoint: Option<&Path>) -> u8 {
    let mut files: Vec<_> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("seg-") && n.ends_with(".jsonl"))
            })
            .collect(),
        Err(e) => {
            eprintln!("error: {}: {e}", dir.display());
            return exit::ERROR;
        }
    };
    files.sort();
    let manifest = [dir.join("manifest.json"), dir.join("../manifest.json")]
        .into_iter()
        .find_map(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice::<Vec<ManifestRow>>(&bytes).ok());
    verify_segments_at(json, &files, checkpoint, manifest)
}

fn verify_segments_at(
    json: bool,
    files: &[std::path::PathBuf],
    checkpoint: Option<&Path>,
    manifest: Option<Vec<ManifestRow>>,
) -> u8 {
    let anchor = match checkpoint.map(load::<AuditCheckpoint>).transpose() {
        Ok(anchor) => anchor,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::ERROR;
        }
    };
    let mut problems = Vec::new();
    let mut previous: Option<SegmentSummary> = None;
    let (mut records, mut commands) = (0i64, 0u64);
    let mut anchor_hash_seen = None;
    for path in files {
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("error: {}: {e}", path.display());
                return exit::ERROR;
            }
        };
        let result = verify_segment_with(std::io::BufReader::new(file), |record| {
            if anchor.as_ref().is_some_and(|a| a.seq == record.seq) {
                anchor_hash_seen = Some(record.hash.clone());
            }
        });
        match result {
            Err(e) => {
                problems.push(format!("{}: {e}", path.display()));
                previous = None;
            }
            Ok(summary) => {
                // A single file given to verify-file need not be the first segment.
                if (files.len() > 1 || manifest.is_some())
                    && let Err(e) = check_continuity(previous.as_ref(), &summary)
                {
                    problems.push(format!("{}: {e}", path.display()));
                }
                if let Some(row) = manifest.as_ref().and_then(|rows| {
                    rows.iter()
                        .find(|r| r.number == summary.header.number as i64)
                }) && (row.sha256.as_deref() != Some(summary.footer.sha256.as_str())
                    || row.last_hash.as_deref() != Some(summary.footer.last_hash.as_str()))
                {
                    problems.push(format!(
                        "{}: segment {} differs from the manifest",
                        path.display(),
                        summary.header.number
                    ));
                }
                records += i64::try_from(summary.footer.records).unwrap_or(0);
                commands += summary.footer.commands;
                previous = Some(summary);
            }
        }
    }
    if let Some(rows) = &manifest {
        for row in rows.iter().filter(|r| {
            r.file.is_some()
                && !files
                    .iter()
                    .any(|f| f.file_name().and_then(|n| n.to_str()) == r.file.as_deref())
        }) {
            problems.push(format!(
                "segment {} is in the manifest but its file is missing",
                row.number
            ));
        }
    }
    let checkpoint = anchor.as_ref().map(|a| {
        let last = previous.as_ref().map_or(0, |s| s.header.last_seq);
        match &anchor_hash_seen {
            Some(hash) if *hash == a.hash => "contained".to_owned(),
            Some(_) => {
                problems.push(format!("record {} differs from the checkpoint", a.seq));
                "mismatch".to_owned()
            }
            None if a.seq > last => {
                "beyond the segments (the journal's newer records are not checked here)".to_owned()
            }
            None => {
                problems.push(format!("record {} is missing from the segments", a.seq));
                "missing".to_owned()
            }
        }
    });
    let result = SegmentsVerification {
        valid: problems.is_empty() && !files.is_empty(),
        segments: files.len(),
        records,
        commands,
        problems,
        checkpoint,
    };
    show(json, &result, |r| {
        println!(
            "valid: {}; {} segments, {} records, {} commands",
            r.valid, r.segments, r.records, r.commands
        );
        if let Some(c) = &r.checkpoint {
            println!("checkpoint: {c}");
        }
        for p in &r.problems {
            println!("error: {p}");
        }
        if r.segments == 0 {
            println!("error: no segment files");
        }
    });
    if result.valid { exit::OK } else { exit::FAILED }
}

fn load<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_reader(file).map_err(|e| format!("{}: {e}", path.display()))
}

/// Verify the bundle in `path` as it is read: memory grows with the number of commands, not
/// with the length of the audit log.
pub fn verify_file(json: bool, path: &Path, checkpoint: Option<&Path>) -> u8 {
    // A segment file (RFC 0002) is verified as one, a bundle as a bundle.
    if is_segment(path) {
        return verify_segments_at(json, &[path.to_owned()], checkpoint, None);
    }
    let verified = (|| -> Result<_, String> {
        let anchor = checkpoint.map(load::<AuditCheckpoint>).transpose()?;
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        verify_evidence_reader(
            std::io::BufReader::with_capacity(1 << 16, file),
            anchor.as_ref(),
        )
        .map_err(|e| format!("{}: {e}", path.display()))
    })();
    let result = match verified {
        Ok(result) => result,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::ERROR;
        }
    };
    show(json, &result, |result| {
        println!(
            "valid: {}, complete receipt coverage: {}; {} records, {} receipts",
            result.valid, result.complete, result.records, result.receipts
        );
        for error in &result.errors {
            println!("error: {error}");
        }
        for id in &result.unbound_commands {
            println!("legacy receipt without digest: {id}");
        }
    });
    if result.valid && result.complete {
        exit::OK
    } else {
        exit::FAILED
    }
}
