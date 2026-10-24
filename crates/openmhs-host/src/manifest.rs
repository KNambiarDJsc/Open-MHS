//! Adapter permission manifests (spec/permission-manifest.md).
//!
//! A manifest says how to start an adapter process and what it may touch. The host
//! enforces what it can see from outside the process: the capabilities the adapter
//! exposes, the environment it gets, and how often it may restart. Network endpoints,
//! serial ports and file paths are *declared*: they are handed to the adapter, which the
//! SDK checks cooperatively, but nothing in M7 stops a process from ignoring them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openmhs_types::Descriptor;
use openmhs_types::descriptor::Operation;
use serde::{Deserialize, Serialize};

use crate::protocol::PROTOCOL_VERSIONS;

pub const MANIFEST_VERSION: &str = "0.1";

/// The manifest's JSON Schema (spec/schemas/adapter-manifest.schema.json).
pub const MANIFEST_SCHEMA: &str =
    include_str!("../../../spec/schemas/adapter-manifest.schema.json");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub openmhs_manifest: String,
    pub adapter: AdapterInfo,
    pub runtime: Runtime,
    pub capabilities: CapabilityPolicy,
    #[serde(default)]
    pub endpoints: Endpoints,
    #[serde(default)]
    pub filesystem: Filesystem,
    /// Names of host environment variables passed to the process. Their values are
    /// redacted from the adapter's log output.
    #[serde(default)]
    pub credentials: Vec<String>,
    /// Adapter-specific settings, sent to the adapter in `initialize`.
    #[serde(default)]
    pub config: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub restart: RestartPolicy,
    /// Who vouches for this manifest and the files it launches (ADR-0017). Checked before
    /// launch; never widens what the rest of the manifest allows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Signature>,
}

