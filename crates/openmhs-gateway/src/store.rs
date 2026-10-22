//! Files the gateway keeps in its home directory: operator policy and API tokens.
//!
//! Both are small JSON documents rewritten atomically (write a temporary file, then
//! rename over the original). Tokens are stored only as SHA-256 hashes.

use std::path::{Path, PathBuf};

use openmhs_api::{Role, TokenInfo};
use openmhs_engine::journal::sha256_hex;
use openmhs_policy::{Grant, Policy};
use openmhs_types::{ErrorObject, Timestamp};
use serde::{Deserialize, Serialize};

use crate::GatewayError;

pub const POLICY_FILE: &str = "policy.json";
pub const TOKENS_FILE: &str = "tokens.json";

/// Write `contents` to `path` atomically. On Unix the file is created `0600`.
pub fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

/// A random API token: 256 bits from the OS, hex-encoded, with a recognizable prefix.
pub fn new_token() -> Result<String, GatewayError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| GatewayError::Io(std::io::Error::other(e.to_string())))?;
    Ok(format!("omhs_{}", hex(&bytes)))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ------------------------------------------------------------------ policy

pub struct PolicyStore {
    path: PathBuf,
}

impl PolicyStore {
    /// Open the policy file, creating it on first start: no grants (everything denied)
    /// and the operator as the only approver.
    pub fn open(home: &Path, operator: &str) -> Result<(Self, Policy), GatewayError> {
        let path = home.join(POLICY_FILE);
        let store = Self { path };
        if !store.path.exists() {
            let policy = Policy {
                grants: Vec::new(),
                approvers: vec![operator.to_owned()],
            };
            store.save(&policy)?;
            return Ok((store, policy));
        }
        let text = std::fs::read_to_string(&store.path)?;
        let policy: Policy = serde_json::from_str(&text)
            .map_err(|e| GatewayError::Config(format!("{}: {e}", store.path.display())))?;
        validate_policy(&policy).map_err(|e| {
            GatewayError::Config(format!("{}: {}", store.path.display(), e.message))
        })?;
        Ok((store, policy))
    }

    pub fn save(&self, policy: &Policy) -> Result<(), GatewayError> {
        let text = serde_json::to_string_pretty(policy)
            .map_err(|e| GatewayError::Config(e.to_string()))?;
        write_private(&self.path, &(text + "\n"))?;
        Ok(())
    }
}

/// Checks that keep a policy meaningful: unique, non-empty ids and patterns.
pub fn validate_policy(policy: &Policy) -> Result<(), ErrorObject> {
    let mut seen = std::collections::HashSet::new();
    for g in &policy.grants {
        validate_grant(g)?;
        if !seen.insert(g.id.as_str()) {
            return Err(ErrorObject::new(
                "policy.grant.duplicate",
                format!("grant id '{}' is used twice", g.id),
            ));
        }
    }
    Ok(())
}

pub fn validate_grant(g: &Grant) -> Result<(), ErrorObject> {
    let empty = [
        ("id", &g.id),
        ("principal", &g.principal),
        ("device", &g.device),
        ("capability", &g.capability),
    ]
    .into_iter()
    .find(|(_, v)| v.trim().is_empty());
    if let Some((field, _)) = empty {
        return Err(ErrorObject::new(
            "policy.grant.invalid",
            format!("grant field '{field}' must not be empty"),
        )
        .at(format!("/grant/{field}")));
    }
    if g.operations.is_empty() {
        return Err(ErrorObject::new(
            "policy.grant.invalid",
            "a grant needs at least one operation",
        )
        .at("/grant/operations"));
    }
    Ok(())
}

// ------------------------------------------------------------------ tokens

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredToken {
    #[serde(flatten)]
    info: TokenInfo,
    sha256: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TokenFile {
    tokens: Vec<StoredToken>,
}

pub struct TokenStore {
    path: PathBuf,
    file: TokenFile,
}

impl TokenStore {
    pub fn open(home: &Path) -> Result<Self, GatewayError> {
        let path = home.join(TOKENS_FILE);
        let file = if path.exists() {
            let text = std::fs::read_to_string(&path)?;
            serde_json::from_str(&text)
                .map_err(|e| GatewayError::Config(format!("{}: {e}", path.display())))?
        } else {
            TokenFile::default()
        };
        Ok(Self { path, file })
    }

    fn save(&self) -> Result<(), GatewayError> {
        let text = serde_json::to_string_pretty(&self.file)
            .map_err(|e| GatewayError::Config(e.to_string()))?;
        write_private(&self.path, &(text + "\n"))?;
        Ok(())
    }

    pub fn lookup(&self, token: &str) -> Option<&TokenInfo> {
        let hash = sha256_hex(token);
        self.file
            .tokens
            .iter()
            .find(|t| t.sha256 == hash)
            .map(|t| &t.info)
    }

    pub fn list(&self) -> Vec<TokenInfo> {
        self.file.tokens.iter().map(|t| t.info.clone()).collect()
    }

    /// Create a token. The secret is returned once and only its hash is kept.
    pub fn create(
        &mut self,
        principal: &str,
        role: Role,
        now: Timestamp,
    ) -> Result<(TokenInfo, String), GatewayError> {
        let token = new_token()?;
        let hash = sha256_hex(&token);
        let info = TokenInfo {
            id: format!("tok_{}", &hash[..12]),
            principal: principal.to_owned(),
            role,
            created_at: now,
        };
        self.file.tokens.push(StoredToken {
            info: info.clone(),
            sha256: hash,
        });
        self.save()?;
        Ok((info, token))
    }

    pub fn revoke(&mut self, id: &str) -> Result<Option<TokenInfo>, GatewayError> {
        let Some(pos) = self.file.tokens.iter().position(|t| t.info.id == id) else {
            return Ok(None);
        };
        let removed = self.file.tokens.remove(pos);
        self.save()?;
        Ok(Some(removed.info))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_hashed_and_revocable() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = TokenStore::open(dir.path()).unwrap();
        let (info, secret) = store
            .create("agent:a", Role::Agent, Timestamp::from_millis(0))
            .unwrap();
        let (_, other) = store
            .create("agent:b", Role::Agent, Timestamp::from_millis(0))
            .unwrap();
        assert_ne!(secret, other);
        assert!(secret.starts_with("omhs_") && secret.len() == 5 + 64);

        let on_disk = std::fs::read_to_string(dir.path().join(TOKENS_FILE)).unwrap();
        assert!(!on_disk.contains(&secret), "secret must not be stored");

        let reopened = TokenStore::open(dir.path()).unwrap();
        assert_eq!(reopened.lookup(&secret).unwrap().principal, "agent:a");
        assert!(reopened.lookup("omhs_wrong").is_none());

        store.revoke(&info.id).unwrap().unwrap();
        assert!(
            TokenStore::open(dir.path())
                .unwrap()
                .lookup(&secret)
                .is_none()
        );
    }

    #[test]
    fn first_start_policy_denies_everything() {
        let dir = tempfile::tempdir().unwrap();
        let (_, policy) = PolicyStore::open(dir.path(), "operator:me").unwrap();
        assert!(policy.grants.is_empty());
        assert_eq!(policy.approvers, vec!["operator:me".to_owned()]);
        // Reopening reads the file rather than recreating it.
        let (_, again) = PolicyStore::open(dir.path(), "operator:other").unwrap();
        assert_eq!(again, policy);
    }
}
