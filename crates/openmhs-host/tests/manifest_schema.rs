//! The manifest schema and the host's parser agree on every manifest in the repository.

use std::path::{Path, PathBuf};

use jsonschema::{Registry, Resource};
use openmhs_host::{MANIFEST_SCHEMA, Manifest};
use serde_json::{Value, json};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn validator() -> jsonschema::Validator {
    let common: Value =
        serde_json::from_str(openmhs_types::schema::COMMON_SCHEMA).expect("common schema");
    let registry = Registry::new()
        .add(
            "urn:openmhs:schema:0.1:common",
            Resource::from_contents(common),
        )
        .expect("register common")
        .prepare()
        .expect("registry");
    jsonschema::draft202012::options()
        .with_registry(&registry)
        .build(&serde_json::from_str(MANIFEST_SCHEMA).expect("manifest schema JSON"))
        .expect("manifest schema compiles")
}

fn manifests() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in ["adapters/python", "spec/examples/manifests"] {
        let Ok(entries) = std::fs::read_dir(repo().join(dir)) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                // manifest.json, manifest.sim.json, …
                let Ok(files) = std::fs::read_dir(&p) else {
                    continue;
                };
                out.extend(files.flatten().map(|f| f.path()).filter(|f| {
                    let name = f.file_name().unwrap_or_default().to_string_lossy();
                    name.starts_with("manifest") && name.ends_with(".json")
                }));
            } else if p.extension().is_some_and(|x| x == "json") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn every_manifest_in_the_repository_is_valid() {
    let v = validator();
    let found = manifests();
    assert!(!found.is_empty());
    for path in found {
        let text = std::fs::read_to_string(&path).unwrap();
        let doc: Value = serde_json::from_str(&text).unwrap();
        let errors: Vec<String> = v.iter_errors(&doc).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "{}: {errors:?}", path.display());
        Manifest::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
}

#[test]
fn schema_and_parser_reject_the_same_mistakes() {
    let v = validator();
    let base = json!({
        "openmhs_manifest": "0.1",
        "adapter": {"id": "x.y", "version": "0.1.0"},
        "runtime": {"command": ["python"]},
        "capabilities": {"allow": [{"name": "*", "operations": ["read"]}]},
    });
    assert!(v.is_valid(&base));
    Manifest::parse(&base.to_string()).unwrap();

    let broken = [
        ("/openmhs_manifest", json!("0.2")),
        ("/runtime/command", json!([])),
        ("/capabilities/allow", json!([])),
        ("/capabilities/allow/0/operations", json!([])),
        ("/credentials", json!(["not-a-name"])),
        ("/endpoints", json!({"network": ["localhost:4840"]})),
        ("/sandbox", json!(true)),
        ("/limits", json!({"max_message_bytes": 10})),
    ];
    for (pointer, value) in broken {
        let mut doc = base.clone();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        let target = if parent.is_empty() {
            &mut doc
        } else {
            doc.pointer_mut(parent).unwrap()
        };
        match target {
            Value::Object(m) => {
                m.insert(key.to_string(), value);
            }
            Value::Array(a) => a[key.parse::<usize>().unwrap()] = value,
            _ => unreachable!(),
        }
        assert!(!v.is_valid(&doc), "schema accepts {pointer}");
        assert!(
            Manifest::parse(&doc.to_string()).is_err(),
            "parser accepts {pointer}"
        );
    }
}
