//! M8: signed adapter manifests (ADR-0017). Each test builds a real package in a temporary
//! directory, signs it with a real key, and starts a real adapter process (or fails to).

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use openmhs_adapter::HardwareAdapter;
use openmhs_host::trust::{self, Keypair, TrustStore};
use openmhs_host::{HostOptions, ProcessAdapter, Resolved, Status, Trust};
use serde_json::{Value, json};

const SCRIPT: &str = "misbehave.py";

struct Package {
    dir: tempfile::TempDir,
    key: Keypair,
}

impl Package {
    /// A package holding the test adapter and a manifest for it, signed by a fresh key.
    fn signed(mode: &str, extra: Value) -> Self {
        let key = trust::generate_keypair().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/adapters")
            .join(SCRIPT);
        std::fs::copy(source, dir.path().join(SCRIPT)).unwrap();
        let mut manifest = common::misbehave_json(mode, extra);
        manifest["runtime"]["command"] = json!([common::python(), "{manifest_dir}/misbehave.py"]);
        let pkg = Self { dir, key };
        pkg.write_manifest(&manifest);
        pkg.sign_with(&pkg.key.seed.clone(), &[SCRIPT]);
        pkg
    }

    fn manifest_path(&self) -> PathBuf {
        self.dir.path().join("manifest.json")
    }

    fn write_manifest(&self, manifest: &Value) {
        std::fs::write(self.manifest_path(), manifest.to_string()).unwrap();
    }

    fn json(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.manifest_path()).unwrap()).unwrap()
    }

    fn sign_with(&self, seed: &str, artifacts: &[&str]) {
        let text = std::fs::read_to_string(self.manifest_path()).unwrap();
        let artifacts: Vec<String> = artifacts.iter().map(|a| (*a).to_owned()).collect();
        let signed = trust::sign_manifest(&text, self.dir.path(), seed, &artifacts).unwrap();
        std::fs::write(self.manifest_path(), signed).unwrap();
    }

    fn trusting(&self) -> TrustStore {
        let mut store = TrustStore::empty();
        store
            .add(&self.key.public_key, "test signer", vec![], 0)
            .unwrap();
        store
    }

    fn resolved(&self) -> Resolved {
        Resolved::load(&self.manifest_path()).unwrap()
    }

    async fn start(&self, store: TrustStore) -> Result<ProcessAdapter, String> {
        ProcessAdapter::start(
            self.resolved(),
            HostOptions {
                fixture: false,
                trust: Trust::Require(store),
            },
        )
        .await
    }
}

async fn refused(result: Result<ProcessAdapter, String>) -> String {
    match result {
        Ok(a) => {
            let _ = a.shutdown().await;
            panic!("the adapter must not have started");
        }
        Err(e) => e,
    }
}

/// A correctly signed manifest from a trusted key starts, and the adapter works.
#[tokio::test(flavor = "multi_thread")]
async fn a_correctly_signed_manifest_is_accepted() {
    let pkg = Package::signed("normal", json!({}));
    let adapter = pkg.start(pkg.trusting()).await.unwrap();
    adapter.read("level.measured").await.unwrap();
    assert_eq!(adapter.status(), Status::Running { generation: 1 });
    adapter.shutdown().await.unwrap();
}

/// The default host options trust no one, so an unsigned manifest does not start.
#[tokio::test(flavor = "multi_thread")]
async fn an_unsigned_manifest_is_refused_by_default() {
    let resolved = common::misbehave("normal", json!({}));
    let err = refused(ProcessAdapter::start(resolved, HostOptions::default()).await).await;
    assert!(err.contains("manifest.unsigned"), "{err}");
    assert!(
        err.contains("openmhs adapter sign"),
        "the error says what to do: {err}"
    );
}

/// Changing the manifest after it was signed breaks the signature, whatever was changed.
#[tokio::test(flavor = "multi_thread")]
async fn a_tampered_manifest_is_rejected() {
    for change in [
        // Widen what the adapter may do.
        (|m: &mut Value| m["capabilities"]["allow"][0]["operations"] = json!(["read", "write"]))
            as fn(&mut Value),
        // Hand it a different configuration.
        |m| m["config"]["mode"] = json!("crash_on_read"),
        // Point the signature at a different file digest.
        |m| m["signature"]["artifacts"][0]["sha256"] = json!("0".repeat(64)),
        // Allow a network endpoint it was not signed for.
        |m| m["endpoints"] = json!({"network": ["tcp://10.0.0.9:502"]}),
    ] {
        let pkg = Package::signed("normal", json!({}));
        let mut m = pkg.json();
        change(&mut m);
        pkg.write_manifest(&m);
        let err = refused(pkg.start(pkg.trusting()).await).await;
        assert!(err.contains("manifest.signature.invalid"), "{err}");
    }
}

/// A signature that is not even hex, or the wrong length, is refused cleanly.
#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_signature_is_rejected() {
    for bad in ["", "zz", "ab".repeat(10).as_str()] {
        let pkg = Package::signed("normal", json!({}));
        let mut m = pkg.json();
        m["signature"]["value"] = json!(bad);
        pkg.write_manifest(&m);
        let err = refused(pkg.start(pkg.trusting()).await).await;
        assert!(err.contains("manifest.signature.invalid"), "{err}");
    }
}

