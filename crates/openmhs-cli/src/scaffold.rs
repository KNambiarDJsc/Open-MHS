//! `openmhs adapter new`: create an adapter crate from `templates/adapter`.
//!
//! The template is a real workspace crate, built and conformance-tested with everything
//! else, so what this writes is known to work. Only the package name, the adapter id and
//! the device id change; the Cargo.toml is written fresh because the template's own uses
//! workspace inheritance.

use std::path::{Path, PathBuf};

const REPO: &str = "https://github.com/KNambiarDJsc/Open-MHS";
const TEMPLATE_CRATE: &str = "openmhs-adapter-template";
const TEMPLATE_ADAPTER_ID: &str = "example.lamp";
const TEMPLATE_DEVICE_ID: &str = "example.lamp-01";

/// Template files, by path in the new crate.
const FILES: &[(&str, &str)] = &[
    (
        "descriptor.json",
        include_str!("../../../templates/adapter/descriptor.json"),
    ),
    (
        "src/lib.rs",
        include_str!("../../../templates/adapter/src/lib.rs"),
    ),
    (
        "src/link.rs",
        include_str!("../../../templates/adapter/src/link.rs"),
    ),
    (
        "src/fixture.rs",
        include_str!("../../../templates/adapter/src/fixture.rs"),
    ),
    (
        "tests/conformance.rs",
        include_str!("../../../templates/adapter/tests/conformance.rs"),
    ),
    (
        "examples/serve.rs",
        include_str!("../../../templates/adapter/examples/serve.rs"),
    ),
    (
        "README.md",
        include_str!("../../../templates/adapter/README.md"),
    ),
    (".gitignore", "/target\n"),
];

/// The crates the template uses from OpenMHS.
const OPENMHS_DEPS: &[&str] = &["openmhs-adapter", "openmhs-conformance", "openmhs-types"];
const OPENMHS_DEV_DEPS: &[&str] = &["openmhs-gateway"];

/// Where the OpenMHS crates come from.
pub enum Source {
    /// A local checkout of the repository.
    Path(PathBuf),
    /// The repository's `main` branch.
    Git,
}

/// Whether `name` can name the crate, the adapter and the device.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() <= 48
        && !name.ends_with('-')
        && !name.contains("--")
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn dep(name: &str, source: &Source) -> String {
    match source {
        Source::Path(root) => {
            let dir = root.join("crates").join(name);
            // A TOML literal string, so Windows backslashes need no escaping.
            format!("{name} = {{ path = '{}' }}", dir.display())
        }
        Source::Git => format!("{name} = {{ git = \"{REPO}\", branch = \"main\" }}"),
    }
}

pub fn cargo_toml(name: &str, source: &Source) -> String {
    let deps: Vec<String> = OPENMHS_DEPS.iter().map(|d| dep(d, source)).collect();
    let dev: Vec<String> = OPENMHS_DEV_DEPS.iter().map(|d| dep(d, source)).collect();
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
async-trait = "0.1"
{deps}
serde_json = "1"
tokio = {{ version = "1", features = ["rt-multi-thread", "macros", "sync", "time", "net", "io-util"] }}

[dev-dependencies]
{dev}
tokio = {{ version = "1", features = ["test-util", "signal"] }}

# A crate of its own, even when created inside another workspace.
[workspace]
"#,
        deps = deps.join("\n"),
        dev = dev.join("\n"),
    )
}

/// The template's text with its names replaced by the new adapter's.
pub fn render(text: &str, name: &str) -> String {
    let lib = name.replace('-', "_");
    text.replace(TEMPLATE_DEVICE_ID, &format!("{name}-01"))
        .replace(TEMPLATE_ADAPTER_ID, name)
        .replace(&TEMPLATE_CRATE.replace('-', "_"), &lib)
        .replace(TEMPLATE_CRATE, name)
}

