//! MQTT over TLS: the broker's certificate is verified, and failures say why
//! (docs/security/transport-security.md). Certificates are made for each run.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use openmhs_adapter::{AdapterErrorKind, HardwareAdapter};
use openmhs_mqtt::sim::MqttSim;
use openmhs_mqtt::{MqttAdapter, load};
use openmhs_profile::tls::test_pki::{self, EXPIRED, GOOD, OTHER_CA, Pki, WRONG_NAME};
use openmhs_types::{Clock, SystemClock};
use serde_json::{Value as Json, json};
use tokio::task::JoinHandle;

const READABLE: &str = "air.temperature";

fn profile(connection: Json) -> Json {
    let mut p: Json =
        serde_json::from_str(include_str!("../profiles/greenhouse-node.json")).unwrap();
    for (k, v) in connection.as_object().unwrap() {
        p["connection"][k] = v.clone();
    }
    p
}

/// A broker serving MQTT over TLS with `<stem>.pem`.
async fn serve(pki: &Pki, stem: &str, require_client: bool) -> (u16, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let sim = MqttSim::new(load(&profile(json!({})).to_string()).unwrap());
    let task = tokio::spawn(sim.serve_tls(listener, pki.server_config(stem, require_client)));
    (port, task)
}

fn adapter(pki: &Pki, port: u16, tls: Json) -> MqttAdapter {
    let doc = profile(json!({"port": port, "tls": tls}));
    let p = load(&doc.to_string()).unwrap().in_dir(&pki.dir);
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    MqttAdapter::new(p, clock).unwrap()
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
    let a = adapter(&pki, port, trust_test_ca());
    a.connect().await.unwrap();
    a.read(READABLE).await.unwrap();
    task.abort();
}

async fn refused(stem: &str, tls: Json, expect: &str) {
    let (_d, pki) = pki();
    let (port, task) = serve(&pki, stem, false).await;
    let err = adapter(&pki, port, tls).connect().await.unwrap_err();
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
    adapter(&pki, port, tls).connect().await.unwrap();
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broker_that_wants_a_client_certificate_gets_one() {
    let (_d, pki) = pki();
    let (port, task) = serve(&pki, GOOD, true).await;
    let err = adapter(&pki, port, trust_test_ca())
        .connect()
        .await
        .unwrap_err();
    assert_eq!(err.kind, AdapterErrorKind::Disconnected, "{err}");
    let tls = json!({"ca_file": "ca.pem", "client_cert_file": "client.pem", "client_key_file": "client.key"});
    adapter(&pki, port, tls).connect().await.unwrap();
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn plaintext_beyond_loopback_needs_allow_plaintext() {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    // 192.0.2.0/24 is reserved for documentation: nothing is ever contacted.
    let doc = profile(json!({"host": "192.0.2.10"}));
    let a = MqttAdapter::new(load(&doc.to_string()).unwrap(), clock).unwrap();
    for err in [
        a.connect().await.unwrap_err(),
        a.read(READABLE).await.unwrap_err(),
    ] {
        assert_eq!(err.kind, AdapterErrorKind::NotSent, "{err}");
        assert!(err.message.contains("allow_plaintext"), "{err}");
    }

    // Pointing it at a broker on loopback is fine.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(MqttSim::new(load(&doc.to_string()).unwrap()).serve(listener));
    let a = a.at("127.0.0.1", port);
    a.connect().await.unwrap();
    task.abort();
}

#[test]
fn verification_cannot_be_disabled() {
    let doc = profile(json!({"tls": {"insecure": true}}));
    assert!(load(&doc.to_string()).is_err());
}
