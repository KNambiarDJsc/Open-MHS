//! Files the gateway keeps in its home directory: operator policy and API tokens.
//!
//! Both are small JSON documents rewritten atomically (write a temporary file, then
//! rename over the original). Tokens are stored only as SHA-256 hashes, compared in
//! constant time; revoked tokens stay on record so their use can be told from a guess.

use std::path::{Path, PathBuf};

use openmhs_api::{Role, TokenInfo};
use openmhs_engine::journal::sha256_hex;
use openmhs_policy::{Grant, Policy, PrincipalKind, Scope};
use openmhs_types::{ErrorObject, Timestamp};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

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

/// What a presented secret turned out to be.
#[derive(Debug)]
pub enum Lookup<'a> {
    Valid(&'a TokenInfo),
    Expired(&'a TokenInfo),
    Revoked(&'a TokenInfo),
    Unknown,
}

/// Why a credential could not be issued.
#[derive(Debug)]
pub enum TokenError {
    Rejected(ErrorObject),
    Storage(GatewayError),
}

impl From<GatewayError> for TokenError {
    fn from(e: GatewayError) -> Self {
        Self::Storage(e)
    }
}

fn rejected(code: &str, message: impl Into<String>) -> TokenError {
    TokenError::Rejected(ErrorObject::new(code, message))
}

/// What the operator asks for when issuing a credential.
pub struct NewToken<'a> {
    pub principal: &'a str,
    pub role: Role,
    pub scopes: Vec<Scope>,
    pub ttl_ms: Option<u64>,
}

pub struct TokenStore {
    path: PathBuf,
    file: TokenFile,
    /// The gateway's own operator principal, which no stored credential may take over.
    reserved: String,
}

/// A random credential id. Independent of the secret, so showing it reveals nothing.
fn new_id() -> Result<String, GatewayError> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes)
        .map_err(|e| GatewayError::Io(std::io::Error::other(e.to_string())))?;
    Ok(format!("tok_{}", hex(&bytes)))
}

/// Scopes name a device, a capability and operations, like a grant does.
pub fn validate_scope(scope: &Scope) -> Result<(), ErrorObject> {
    for (field, value) in [("device", &scope.device), ("capability", &scope.capability)] {
        if value.trim().is_empty() {
            return Err(ErrorObject::new(
                "credential.scope.invalid",
                format!("scope field '{field}' must not be empty"),
            ));
        }
    }
    if scope.operations.is_empty() {
        return Err(ErrorObject::new(
            "credential.scope.invalid",
            "a scope needs at least one operation",
        ));
    }
    Ok(())
}

impl TokenStore {
    pub fn open(home: &Path, reserved_principal: &str) -> Result<Self, GatewayError> {
        let path = home.join(TOKENS_FILE);
        let mut file: TokenFile = if path.exists() {
            let text = std::fs::read_to_string(&path)?;
            serde_json::from_str(&text)
                .map_err(|e| GatewayError::Config(format!("{}: {e}", path.display())))?
        } else {
            TokenFile::default()
        };
        // Records written before M8 carry no kind; it follows from the principal id.
        for t in &mut file.tokens {
            t.info.kind = PrincipalKind::of(&t.info.principal);
        }
        Ok(Self {
            path,
            file,
            reserved: reserved_principal.to_owned(),
        })
    }

    fn save(&self) -> Result<(), GatewayError> {
        let text = serde_json::to_string_pretty(&self.file)
            .map_err(|e| GatewayError::Config(e.to_string()))?;
        write_private(&self.path, &(text + "\n"))?;
        Ok(())
    }

