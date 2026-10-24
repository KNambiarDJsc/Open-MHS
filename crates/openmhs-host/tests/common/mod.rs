//! Shared by the integration tests: finding Python and building manifests.
//!
//! These tests need Python 3.10+. They fail, rather than skip, when it is missing, so a
//! green run always means a real adapter process was exercised. `OPENMHS_PYTHON` picks
//! the interpreter.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use openmhs_host::{Manifest, Resolved};
use serde_json::{Value, json};

pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn python() -> String {
    if let Ok(p) = std::env::var("OPENMHS_PYTHON") {
        return p;
    }
    let candidates: &[&str] = if cfg!(windows) {
        &["python", "python3"]
    } else {
        &["python3", "python"]
    };
    for c in candidates {
        let ok = std::process::Command::new(c)
            .args(["-c", "import sys; assert sys.version_info >= (3, 10)"])
            .output()
            .is_ok_and(|o| o.status.success());
        if ok {
            return c.to_string();
        }
    }
    panic!("these tests need Python 3.10 or later on PATH (or OPENMHS_PYTHON)");
}

fn merge(base: &mut Value, extra: Value) {
    match (base, extra) {
        (Value::Object(b), Value::Object(e)) => {
            for (k, v) in e {
                merge(b.entry(k).or_insert(Value::Null), v);
            }
        }
        (b, e) => *b = e,
    }
}

/// A manifest for tests/adapters/misbehave.py in `mode`, with `extra` merged in.
pub fn misbehave(mode: &str, extra: Value) -> Resolved {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/adapters");
    let mut m = json!({
        "openmhs_manifest": "0.1",
        "adapter": {"id": "test.misbehave", "version": "0.1.0"},
        "runtime": {
            "command": [python(), "{manifest_dir}/misbehave.py"],
            "env": {"PYTHONPATH": repo().join("python").display().to_string()},
        },
        "capabilities": {"allow": [
            {"name": "level.*", "operations": ["read"]},
            {"name": "dose.run", "operations": ["invoke"]},
        ]},
        "config": {"mode": mode},
        "limits": {"heartbeat_ms": 500, "missed_heartbeats": 3, "call_timeout_ms": 5000},
        "restart": {"max_restarts": 3, "window_s": 60, "initial_backoff_ms": 20, "max_backoff_ms": 200},
    });
    merge(&mut m, extra);
    let manifest = Manifest::parse(&m.to_string()).expect("test manifest");
    Resolved::new(manifest, &dir)
}

/// The Python PSU simulator's manifest, run with the interpreter found here.
pub fn psu_sim() -> Resolved {
    let path = repo().join("adapters/python/psu-sim/manifest.json");
    let mut r = Resolved::load(&path).expect("psu-sim manifest");
    r.program = PathBuf::from(python());
    r
}
