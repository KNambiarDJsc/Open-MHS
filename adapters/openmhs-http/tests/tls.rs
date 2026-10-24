//! HTTPS: the device's certificate is verified, and failures say why
//! (docs/security/transport-security.md). Certificates are made for each run.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::Arc;

use openmhs_adapter::{AdapterCommand, AdapterErrorKind, HardwareAdapter, Progress};
use openmhs_http::sim::HttpSim;
use openmhs_http::{HttpAdapter, load};
use openmhs_profile::tls::test_pki::{self, EXPIRED, GOOD, OTHER_CA, Pki, WRONG_NAME};
use openmhs_types::descriptor::Operation;
use openmhs_types::{Clock, SystemClock, Value};
use serde_json::{Value as Json, json};
use tokio::task::JoinHandle;

const READABLE: &str = "sensor.temperature";

fn profile(connection: Json) -> Json {
    let mut p: Json =
        serde_json::from_str(include_str!("../profiles/led-controller.json")).unwrap();
    for (k, v) in connection.as_object().unwrap() {
        p["connection"][k] = v.clone();
    }
    p
}

/// A simulator serving HTTPS with `<stem>.pem`.
async fn serve(pki: &Pki, stem: &str, require_client: bool) -> (u16, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let doc = profile(json!({}));
    let sim = HttpSim::new(load(&doc.to_string()).unwrap());
    let task = tokio::spawn(sim.serve_tls(listener, pki.server_config(stem, require_client)));
    (port, task)
}

fn adapter(pki: &Pki, port: u16, tls: Json) -> HttpAdapter {
    let doc = profile(json!({"base_url": format!("https://127.0.0.1:{port}"), "tls": tls}));
    let p = load(&doc.to_string()).unwrap().in_dir(&pki.dir);
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    HttpAdapter::new(p, clock).unwrap()
}

fn trust_test_ca() -> Json {
    json!({"ca_file": "ca.pem"})
}

fn pki() -> (tempfile::TempDir, Pki) {
    let dir = tempfile::tempdir().unwrap();
    let pki = test_pki::create(dir.path());
    (dir, pki)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_trusted_certificate_connects() {
    let (_d, pki) = pki();
    let (port, task) = serve(&pki, GOOD, false).await;
    adapter(&pki, port, trust_test_ca())
        .read(READABLE)
        .await
        .unwrap();
    task.abort();
}

async fn refused(stem: &str, tls: Json, expect: &str) {
    let (_d, pki) = pki();
    let (port, task) = serve(&pki, stem, false).await;
    let err = adapter(&pki, port, tls).read(READABLE).await.unwrap_err();
    assert_eq!(err.kind, AdapterErrorKind::Disconnected, "{err}");
    assert!(
        err.message.contains(expect),
        "expected {expect:?} in: {err}"
    );
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_certificate_from_an_untrusted_ca_is_refused() {
    refused(OTHER_CA, trust_test_ca(), "UnknownIssuer").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_system_store_does_not_trust_a_private_ca() {
    refused(GOOD, json!({}), "UnknownIssuer").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_certificate_is_refused() {
    refused(EXPIRED, trust_test_ca(), "certificate expired").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_certificate_for_another_name_is_refused() {
    refused(WRONG_NAME, trust_test_ca(), "not valid for name").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn server_name_checks_the_certificate_against_the_configured_name() {
    let (_d, pki) = pki();
    let (port, task) = serve(&pki, WRONG_NAME, false).await;
    let tls = json!({"ca_file": "ca.pem", "server_name": "other.example"});
    adapter(&pki, port, tls).read(READABLE).await.unwrap();
    // The configured name must still match: a certificate for 127.0.0.1 is not one for
    // other.example.
    let (port, task2) = serve(&pki, GOOD, false).await;
    let tls = json!({"ca_file": "ca.pem", "server_name": "other.example"});
    let err = adapter(&pki, port, tls).read(READABLE).await.unwrap_err();
    assert!(err.message.contains("not valid for name"), "{err}");
    task.abort();
    task2.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_wants_a_client_certificate_gets_one() {
    let (_d, pki) = pki();
    let (port, task) = serve(&pki, GOOD, true).await;
    let err = adapter(&pki, port, trust_test_ca())
        .read(READABLE)
        .await
        .unwrap_err();
    assert_eq!(err.kind, AdapterErrorKind::Disconnected, "{err}");
    let tls = json!({"ca_file": "ca.pem", "client_cert_file": "client.pem", "client_key_file": "client.key"});
    adapter(&pki, port, tls).read(READABLE).await.unwrap();
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_handshake_means_the_command_was_not_sent() {
    let (_d, pki) = pki();
    let (port, task) = serve(&pki, OTHER_CA, false).await;
    let a = adapter(&pki, port, trust_test_ca());
    let command = AdapterCommand {
        command_id: "c-1".into(),
        capability: "led.on".into(),
        operation: Operation::Write,
        value: Some(Value::Bool(true)),
        params: BTreeMap::new(),
        idempotent: true,
        deadline: SystemClock.now().plus_ms(5_000),
    };
    let (progress, _rx) = Progress::channel();
    let err = a.execute(command, progress).await.unwrap_err();
    assert_eq!(err.kind, AdapterErrorKind::NotSent, "{err}");
    assert!(err.message.contains("UnknownIssuer"), "{err}");
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn plaintext_beyond_loopback_needs_allow_plaintext() {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    // 192.0.2.0/24 is reserved for documentation: nothing is ever contacted.
    let doc = profile(json!({"base_url": "http://192.0.2.10"}));
    let a = HttpAdapter::new(load(&doc.to_string()).unwrap(), clock.clone()).unwrap();
    let err = a.read(READABLE).await.unwrap_err();
    assert_eq!(err.kind, AdapterErrorKind::NotSent);
    assert!(err.message.contains("allow_plaintext"), "{err}");

    // Pointing it at a simulator on loopback is fine.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(HttpSim::new(load(&doc.to_string()).unwrap()).serve(listener));
    let a = a.at(format!("http://127.0.0.1:{port}"));
    a.read(READABLE).await.unwrap();
    task.abort();
}

#[test]
fn tls_settings_need_https_and_cannot_disable_verification() {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let doc = profile(json!({"base_url": "http://127.0.0.1:8080", "tls": {}}));
    let err = HttpAdapter::new(load(&doc.to_string()).unwrap(), clock)
        .err()
        .unwrap();
    assert!(err.contains("https"), "{err}");
    let doc = profile(json!({"base_url": "https://device.lab", "tls": {"insecure": true}}));
    assert!(load(&doc.to_string()).is_err());
}