/// Changing the adapter's program after signing is caught, and nothing is launched.
#[tokio::test(flavor = "multi_thread")]
async fn a_modified_executable_is_rejected() {
    let pkg = Package::signed("normal", json!({}));
    let script = pkg.dir.path().join(SCRIPT);
    let mut text = std::fs::read_to_string(&script).unwrap();
    text.push_str("\nprint('backdoor')\n");
    std::fs::write(&script, text).unwrap();
    let err = refused(pkg.start(pkg.trusting()).await).await;
    assert!(err.contains("artifact.modified"), "{err}");
    assert!(err.contains(SCRIPT), "{err}");
}

/// A signed file that has gone missing, or been replaced by a link, is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_artifact_is_rejected() {
    let pkg = Package::signed("normal", json!({}));
    std::fs::remove_file(pkg.dir.path().join(SCRIPT)).unwrap();
    let err = refused(pkg.start(pkg.trusting()).await).await;
    assert!(err.contains("artifact.missing"), "{err}");
}

/// A launch command that runs something the signature does not cover is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_launch_command_outside_the_signature_is_rejected() {
    let pkg = Package::signed("normal", json!({}));
    std::fs::write(pkg.dir.path().join("other.py"), "print('x')").unwrap();
    // Sign only the other file: the script the command runs is not covered.
    pkg.sign_with(&pkg.key.seed.clone(), &["other.py"]);
    let err = refused(pkg.start(pkg.trusting()).await).await;
    assert!(err.contains("artifact.uncovered"), "{err}");
    assert!(err.contains(SCRIPT), "{err}");
}

/// A signer nobody trusts, a signer trusted only for other adapters, and a revoked signer
/// are all refused, each for its own reason.
#[tokio::test(flavor = "multi_thread")]
async fn unknown_revoked_and_out_of_scope_signers_are_rejected() {
    let pkg = Package::signed("normal", json!({}));

    let err = refused(pkg.start(TrustStore::empty()).await).await;
    assert!(err.contains("signer.unknown"), "{err}");

    let stranger = trust::generate_keypair().unwrap();
    let mut other = TrustStore::empty();
    other
        .add(&stranger.public_key, "someone else", vec![], 0)
        .unwrap();
    let err = refused(pkg.start(other).await).await;
    assert!(err.contains("signer.unknown"), "{err}");

    let mut revoked = pkg.trusting();
    revoked.revoke(&pkg.key.key_id, 1).unwrap();
    let err = refused(pkg.start(revoked).await).await;
    assert!(err.contains("signer.revoked"), "{err}");

    let mut narrow = TrustStore::empty();
    narrow
        .add(
            &pkg.key.public_key,
            "pump vendor",
            vec!["vendor.pump.*".into()],
            0,
        )
        .unwrap();
    let err = refused(pkg.start(narrow).await).await;
    assert!(err.contains("signer.not_allowed"), "{err}");
}

/// Naming a trusted key's id in a manifest signed by another key proves nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_forged_key_id_does_not_borrow_trust() {
    let trusted = trust::generate_keypair().unwrap();
    let forger = trust::generate_keypair().unwrap();
    let pkg = Package::signed("normal", json!({}));
    pkg.sign_with(&forger.seed, &[SCRIPT]);
    let mut m = pkg.json();
    m["signature"]["key_id"] = json!(trusted.key_id);
    pkg.write_manifest(&m);
    let mut store = TrustStore::empty();
    store
        .add(&trusted.public_key, "the real signer", vec![], 0)
        .unwrap();
    let err = refused(pkg.start(store).await).await;
    assert!(err.contains("manifest.signature.invalid"), "{err}");
}

/// A valid signature verifies who and what, not safety: the manifest's own limits are
/// still enforced, so an adapter that exposes more than it declared is still refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_valid_signature_does_not_override_the_manifests_permissions() {
    let pkg = Package::signed(
        "normal",
        json!({"capabilities": {"allow": [{"name": "level.*", "operations": ["read"]}]}}),
    );
    let err = refused(pkg.start(pkg.trusting()).await).await;
    assert!(err.contains("manifest violation"), "{err}");
    assert!(err.contains("dose.run"), "{err}");
    assert!(
        !err.contains("manifest.signature"),
        "the signature itself was fine: {err}"
    );
}

/// What runs is what was verified. Replacing the original afterwards changes nothing,
/// not even when the process crashes and is restarted.
#[tokio::test(flavor = "multi_thread")]
async fn the_adapter_runs_from_the_verified_copy() {
    let pkg = Package::signed("crash_on_read", json!({}));
    let adapter = pkg.start(pkg.trusting()).await.unwrap();
    // Swap the original for something hostile.
    std::fs::write(pkg.dir.path().join(SCRIPT), "import sys; sys.exit(9)\n").unwrap();
    // The read crashes the adapter; the host restarts it from its verified copy.
    let _ = adapter.read("level.measured").await;
    let status = adapter
        .wait_for_status(
            Duration::from_secs(15),
            |s| matches!(s, Status::Running { generation } if *generation >= 2),
        )
        .await;
    assert!(
        matches!(status, Status::Running { generation } if generation >= 2),
        "restarted from the verified copy, got {status:?}"
    );
    adapter.shutdown().await.unwrap();
}
