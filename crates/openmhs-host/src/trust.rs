//! Signed adapter manifests (ADR-0017, docs/security/M8_THREAT_MODEL.md).
//!
//! A manifest may carry a `signature`: an Ed25519 signature by a key the operator trusts,
//! over the whole manifest (including the list of files that make up the adapter and
//! their SHA-256 digests). Before the host launches an adapter it checks that
//!
//! 1. the signature verifies, so the manifest is exactly what the signer signed;
//! 2. the signer's key is in the trusted-key registry, not revoked, and allowed to sign
//!    this adapter id;
//! 3. every listed file hashes to its signed digest, and every file the launch command
//!    names inside the package directory is listed.
//!
//! The verified bytes are then copied into a private staging directory and the adapter is
//! launched from the copies. Checking a file and running it are therefore one read, and
//! replacing the original afterwards changes nothing.
//!
//! A valid signature says who vouches for this package and that it is intact. It does not
//! say the package is safe, and it never widens what the manifest permits at runtime:
//! capabilities, environment and limits are enforced exactly as for an unsigned one.

use std::path::{Component, Path, PathBuf};

use ed25519_dalek::{Signature as Ed25519Signature, Signer, SigningKey, VerifyingKey};
use openmhs_types::canonical_json;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest::{Artifact, Manifest, Resolved, Signature};

pub const ALGORITHM: &str = "ed25519";
/// Prefixed to the signed bytes so a signature over a manifest cannot be mistaken for a
/// signature over anything else.
pub const SIGNING_DOMAIN: &str = "openmhs-adapter-manifest-v1\n";
pub const TRUSTED_KEYS_FILE: &str = "trusted_keys.json";

/// A refusal to launch, with a stable code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for TrustError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

fn err(code: &'static str, message: impl Into<String>) -> TrustError {
    TrustError {
        code,
        message: message.into(),
    }
}

// ------------------------------------------------------------------ encoding

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn unhex_array<const N: usize>(s: &str) -> Option<[u8; N]> {
    unhex(s)?.try_into().ok()
}

fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// A key's id: the SHA-256 of its 32 public-key bytes. Derived, so it cannot name a key
/// other than the one it is the digest of.
pub fn key_id(public_key_hex: &str) -> Result<String, String> {
    let bytes: [u8; 32] =
        unhex_array(public_key_hex).ok_or("a public key is 32 bytes, written as 64 hex digits")?;
    Ok(sha256(&bytes))
}

// ------------------------------------------------------------------ trusted keys

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedKey {
    pub key_id: String,
    /// 32 bytes, hex.
    pub public_key: String,
    pub name: String,
    /// Adapter ids (`*` patterns) this key may sign. A key can be trusted for one family
    /// of adapters and nothing else.
    pub adapters: Vec<String>,
    pub added_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at_ms: Option<i64>,
}

/// The operator's list of signing keys, a file in the gateway home. Changing it needs
/// access to that directory, not an API token.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustStore {
    #[serde(default)]
    keys: Vec<TrustedKey>,
}

impl TrustStore {
    /// Trusts no one: every signed manifest is refused as `signer.unknown`.
    pub fn empty() -> Self {
        Self::default()
    }

    /// A missing file is an empty registry.
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::empty());
        }
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())? + "\n";
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn keys(&self) -> &[TrustedKey] {
        &self.keys
    }

    pub fn add(
        &mut self,
        public_key_hex: &str,
        name: &str,
        adapters: Vec<String>,
        now_ms: i64,
    ) -> Result<TrustedKey, String> {
        let id = key_id(public_key_hex)?;
        VerifyingKey::from_bytes(&unhex_array(public_key_hex).ok_or("bad public key")?)
            .map_err(|_| "not a valid Ed25519 public key".to_owned())?;
        if self.keys.iter().any(|k| k.key_id == id) {
            return Err(format!("key {id} is already in the registry"));
        }
        if name.trim().is_empty() {
            return Err("give the key a name, so you can tell later whose it is".into());
        }
        let key = TrustedKey {
            key_id: id,
            public_key: public_key_hex.to_lowercase(),
            name: name.to_owned(),
            adapters: if adapters.is_empty() {
                vec!["*".into()]
            } else {
                adapters
            },
            added_at_ms: now_ms,
            revoked_at_ms: None,
        };
        self.keys.push(key.clone());
        Ok(key)
    }

    /// Revoke a key. It stays listed, revoked. Returns false if it was already revoked.
    pub fn revoke(&mut self, key_id: &str, now_ms: i64) -> Result<bool, String> {
        let key = self
            .keys
            .iter_mut()
            .find(|k| k.key_id == key_id)
            .ok_or_else(|| format!("no key {key_id} in the registry"))?;
        if key.revoked_at_ms.is_some() {
            return Ok(false);
        }
        key.revoked_at_ms = Some(now_ms);
        Ok(true)
    }
}

