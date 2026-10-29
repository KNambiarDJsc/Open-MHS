//! Export files and offline evidence verification. Never opens a gateway or journal.

use std::io::Write;
use std::path::Path;

use openmhs_types::evidence::{AuditCheckpoint, EvidenceBundle, verify_evidence};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{exit, show};

pub fn save(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let data = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(&data)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))
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
