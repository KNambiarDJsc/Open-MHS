use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openmhs_types::Descriptor;
use openmhs_types::descriptor::{Capability, Operation};
use openmhs_types::value::Value;
use serde::Deserialize;
use serde::de::DeserializeOwned;

/// The profile format version this crate reads.
pub const FORMAT_VERSION: &str = "0.1";

/// How one capability is reached over a protocol. Implemented by each adapter's mapping
/// type, so the loader can reject a mapping that does not fit its capability.
pub trait Mapping {
    /// Problems with this mapping for `cap`, e.g. a writable property mapped to a
    /// read-only register.
    fn check(&self, cap: &Capability) -> Result<(), String>;
}

/// Settings for the in-repo simulator serving this profile. Ignored by the adapter.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimSettings {
    /// Starting values, by capability. Others start at the middle of their range,
    /// `false`, the first enum value, or an empty string.
    #[serde(default)]
    pub initial: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw<C, M> {
    openmhs_profile: String,
    protocol: String,
    descriptor: serde_json::Value,
    connection: C,
    mapping: BTreeMap<String, M>,
    #[serde(default)]
    simulator: SimSettings,
}

/// A validated profile.
#[derive(Debug, Clone)]
pub struct Loaded<C, M> {
    pub descriptor: Descriptor,
    /// The descriptor as written, which the adapter returns from `describe`.
    pub descriptor_json: serde_json::Value,
    pub connection: C,
    pub mapping: BTreeMap<String, M>,
    pub simulator: SimSettings,
    /// The directory relative file names in the profile (TLS certificates) are relative
    /// to: the profile's own. Empty, meaning the working directory, until [`Loaded::in_dir`].
    pub dir: PathBuf,
}

impl<C, M> Loaded<C, M> {
    /// Resolve the profile's relative file names against `dir`.
    #[must_use]
    pub fn in_dir(mut self, dir: &Path) -> Self {
        self.dir = dir.to_path_buf();
        self
    }

    /// The mapping and capability for `name`, if both exist.
    pub fn get(&self, name: &str) -> Option<(&Capability, &M)> {
        Some((self.descriptor.capability(name)?, self.mapping.get(name)?))
    }
}

/// Everything wrong with a profile, each problem with a JSON pointer to where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileError {
    pub problems: Vec<(String, String)>,
}

impl ProfileError {
    fn one(at: &str, message: impl Into<String>) -> Self {
        Self {
            problems: vec![(at.to_owned(), message.into())],
        }
    }
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid profile:")?;
        for (at, message) in &self.problems {
            let at = if at.is_empty() { "(root)" } else { at };
            write!(f, "\n  {at}: {message}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ProfileError {}

/// The `protocol` a profile declares, to pick the adapter that loads it.
pub fn protocol_of(json: &str) -> Result<String, ProfileError> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| ProfileError::one("", format!("not JSON: {e}")))?;
    v.get("protocol")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ProfileError::one("/protocol", "missing; e.g. \"modbus-tcp\""))
}

/// Parse and validate a profile for `protocol`.
pub fn load<C, M>(json: &str, protocol: &str) -> Result<Loaded<C, M>, ProfileError>
where
    C: DeserializeOwned,
    M: DeserializeOwned + Mapping,
{
    let raw: Raw<C, M> = serde_json::from_str(json).map_err(|e| {
        ProfileError::one(
            "",
            format!("does not match the {protocol} profile format: {e}"),
        )
    })?;
    if raw.openmhs_profile != FORMAT_VERSION {
        return Err(ProfileError::one(
            "/openmhs_profile",
            format!(
                "version '{}' is not supported; expected '{FORMAT_VERSION}'",
                raw.openmhs_profile
            ),
        ));
    }
    if raw.protocol != protocol {
        return Err(ProfileError::one(
            "/protocol",
            format!("'{}' is not a {protocol} profile", raw.protocol),
        ));
    }
    let descriptor =
        openmhs_types::validate_descriptor(&raw.descriptor).map_err(|issues| ProfileError {
            problems: issues
                .into_iter()
                .map(|i| {
                    (
                        format!("/descriptor{}", i.pointer),
                        format!("[{}] {}", i.code, i.message),
                    )
                })
                .collect(),
        })?;

    let mut problems = Vec::new();
    for (name, m) in &raw.mapping {
        match descriptor.capability(name) {
            None => problems.push((
                format!("/mapping/{name}"),
                "the descriptor declares no such capability".to_owned(),
            )),
            Some(cap) => {
                if let Err(e) = m.check(cap) {
                    problems.push((format!("/mapping/{name}"), e));
                }
            }
        }
    }
    for cap in &descriptor.capabilities {
        let reachable = cap
            .operations
            .iter()
            .any(|op| matches!(op, Operation::Read | Operation::Write | Operation::Invoke));
        if reachable && !raw.mapping.contains_key(&cap.name) {
            problems.push((
                format!("/mapping/{}", cap.name),
                "missing: every readable, writable or invokable capability needs a mapping"
                    .to_owned(),
            ));
        }
    }
    for (name, value) in &raw.simulator.initial {
        match descriptor.capability(name).and_then(|c| c.value.as_ref()) {
            None => problems.push((
                format!("/simulator/initial/{name}"),
                "not a property with a value".to_owned(),
            )),
            Some(spec) => {
                if let Err(e) = crate::codec::check(spec, value) {
                    problems.push((format!("/simulator/initial/{name}"), e));
                }
            }
        }
    }
    if !problems.is_empty() {
        return Err(ProfileError { problems });
    }
    Ok(Loaded {
        descriptor,
        descriptor_json: raw.descriptor,
        connection: raw.connection,
        mapping: raw.mapping,
        simulator: raw.simulator,
        dir: PathBuf::new(),
    })
}