/// An Ed25519 signature over the manifest and the digests of the adapter's files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signature {
    /// Only `ed25519`.
    pub algorithm: String,
    /// SHA-256 of the signer's public key, as listed in the trusted-key registry.
    pub key_id: String,
    /// The files that make up the adapter, relative to the manifest's directory.
    pub artifacts: Vec<Artifact>,
    /// 64 bytes, hex, over the domain-prefixed canonical manifest with this field blank.
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub path: String,
    /// SHA-256 of the file, hex.
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterInfo {
    pub id: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runtime {
    /// Program and arguments. `{manifest_dir}` expands to the manifest's directory.
    pub command: Vec<String>,
    /// Working directory, relative to the manifest's directory. Default: that directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Non-secret environment variables. `{manifest_dir}` expands as in `command`.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Protocol versions the adapter speaks, offered in `initialize`.
    #[serde(default = "default_protocol")]
    pub protocol: Vec<String>,
}

fn default_protocol() -> Vec<String> {
    PROTOCOL_VERSIONS.iter().map(|v| v.to_string()).collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityPolicy {
    pub allow: Vec<Allow>,
}

/// Capabilities matching `name` may expose `operations`. `*` in `name` matches any run of
/// characters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Allow {
    pub name: String,
    pub operations: Vec<Operation>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoints {
    /// `scheme://host:port` the adapter may connect to.
    #[serde(default)]
    pub network: Vec<String>,
    /// Serial devices the adapter may open (`COM3`, `/dev/ttyUSB0`).
    #[serde(default)]
    pub serial: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filesystem {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Limits {
    /// Largest message either side may send, in bytes, newline excluded.
    pub max_message_bytes: usize,
    /// Bound for `initialize` and `describe`: process start-up, imports, and for a
    /// container the image start, can take far longer than a call.
    pub start_timeout_ms: u64,
    /// Bound for `connect`, `read`, `health` and fixture calls.
    pub call_timeout_ms: u64,
    pub heartbeat_ms: u64,
    /// The process is killed after this many heartbeats in a row go unanswered.
    pub missed_heartbeats: u32,
    /// How long `shutdown` waits for the process to exit before killing it.
    pub shutdown_grace_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_message_bytes: 1 << 20,
            start_timeout_ms: 30_000,
            call_timeout_ms: 5_000,
            heartbeat_ms: 2_000,
            missed_heartbeats: 3,
            shutdown_grace_ms: 2_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RestartPolicy {
    /// More restarts than this within `window_s` quarantines the adapter.
    pub max_restarts: u32,
    pub window_s: u64,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 5,
            window_s: 60,
            initial_backoff_ms: 200,
            max_backoff_ms: 10_000,
        }
    }
}

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whether `name` matches `pattern`, where `*` matches any run of characters.
pub fn matches(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if name.len() < first.len() + last.len() || !name.starts_with(first) || !name.ends_with(last) {
        return false;
    }
    // Both ends matched, so these are character boundaries.
    let mut rest = &name[first.len()..name.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    true
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        let manifest: Manifest = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let problems = manifest.problems();
        if problems.is_empty() {
            Ok(manifest)
        } else {
            Err(problems.join("; "))
        }
    }

    /// Everything wrong with the manifest beyond its JSON shape.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.openmhs_manifest != MANIFEST_VERSION {
            out.push(format!(
                "/openmhs_manifest: unsupported version {:?} (supported: {MANIFEST_VERSION})",
                self.openmhs_manifest
            ));
        }
        if self.adapter.id.is_empty() {
            out.push("/adapter/id: must not be empty".into());
        }
        if self.runtime.command.first().is_none_or(|c| c.is_empty()) {
            out.push("/runtime/command: must name a program".into());
        }
        if !self
            .runtime
            .protocol
            .iter()
            .any(|v| PROTOCOL_VERSIONS.contains(&v.as_str()))
        {
            out.push(format!(
                "/runtime/protocol: none of {:?} is supported by this host (supported: {PROTOCOL_VERSIONS:?})",
                self.runtime.protocol
            ));
        }
        for name in self.runtime.env.keys() {
            if !valid_env_name(name) {
                out.push(format!("/runtime/env/{name}: not a valid variable name"));
            }
            if self.credentials.contains(name) {
                out.push(format!(
                    "/runtime/env/{name}: also listed in /credentials; secrets come from the host environment only"
                ));
            }
        }
        for (i, name) in self.credentials.iter().enumerate() {
            if !valid_env_name(name) {
                out.push(format!(
                    "/credentials/{i}: {name:?} is not a valid variable name"
                ));
            }
        }
        if self.capabilities.allow.is_empty() {
            out.push("/capabilities/allow: must allow at least one capability".into());
        }
        for (i, allow) in self.capabilities.allow.iter().enumerate() {
            if allow.name.is_empty() {
                out.push(format!("/capabilities/allow/{i}/name: must not be empty"));
            }
            if allow.operations.is_empty() {
                out.push(format!(
                    "/capabilities/allow/{i}/operations: must not be empty"
                ));
            }
        }
        for (i, e) in self.endpoints.network.iter().enumerate() {
            if !e.contains("://") {
                out.push(format!(
                    "/endpoints/network/{i}: expected scheme://host:port, got {e:?}"
                ));
            }
        }
        let l = &self.limits;
        if l.max_message_bytes < 1024 {
            out.push("/limits/max_message_bytes: must be at least 1024".into());
        }
        if l.start_timeout_ms == 0
            || l.call_timeout_ms == 0
            || l.heartbeat_ms == 0
            || l.missed_heartbeats == 0
        {
            out.push(
                "/limits: timeouts, heartbeat_ms and missed_heartbeats must be positive".into(),
            );
        }
        let r = &self.restart;
        if r.window_s == 0 || r.initial_backoff_ms == 0 || r.max_backoff_ms < r.initial_backoff_ms {
            out.push(
                "/restart: window_s and initial_backoff_ms must be positive, max_backoff_ms at least initial_backoff_ms"
                    .into(),
            );
        }
        out
    }

    /// Whether the manifest lets a capability called `name` expose `op`.
    pub fn allows(&self, name: &str, op: Operation) -> bool {
        self.capabilities
            .allow
            .iter()
            .any(|a| matches(&a.name, name) && a.operations.contains(&op))
    }

    /// Every way `descriptor` exceeds the manifest. Checked at every process start.
    pub fn violations(&self, descriptor: &Descriptor) -> Vec<String> {
        let mut out = Vec::new();
        if descriptor.adapter.id != self.adapter.id {
            out.push(format!(
                "descriptor adapter.id {:?} differs from the manifest's {:?}",
                descriptor.adapter.id, self.adapter.id
            ));
        }
        for cap in &descriptor.capabilities {
            for op in &cap.operations {
                if !self.allows(&cap.name, *op) {
                    out.push(format!(
                        "capability {:?} exposes {} but the manifest does not allow it",
                        cap.name,
                        op_name(*op)
                    ));
                }
            }
        }
        out
    }
}

pub fn op_name(op: Operation) -> &'static str {
    match op {
        Operation::Read => "read",
        Operation::Write => "write",
        Operation::Subscribe => "subscribe",
        Operation::Invoke => "invoke",
    }
}