// ------------------------------------------------------------------ signing

pub struct Keypair {
    /// The 32-byte secret seed, hex. Keep it secret.
    pub seed: String,
    pub public_key: String,
    pub key_id: String,
}

pub fn generate_keypair() -> Result<Keypair, String> {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
    keypair_from_seed(&hex(&seed))
}

pub fn keypair_from_seed(seed_hex: &str) -> Result<Keypair, String> {
    let seed: [u8; 32] =
        unhex_array(seed_hex.trim()).ok_or("a signing seed is 32 bytes, written as 64 hex digits")?;
    let public = hex(SigningKey::from_bytes(&seed).verifying_key().as_bytes());
    Ok(Keypair {
        seed: hex(&seed),
        key_id: key_id(&public)?,
        public_key: public,
    })
}

/// The bytes that are signed: the domain, then the manifest in canonical form with the
/// signature value blanked. Covers the key id, the algorithm and every artifact digest.
fn signing_message(manifest: &Manifest) -> Result<Vec<u8>, String> {
    let mut m = manifest.clone();
    if let Some(s) = &mut m.signature {
        s.value = String::new();
    }
    let value = serde_json::to_value(&m).map_err(|e| e.to_string())?;
    Ok([SIGNING_DOMAIN.as_bytes(), canonical_json(&value).as_bytes()].concat())
}

/// A path inside the package: relative, forward-slashed, no `..`.
fn package_path(raw: &str) -> Result<String, String> {
    let p = Path::new(raw);
    let mut parts = Vec::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return Err(format!("artifact path {raw:?} must stay inside the package")),
        }
    }
    if parts.is_empty() {
        return Err(format!("artifact path {raw:?} names nothing"));
    }
    Ok(parts.join("/"))
}

