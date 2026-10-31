//! Export files and offline evidence verification. Never opens a gateway or journal: an
//! export reads through a running gateway.

use std::io::Write;
use std::path::Path;

use openmhs_api::{Client, ClientError};
use openmhs_types::evidence::{AuditCheckpoint, EvidenceBundle, verify_evidence};
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

fn load<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_reader(file).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn verify_file(json: bool, path: &Path, checkpoint: Option<&Path>) -> u8 {
    let input = (|| -> Result<_, String> {
        let bundle = load::<EvidenceBundle>(path)?;
        let anchor = checkpoint.map(load::<AuditCheckpoint>).transpose()?;
        Ok((bundle, anchor))
    })();
    let (bundle, anchor) = match input {
        Ok(input) => input,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::ERROR;
        }
    };
    let result = verify_evidence(&bundle, anchor.as_ref());
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
