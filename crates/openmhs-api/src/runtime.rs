//! The runtime file: how a local client finds the running gateway and its operator token.
//!
//! Written by the gateway at start into its home directory, readable only by the current
//! user on Unix (`0600`). On Windows it relies on the per-user profile directory's ACL;
//! a dedicated ACL is a tracked hardening item (ADR-0005).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const RUNTIME_FILE: &str = "gateway.json";
pub const HOME_ENV: &str = "OPENMHS_HOME";

#[derive(Clone, Serialize, Deserialize)]
pub struct RuntimeInfo {
    /// Base URL, e.g. `http://127.0.0.1:7400`.
    pub url: String,
    pub pid: u32,
    /// Principal of the operator token below.
    pub principal: String,
    pub token: String,
    pub started_at: openmhs_types::Timestamp,
}

// The operator token must never reach a log through `{:?}`.
impl std::fmt::Debug for RuntimeInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeInfo")
            .field("url", &self.url)
            .field("pid", &self.pid)
            .field("principal", &self.principal)
            .field("token", &"<redacted>")
            .field("started_at", &self.started_at)
            .finish()
    }
}

/// `$OPENMHS_HOME`, else the platform's per-user local data directory + `openmhs`.
pub fn default_home() -> PathBuf {
    if let Some(home) = std::env::var_os(HOME_ENV) {
        return PathBuf::from(home);
    }
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("openmhs")
}

pub fn runtime_path(home: &Path) -> PathBuf {
    home.join(RUNTIME_FILE)
}

impl RuntimeInfo {
    pub fn load(home: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(runtime_path(home))?;
        serde_json::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}
