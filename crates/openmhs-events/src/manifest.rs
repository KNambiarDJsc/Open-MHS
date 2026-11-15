//! Deployment manifests (RFC 0005 §3): what runs, content-addressed.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::canonical::{canonical_json, sha256_digest};
use crate::check_major;
use crate::error::RecordError;

pub const COMPONENT_KINDS: &[&str] = &[
    "software",
    "model",
    "firmware",
    "hardware",
    "calibration",
    "config",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// `sha256:` of the canonical JSON: two identical manifests are one record.
    pub digest: String,
    pub body: String,
    pub labels: BTreeMap<String, String>,
    /// (kind, name) → canonical JSON of the component.
    pub components: BTreeMap<(String, String), String>,
}

/// One component that differs between two manifests.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ComponentChange {
    pub kind: String,
    pub name: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

impl Manifest {
    pub fn parse(v: &Value) -> Result<Self, RecordError> {
        let schema = v
            .get("schema")
            .and_then(Value::as_str)
            .ok_or_else(|| RecordError::new("manifest.field", "`schema` is required"))?;
        check_major(schema, "openmhs.deployment/")?;
        let items = v
            .get("components")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty())
            .ok_or_else(|| {
                RecordError::new("manifest.field", "`components` must be a non-empty array")
            })?;
        let mut components = BTreeMap::new();
        for c in items {
            let kind = c.get("kind").and_then(Value::as_str).unwrap_or_default();
            let name = c.get("name").and_then(Value::as_str).unwrap_or_default();
            if !COMPONENT_KINDS.contains(&kind) || name.is_empty() {
                return Err(RecordError::new(
                    "manifest.component",
                    format!(
                        "every component needs a `name` and a `kind` in {}",
                        COMPONENT_KINDS.join(", ")
                    ),
                ));
            }
            if kind == "model"
                && c.get("revision").is_none()
                && c.get("checkpoint_digest").is_none()
            {
                return Err(RecordError::new(
                    "manifest.component",
                    format!(
                        "model {name:?} needs an immutable `revision` or `checkpoint_digest`, not only a tag"
                    ),
                ));
            }
            let key = (kind.to_owned(), name.to_owned());
            if components.insert(key, canonical_json(c)).is_some() {
                return Err(RecordError::new(
                    "manifest.component",
                    format!("component {kind}/{name} appears twice"),
                ));
            }
        }
        let mut labels = BTreeMap::new();
        if let Some(map) = v.get("labels").and_then(Value::as_object) {
            for (k, val) in map {
                if let Some(s) = val.as_str() {
                    labels.insert(k.clone(), s.to_owned());
                }
            }
        }
        let body = canonical_json(v);
        Ok(Self {
            digest: sha256_digest(&body),
            body,
            labels,
            components,
        })
    }
}

/// Component-wise difference from `before` to `after`.
pub fn diff(before: &Manifest, after: &Manifest) -> Vec<ComponentChange> {
    let mut keys: Vec<&(String, String)> = before
        .components
        .keys()
        .chain(after.components.keys())
        .collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter_map(|key| {
            let b = before.components.get(key);
            let a = after.components.get(key);
            (b != a).then(|| ComponentChange {
                kind: key.0.clone(),
                name: key.1.clone(),
                before: b.cloned(),
                after: a.cloned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest(policy: &str) -> Value {
        json!({"schema": "openmhs.deployment/0.1",
               "components": [
                   {"kind": "software", "name": "pick-stack", "version": "2.14.0", "revision": "3f9c2e1"},
                   {"kind": "model", "name": "grasp-policy", "revision": policy}],
               "labels": {"release": "2026.11.2"}})
    }

    #[test]
    fn identical_manifests_have_one_digest_whatever_the_key_order() {
        let a = Manifest::parse(&manifest("a1b2c3d")).unwrap_or_else(|e| panic!("{e}"));
        let text = r#"{"labels":{"release":"2026.11.2"},"components":[{"revision":"3f9c2e1","version":"2.14.0","name":"pick-stack","kind":"software"},{"revision":"a1b2c3d","name":"grasp-policy","kind":"model"}],"schema":"openmhs.deployment/0.1"}"#;
        let b = Manifest::parse(&serde_json::from_str(text).unwrap_or_default())
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(a.digest, b.digest);
        assert_eq!(
            a.labels.get("release").map(String::as_str),
            Some("2026.11.2")
        );
    }

    #[test]
    fn the_diff_names_only_what_changed() {
        let a = Manifest::parse(&manifest("9e8d7c6")).unwrap_or_else(|e| panic!("{e}"));
        let b = Manifest::parse(&manifest("a1b2c3d")).unwrap_or_else(|e| panic!("{e}"));
        let changes = diff(&a, &b);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].name, "grasp-policy");
        assert!(
            changes[0]
                .before
                .as_deref()
                .unwrap_or_default()
                .contains("9e8d7c6")
        );
    }

    #[test]
    fn a_model_without_an_immutable_revision_is_refused() {
        let v = json!({"schema": "openmhs.deployment/0.1",
                       "components": [{"kind": "model", "name": "grasp-policy", "tag": "latest"}]});
        assert_eq!(
            Manifest::parse(&v).map_err(|e| e.code),
            Err("manifest.component")
        );
    }
}