/// A manifest with its paths resolved, ready to start a process from.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub manifest: Manifest,
    /// The manifest's directory.
    pub dir: PathBuf,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
}

impl Resolved {
    /// `dir` is the directory the manifest's relative paths are relative to.
    pub fn new(manifest: Manifest, dir: &Path) -> Self {
        let dir_text = dir.display().to_string();
        let expand = |s: &str| s.replace("{manifest_dir}", &dir_text);
        let mut command = manifest.runtime.command.iter().map(|c| expand(c));
        let program = command.next().unwrap_or_default();
        // A bare name ("python") is looked up on PATH; a relative path is relative to the
        // manifest.
        let program = if program.contains('/') || program.contains('\\') {
            dir.join(program)
        } else {
            PathBuf::from(program)
        };
        let args = command.collect();
        let cwd = match &manifest.runtime.cwd {
            Some(c) => dir.join(expand(c)),
            None => dir.to_path_buf(),
        };
        let env = manifest
            .runtime
            .env
            .iter()
            .map(|(k, v)| (k.clone(), expand(v)))
            .collect();
        Self {
            manifest,
            dir: dir.to_path_buf(),
            program,
            args,
            cwd,
            env,
        }
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let manifest = Manifest::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let dir = std::path::absolute(&dir).unwrap_or(dir);
        Ok(Self::new(manifest, &dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(matches("pump.*", "pump.speed"));
        assert!(matches("pump.*", "pump.a.b"));
        assert!(!matches("pump.*", "valve.open"));
        assert!(matches("*", "anything"));
        assert!(matches("a*c*e", "abcde"));
        assert!(!matches("a*c*e", "abde"));
        assert!(matches("exact", "exact"));
        assert!(!matches("exact", "exactly"));
        assert!(!matches("ab*ba", "aba"));
    }

    fn minimal() -> serde_json::Value {
        serde_json::json!({
            "openmhs_manifest": "0.1",
            "adapter": {"id": "py-test", "version": "0.1.0"},
            "runtime": {"command": ["python", "-m", "x"]},
            "capabilities": {"allow": [{"name": "*", "operations": ["read"]}]}
        })
    }

    #[test]
    fn minimal_manifest_parses_with_defaults() {
        let m = Manifest::parse(&minimal().to_string()).unwrap();
        assert_eq!(m.runtime.protocol, vec!["0.1".to_string()]);
        assert_eq!(m.limits, Limits::default());
        assert_eq!(m.restart, RestartPolicy::default());
    }

    #[test]
    fn problems_are_reported_by_pointer() {
        let mut v = minimal();
        v["openmhs_manifest"] = "9".into();
        v["runtime"]["protocol"] = serde_json::json!(["7.0"]);
        v["runtime"]["env"] = serde_json::json!({"TOKEN": "x", "1BAD": "y"});
        v["credentials"] = serde_json::json!(["TOKEN"]);
        v["endpoints"] = serde_json::json!({"network": ["localhost:4840"]});
        let err = Manifest::parse(&v.to_string()).unwrap_err();
        for needle in [
            "/openmhs_manifest",
            "/runtime/protocol",
            "/runtime/env/1BAD",
            "/runtime/env/TOKEN",
            "/endpoints/network/0",
        ] {
            assert!(err.contains(needle), "{needle} missing from {err}");
        }
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let mut v = minimal();
        v["sandbox"] = true.into();
        assert!(Manifest::parse(&v.to_string()).is_err());
    }

    #[test]
    fn paths_resolve_against_the_manifest_directory() {
        let mut v = minimal();
        v["runtime"]["command"] = serde_json::json!(["./bin/adapter", "{manifest_dir}/x.json"]);
        v["runtime"]["cwd"] = "work".into();
        v["runtime"]["env"] = serde_json::json!({"PYTHONPATH": "{manifest_dir}/lib"});
        let m = Manifest::parse(&v.to_string()).unwrap();
        let dir = Path::new("/opt/adapters/pump");
        let r = Resolved::new(m, dir);
        assert_eq!(r.program, dir.join("./bin/adapter"));
        assert_eq!(r.args, vec![format!("{}/x.json", dir.display())]);
        assert_eq!(r.cwd, dir.join("work"));
        assert_eq!(r.env["PYTHONPATH"], format!("{}/lib", dir.display()));
    }
}