fn collect(dir: &Path, rel: &str, out: &mut Vec<String>) -> Result<(), String> {
    let full = dir.join(rel);
    let meta = std::fs::symlink_metadata(&full).map_err(|e| format!("{}: {e}", full.display()))?;
    if meta.is_dir() {
        let mut names: Vec<String> = std::fs::read_dir(&full)
            .map_err(|e| format!("{}: {e}", full.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for name in names {
            if name == "__pycache__" || name.ends_with(".pyc") {
                continue;
            }
            collect(dir, &format!("{rel}/{name}"), out)?;
        }
    } else if meta.is_file() {
        out.push(rel.to_owned());
    } else {
        return Err(format!("{}: not a regular file", full.display()));
    }
    Ok(())
}

/// Sign the manifest at `manifest_text` (a manifest file in `dir`) covering `artifacts`
/// (files, or directories searched recursively, relative to `dir`). Returns the manifest
/// JSON with its `signature`.
pub fn sign_manifest(
    manifest_text: &str,
    dir: &Path,
    seed_hex: &str,
    artifacts: &[String],
) -> Result<String, String> {
    let mut manifest = Manifest::parse(manifest_text)?;
    let pair = keypair_from_seed(seed_hex)?;
    let mut files = Vec::new();
    for raw in artifacts {
        collect(dir, &package_path(raw)?, &mut files)?;
    }
    files.sort();
    files.dedup();
    if files.is_empty() {
        return Err("name at least one file that makes up the adapter (--artifact)".into());
    }
    let mut listed = Vec::new();
    for rel in files {
        let bytes = std::fs::read(dir.join(&rel)).map_err(|e| format!("{rel}: {e}"))?;
        listed.push(Artifact {
            path: rel,
            sha256: sha256(&bytes),
        });
    }
    manifest.signature = Some(Signature {
        algorithm: ALGORITHM.into(),
        key_id: pair.key_id,
        artifacts: listed,
        value: String::new(),
    });
    let seed: [u8; 32] = unhex_array(seed_hex.trim()).ok_or("bad seed")?;
    let sig = SigningKey::from_bytes(&seed).sign(&signing_message(&manifest)?);
    if let Some(s) = &mut manifest.signature {
        s.value = hex(&sig.to_bytes());
    }
    serde_json::to_string_pretty(&manifest)
        .map(|t| t + "\n")
        .map_err(|e| e.to_string())
}

// ------------------------------------------------------------------ verification

/// A verified package, copied to a private directory the adapter is launched from. The
/// directory is removed when this is dropped.
pub struct Staged {
    /// The manifest with its launch command pointing at the copies.
    pub resolved: Resolved,
    dir: PathBuf,
    /// The signer.
    pub signer: TrustedKey,
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl std::fmt::Debug for Staged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Staged")
            .field("dir", &self.dir)
            .field("signer", &self.signer.key_id)
            .finish()
    }
}

/// Lexical normalization (no filesystem access): drops `.` and resolves `..`.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn stage_dir() -> Result<PathBuf, TrustError> {
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).map_err(|e| err("stage.failed", e.to_string()))?;
    let dir = std::env::temp_dir().join(format!("openmhs-stage-{}", hex(&random)));
    std::fs::create_dir(&dir).map_err(|e| err("stage.failed", format!("{}: {e}", dir.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

/// Verify `resolved` against `store` and stage it. See the module documentation for what
/// is checked.
pub fn verify_and_stage(resolved: &Resolved, store: &TrustStore) -> Result<Staged, TrustError> {
    let manifest = &resolved.manifest;
    let Some(sig) = &manifest.signature else {
        return Err(err(
            "manifest.unsigned",
            format!(
                "adapter {:?} has no signature; sign it with `openmhs adapter sign`, or for development only run with --allow-unsigned-adapters",
                manifest.adapter.id
            ),
        ));
    };
    if sig.algorithm != ALGORITHM {
        return Err(err(
            "manifest.signature.algorithm",
            format!("unsupported algorithm {:?}; only {ALGORITHM} is accepted", sig.algorithm),
        ));
    }

    // Who signed.
    let signer = store
        .keys
        .iter()
        .find(|k| k.key_id == sig.key_id)
        .ok_or_else(|| {
            err(
                "signer.unknown",
                format!("signing key {} is not in the trusted-key registry", sig.key_id),
            )
        })?;
    if signer.revoked_at_ms.is_some() {
        return Err(err(
            "signer.revoked",
            format!("signing key {} ({}) was revoked", signer.key_id, signer.name),
        ));
    }
    if !signer
        .adapters
        .iter()
        .any(|p| crate::manifest::matches(p, &manifest.adapter.id))
    {
        return Err(err(
            "signer.not_allowed",
            format!(
                "key {} ({}) is not trusted to sign adapter {:?}",
                signer.key_id, signer.name, manifest.adapter.id
            ),
        ));
    }

    // That the manifest is what they signed.
    let public: [u8; 32] = unhex_array(&signer.public_key)
        .ok_or_else(|| err("signer.invalid", "the registry holds a malformed public key"))?;
    let verifying = VerifyingKey::from_bytes(&public)
        .map_err(|_| err("signer.invalid", "the registry holds an invalid public key"))?;
    let value: [u8; 64] = unhex_array(&sig.value)
        .ok_or_else(|| err("manifest.signature.invalid", "the signature is not 64 bytes of hex"))?;
    let message = signing_message(manifest)
        .map_err(|e| err("manifest.signature.invalid", e))?;
    verifying
        .verify_strict(&message, &Ed25519Signature::from_bytes(&value))
        .map_err(|_| {
            err(
                "manifest.signature.invalid",
                "the signature does not match the manifest: it was changed after signing, or signed by another key",
            )
        })?;

    // That the files are what they signed, copied as they are read.
    if sig.artifacts.is_empty() {
        return Err(err(
            "manifest.signature.no_artifacts",
            "the signature covers no files, so it binds the manifest to no program",
        ));
    }
    let stage = stage_dir()?;
    let result = stage_artifacts(resolved, sig, &stage);
    match result {
        Ok(resolved) => Ok(Staged {
            resolved,
            dir: stage,
            signer: signer.clone(),
        }),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&stage);
            Err(e)
        }
    }
}

fn stage_artifacts(
    resolved: &Resolved,
    sig: &Signature,
    stage: &Path,
) -> Result<Resolved, TrustError> {
    let mut mapped: Vec<(PathBuf, PathBuf)> = Vec::new();
    for artifact in &sig.artifacts {
        let rel = package_path(&artifact.path).map_err(|e| err("artifact.invalid", e))?;
        let source = resolved.dir.join(&rel);
        let meta = std::fs::symlink_metadata(&source).map_err(|e| {
            err(
                "artifact.missing",
                format!("{rel}: signed but not found ({e})"),
            )
        })?;
        if !meta.is_file() {
            return Err(err(
                "artifact.invalid",
                format!("{rel}: signed files must be regular files, not links or directories"),
            ));
        }
        let bytes = std::fs::read(&source)
            .map_err(|e| err("artifact.missing", format!("{rel}: {e}")))?;
        if sha256(&bytes) != artifact.sha256.to_lowercase() {
            return Err(err(
                "artifact.modified",
                format!("{rel} does not match the digest that was signed"),
            ));
        }
        let dest = stage.join(&rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| err("stage.failed", format!("{}: {e}", parent.display())))?;
        }
        {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o500);
            }
            let mut file = options
                .open(&dest)
                .map_err(|e| err("stage.failed", format!("{}: {e}", dest.display())))?;
            file.write_all(&bytes)
                .map_err(|e| err("stage.failed", format!("{}: {e}", dest.display())))?;
        }
        mapped.push((normalize(&source), dest));
    }

    // Point the launch command at the copies, and insist it names nothing unsigned.
    let package = normalize(&resolved.dir);
    let mut out = resolved.clone();
    let check = |candidate: PathBuf, slot: Option<&mut String>| -> Result<Option<PathBuf>, TrustError> {
        let candidate = normalize(&candidate);
        if let Some((_, staged)) = mapped.iter().find(|(src, _)| *src == candidate) {
            if let Some(slot) = slot {
                *slot = staged.display().to_string();
            }
            return Ok(Some(staged.clone()));
        }
        if candidate.starts_with(&package) && candidate.is_file() {
            return Err(err(
                "artifact.uncovered",
                format!(
                    "the launch command runs {}, which the signature does not cover",
                    candidate.display()
                ),
            ));
        }
        Ok(None)
    };
    if resolved.program.components().count() > 1 {
        if let Some(staged) = check(resolved.program.clone(), None)? {
            out.program = staged;
        }
    }
    for arg in &mut out.args {
        let candidate = if Path::new(arg.as_str()).is_absolute() {
            PathBuf::from(arg.as_str())
        } else {
            resolved.cwd.join(arg.as_str())
        };
        check(candidate, Some(arg))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_rejects_junk() {
        assert_eq!(unhex("00ff10"), Some(vec![0, 255, 16]));
        assert_eq!(unhex("0"), None);
        assert_eq!(unhex("zz"), None);
        assert_eq!(unhex("é1"), None);
    }

    #[test]
    fn package_paths_stay_inside() {
        assert_eq!(package_path("a/b.py").unwrap(), "a/b.py");
        assert_eq!(package_path("./a.py").unwrap(), "a.py");
        for bad in ["../a.py", "/etc/passwd", "a/../../b", "", "."] {
            assert!(package_path(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn key_ids_are_derived_from_the_key() {
        let a = generate_keypair().unwrap();
        let b = generate_keypair().unwrap();
        assert_ne!(a.key_id, b.key_id);
        assert_eq!(key_id(&a.public_key).unwrap(), a.key_id);
        assert!(key_id("abcd").is_err());
    }

    #[test]
    fn registry_adds_revokes_and_refuses_duplicates() {
        let k = generate_keypair().unwrap();
        let mut store = TrustStore::empty();
        let added = store.add(&k.public_key, "release key", vec![], 1).unwrap();
        assert_eq!(added.adapters, vec!["*".to_owned()]);
        assert!(store.add(&k.public_key, "again", vec![], 2).is_err());
        assert!(store.add(&k.public_key[..10], "short", vec![], 2).is_err());
        assert!(store.revoke(&k.key_id, 3).unwrap());
        assert!(!store.revoke(&k.key_id, 4).unwrap(), "already revoked");
        assert_eq!(store.keys()[0].revoked_at_ms, Some(3));
        assert!(store.revoke("nope", 5).is_err());
    }
}