/// Write the new crate into `dir`, which must not exist or be empty.
pub fn create(dir: &Path, name: &str, source: &Source) -> Result<Vec<PathBuf>, String> {
    if !valid_name(name) {
        return Err(format!(
            "'{name}' is not a valid adapter name: use lowercase letters, digits and single hyphens, starting with a letter (e.g. bench-psu)"
        ));
    }
    if let Source::Path(root) = source
        && root.display().to_string().contains('\'')
    {
        return Err(format!(
            "{} contains a quote, which Cargo.toml cannot hold here",
            root.display()
        ));
    }
    if let Source::Path(root) = source
        && !root.join("crates/openmhs-adapter/Cargo.toml").is_file()
    {
        return Err(format!(
            "{} is not an OpenMHS checkout (no crates/openmhs-adapter/Cargo.toml)",
            root.display()
        ));
    }
    if dir.exists() {
        let empty = std::fs::read_dir(dir)
            .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
            .next()
            .is_none();
        if !empty {
            return Err(format!(
                "{} already exists and is not empty; nothing was written",
                dir.display()
            ));
        }
    }
    let mut written = Vec::new();
    let mut write = |rel: &str, text: &str| -> Result<(), String> {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        written.push(path);
        Ok(())
    };
    write("Cargo.toml", &cargo_toml(name, source))?;
    for (rel, text) in FILES {
        write(rel, &render(text, name))?;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dependency names in one `[section]` of a Cargo.toml.
    fn deps(toml: &str, section: &str) -> Vec<String> {
        let mut names: Vec<String> = toml
            .lines()
            .skip_while(|l| l.trim() != format!("[{section}]"))
            .skip(1)
            .take_while(|l| !l.starts_with('['))
            .filter_map(|l| l.split_once('='))
            .map(|(k, _)| k.trim().trim_end_matches(".workspace").to_owned())
            .filter(|k| !k.is_empty() && !k.starts_with('#'))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_generated_manifest_has_the_templates_dependencies() {
        let template = include_str!("../../../templates/adapter/Cargo.toml");
        let generated = cargo_toml("bench-psu", &Source::Git);
        for section in ["dependencies", "dev-dependencies"] {
            assert_eq!(
                deps(&generated, section),
                deps(template, section),
                "[{section}]"
            );
        }
    }

    #[test]
    fn names_are_replaced_everywhere() {
        let lib = render(FILES[1].1, "bench-psu");
        assert!(lib.contains("pub const ADAPTER_ID: &str = \"bench-psu\";"));
        let test = render(FILES[4].1, "bench-psu");
        assert!(test.contains("use bench_psu::LampAdapter;"), "{test}");
        let descriptor: serde_json::Value =
            serde_json::from_str(&render(FILES[0].1, "bench-psu")).unwrap();
        assert_eq!(descriptor["adapter"]["id"], "bench-psu");
        assert_eq!(descriptor["device"]["id"], "bench-psu-01");
        openmhs_types::validate_descriptor(&descriptor).unwrap();
        for (_, text) in FILES {
            let out = render(text, "bench-psu");
            for old in [
                TEMPLATE_CRATE,
                "openmhs_adapter_template",
                TEMPLATE_ADAPTER_ID,
            ] {
                assert!(!out.contains(old), "{old} left in output");
            }
        }
    }

    #[test]
    fn names_are_validated() {
        for ok in ["bench-psu", "lamp", "x9", "a-b-c"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in [
            "",
            "Bench",
            "9lamp",
            "lamp-",
            "a--b",
            "a_b",
            "a.b",
            &"x".repeat(49),
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn it_never_writes_into_a_non_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep.txt"), "mine").unwrap();
        let err = create(dir.path(), "lamp", &Source::Git).unwrap_err();
        assert!(err.contains("not empty"), "{err}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("keep.txt")).unwrap(),
            "mine"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn path_sources_must_be_checkouts() {
        let dir = tempfile::tempdir().unwrap();
        let err = create(
            &dir.path().join("x"),
            "lamp",
            &Source::Path(dir.path().into()),
        )
        .unwrap_err();
        assert!(err.contains("not an OpenMHS checkout"), "{err}");
    }
}