    /// Classify a presented secret. Every stored verifier is compared, in constant time,
    /// so how long this takes does not depend on which one (if any) matched.
    pub fn lookup(&self, token: &str, now: Timestamp) -> Lookup<'_> {
        let presented = sha256_hex(token);
        let mut found: Option<&StoredToken> = None;
        for t in &self.file.tokens {
            if bool::from(t.sha256.as_bytes().ct_eq(presented.as_bytes())) {
                found = Some(t);
            }
        }
        match found {
            None => Lookup::Unknown,
            Some(t) if t.info.is_revoked() => Lookup::Revoked(&t.info),
            Some(t) if t.info.is_expired(now) => Lookup::Expired(&t.info),
            Some(t) => Lookup::Valid(&t.info),
        }
    }

    pub fn list(&self) -> Vec<TokenInfo> {
        self.file.tokens.iter().map(|t| t.info.clone()).collect()
    }

    pub fn get(&self, id: &str) -> Option<&TokenInfo> {
        self.file
            .tokens
            .iter()
            .find(|t| t.info.id == id)
            .map(|t| &t.info)
    }

    /// Issue a credential. The secret is returned once and only its hash is kept.
    ///
    /// Refused: an invalid principal or scope; a principal that already holds a
    /// credential with a different role; the gateway's own operator principal; and an
    /// agent principal with the operator role, so an agent can never administer
    /// the gateway or approve.
    pub fn create(
        &mut self,
        new: NewToken<'_>,
        now: Timestamp,
    ) -> Result<(TokenInfo, String), TokenError> {
        if !openmhs_types::is_principal(new.principal) {
            return Err(rejected(
                "credential.principal.invalid",
                "principal must be a lowercase name with an optional kind, e.g. agent:lab-assistant",
            ));
        }
        if new.principal == self.reserved {
            return Err(rejected(
                "credential.principal.reserved",
                format!(
                    "'{}' is the gateway's operator and is authenticated by the runtime file only",
                    new.principal
                ),
            ));
        }
        let kind = PrincipalKind::of(new.principal);
        if kind == PrincipalKind::Agent && new.role == Role::Operator {
            return Err(rejected(
                "credential.role.invalid",
                "an agent principal cannot hold the operator role",
            ));
        }
        if let Some(other) = self
            .file
            .tokens
            .iter()
            .find(|t| t.info.principal == new.principal && t.info.role != new.role)
        {
            return Err(rejected(
                "credential.role.conflict",
                format!(
                    "'{}' already holds credential {} with a different role",
                    new.principal, other.info.id
                ),
            ));
        }
        for scope in &new.scopes {
            validate_scope(scope).map_err(TokenError::Rejected)?;
        }

        let token = new_token()?;
        let info = TokenInfo {
            id: new_id()?,
            principal: new.principal.to_owned(),
            role: new.role,
            kind,
            created_at: now,
            expires_at: new.ttl_ms.map(|ms| now.plus_ms(ms)),
            scopes: new.scopes,
            revoked_at: None,
        };
        self.file.tokens.push(StoredToken {
            info: info.clone(),
            sha256: sha256_hex(&token),
        });
        self.save()?;
        Ok((info, token))
    }

    /// Revoke a credential. It stays on record, revoked, so later use of it is
    /// recognizable. Returns the credential and whether this call revoked it.
    pub fn revoke(
        &mut self,
        id: &str,
        now: Timestamp,
    ) -> Result<Option<(TokenInfo, bool)>, GatewayError> {
        let Some(t) = self.file.tokens.iter_mut().find(|t| t.info.id == id) else {
            return Ok(None);
        };
        let newly = !t.info.is_revoked();
        if newly {
            t.info.revoked_at = Some(now);
        }
        let info = t.info.clone();
        if newly {
            self.save()?;
        }
        Ok(Some((info, newly)))
    }

    /// Replace a credential with a new secret for the same principal, role and scopes,
    /// revoking the old one in the same write. With no `ttl_ms`, the new credential lives
    /// as long as the old one was meant to.
    pub fn rotate(
        &mut self,
        id: &str,
        ttl_ms: Option<u64>,
        now: Timestamp,
    ) -> Result<(TokenInfo, String, TokenInfo), TokenError> {
        let Some(old) = self.get(id).cloned() else {
            return Err(rejected(
                "gateway.token.unknown",
                format!("no token with id '{id}'"),
            ));
        };
        if old.is_revoked() {
            return Err(rejected(
                "credential.revoked",
                format!("{id} is revoked and cannot be rotated"),
            ));
        }
        let ttl_ms = ttl_ms.or_else(|| {
            old.expires_at
                .map(|e| u64::try_from(old.created_at.ms_until(e)).unwrap_or(0))
        });
        let token = new_token()?;
        let info = TokenInfo {
            id: new_id()?,
            principal: old.principal.clone(),
            role: old.role,
            kind: old.kind,
            created_at: now,
            expires_at: ttl_ms.map(|ms| now.plus_ms(ms)),
            scopes: old.scopes.clone(),
            revoked_at: None,
        };
        for t in &mut self.file.tokens {
            if t.info.id == id {
                t.info.revoked_at = Some(now);
            }
        }
        self.file.tokens.push(StoredToken {
            info: info.clone(),
            sha256: sha256_hex(&token),
        });
        self.save()?;
        let mut revoked = old;
        revoked.revoked_at = Some(now);
        Ok((info, token, revoked))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmhs_types::descriptor::Operation;

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    fn scope() -> Scope {
        Scope {
            device: "sim.psu-*".into(),
            capability: "*".into(),
            operations: vec![Operation::Read],
        }
    }

    fn new<'a>(principal: &'a str, role: Role) -> NewToken<'a> {
        NewToken {
            principal,
            role,
            scopes: vec![scope()],
            ttl_ms: None,
        }
    }

    fn store(dir: &tempfile::TempDir) -> TokenStore {
        TokenStore::open(dir.path(), "operator:local").unwrap()
    }

    #[test]
    fn tokens_are_random_hashed_and_revocable() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store(&dir);
        let (info, secret) = store.create(new("agent:a", Role::Agent), ts(0)).unwrap();
        let (_, other) = store.create(new("agent:b", Role::Agent), ts(0)).unwrap();
        assert_ne!(secret, other);
        assert!(secret.starts_with("omhs_") && secret.len() == 5 + 64);

        let on_disk = std::fs::read_to_string(dir.path().join(TOKENS_FILE)).unwrap();
        assert!(!on_disk.contains(&secret), "secret must not be stored");
        assert!(on_disk.contains(&sha256_hex(&secret)), "the hash is stored");

        let reopened = TokenStore::open(dir.path(), "operator:local").unwrap();
        assert!(
            matches!(reopened.lookup(&secret, ts(1)), Lookup::Valid(t) if t.principal == "agent:a")
        );
        assert!(matches!(
            reopened.lookup("omhs_wrong", ts(1)),
            Lookup::Unknown
        ));

        store.revoke(&info.id, ts(2)).unwrap().unwrap();
        let after = TokenStore::open(dir.path(), "operator:local").unwrap();
        assert!(matches!(after.lookup(&secret, ts(3)), Lookup::Revoked(_)));
    }

    #[test]
    fn ids_are_not_derived_from_the_secret() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store(&dir);
        let (info, secret) = store.create(new("agent:a", Role::Agent), ts(0)).unwrap();
        assert!(info.id.starts_with("tok_") && info.id.len() == 4 + 16);
        assert!(!sha256_hex(&secret).contains(&info.id[4..]));
    }

    #[test]
    fn credentials_expire_exactly_at_their_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store(&dir);
        let mut spec = new("agent:a", Role::Agent);
        spec.ttl_ms = Some(1_000);
        let (_, secret) = store.create(spec, ts(10)).unwrap();
        assert!(matches!(store.lookup(&secret, ts(1_009)), Lookup::Valid(_)));
        assert!(matches!(
            store.lookup(&secret, ts(1_010)),
            Lookup::Expired(_)
        ));
    }

    #[test]
    fn revoking_twice_is_harmless_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store(&dir);
        let (info, _) = store.create(new("agent:a", Role::Agent), ts(0)).unwrap();
        assert!(store.revoke(&info.id, ts(1)).unwrap().unwrap().1);
        let (again, newly) = store.revoke(&info.id, ts(9)).unwrap().unwrap();
        assert!(!newly);
        assert_eq!(
            again.revoked_at,
            Some(ts(1)),
            "the first revocation time stands"
        );
        assert!(store.revoke("tok_nope", ts(9)).unwrap().is_none());
    }

    #[test]
    fn rotation_replaces_the_secret_and_keeps_the_scopes() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store(&dir);
        let mut spec = new("agent:a", Role::Agent);
        spec.ttl_ms = Some(5_000);
        let (old, old_secret) = store.create(spec, ts(0)).unwrap();
        let (fresh, fresh_secret, revoked) = store.rotate(&old.id, None, ts(100)).unwrap();
        assert_ne!(old_secret, fresh_secret);
        assert_eq!(fresh.scopes, old.scopes);
        assert_eq!(fresh.expires_at, Some(ts(5_100)), "same lifetime as before");
        assert!(revoked.is_revoked());
        assert!(matches!(
            store.lookup(&old_secret, ts(101)),
            Lookup::Revoked(_)
        ));
        assert!(matches!(
            store.lookup(&fresh_secret, ts(101)),
            Lookup::Valid(_)
        ));
        assert!(matches!(
            store.rotate(&old.id, None, ts(102)),
            Err(TokenError::Rejected(e)) if e.code == "credential.revoked"
        ));
    }

    #[test]
    fn issuing_refuses_impersonation_and_privilege_mixing() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store(&dir);
        let code = |r: Result<_, TokenError>| match r {
            Err(TokenError::Rejected(e)) => e.code,
            Ok(_) => "ok".into(),
            Err(TokenError::Storage(e)) => format!("storage: {e}"),
        };
        // The gateway's operator is authenticated by the runtime file alone.
        assert_eq!(
            code(store.create(new("operator:local", Role::Operator), ts(0))),
            "credential.principal.reserved"
        );
        // An agent name never carries the operator role.
        assert_eq!(
            code(store.create(new("agent:evil", Role::Operator), ts(0))),
            "credential.role.invalid"
        );
        // One principal, one role.
        store.create(new("alice", Role::Agent), ts(0)).unwrap();
        assert_eq!(
            code(store.create(new("alice", Role::Operator), ts(0))),
            "credential.role.conflict"
        );
        assert_eq!(
            code(store.create(new("Not A Principal", Role::Agent), ts(0))),
            "credential.principal.invalid"
        );
        let mut empty_ops = new("agent:x", Role::Agent);
        empty_ops.scopes[0].operations.clear();
        assert_eq!(
            code(store.create(empty_ops, ts(0))),
            "credential.scope.invalid"
        );
    }

    #[test]
    fn tokens_written_before_m8_load_but_authorize_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = format!(
            r#"{{"tokens":[{{"id":"tok_0123456789ab","principal":"agent:old","role":"agent","created_at":"1970-01-01T00:00:00.000Z","sha256":"{}"}}]}}"#,
            sha256_hex("omhs_legacy")
        );
        std::fs::write(dir.path().join(TOKENS_FILE), legacy).unwrap();
        let store = store(&dir);
        let Lookup::Valid(info) = store.lookup("omhs_legacy", ts(1)) else {
            panic!("a legacy token still authenticates");
        };
        assert!(info.scopes.is_empty(), "but it has no scopes");
        assert_eq!(info.kind, PrincipalKind::Agent);
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
