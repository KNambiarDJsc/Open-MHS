//! Transport security for networked profiles (docs/security/transport-security.md).
//!
//! Every TLS connection verifies the server's certificate: its chain to a trusted root,
//! its validity period, and that it was issued for the expected name. There is no
//! setting that turns verification off. Trust comes from the operating system's store
//! or, with `ca_file`, from a CA the deployment chooses (a lab's own CA, or the
//! self-signed certificate of one device).
//!
//! Plaintext is allowed to loopback addresses (simulators, a broker on the same
//! machine). Anything else needs `allow_plaintext: true` in the profile, so sending
//! commands unencrypted across a network is always a decision someone wrote down.

use std::path::Path;
use std::sync::Arc;

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use serde::Deserialize;

/// TLS settings of a profile's connection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tls {
    /// PEM file of CA certificates to trust instead of the system's trust store.
    #[serde(default)]
    pub ca_file: Option<String>,
    /// PEM certificate chain presented to the server (mutual TLS). Needs `client_key_file`.
    #[serde(default)]
    pub client_cert_file: Option<String>,
    /// PEM private key for `client_cert_file`.
    #[serde(default)]
    pub client_key_file: Option<String>,
    /// The name the server's certificate must be issued for, when the profile reaches
    /// the server by another name or by IP address. Verification is otherwise unchanged.
    #[serde(default)]
    pub server_name: Option<String>,
}

/// The crypto provider OpenMHS uses for every TLS connection.
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn read_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    let certs = CertificateDer::pem_file_iter(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if certs.is_empty() {
        return Err(format!(
            "{}: no PEM certificates in the file",
            path.display()
        ));
    }
    Ok(certs)
}

fn system_roots() -> Result<RootCertStore, String> {
    let found = rustls_native_certs::load_native_certs();
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(found.certs);
    if roots.is_empty() {
        let why: Vec<String> = found.errors.iter().map(ToString::to_string).collect();
        return Err(format!(
            "no trusted certificates found in the system store ({}); set tls.ca_file",
            why.join("; ")
        ));
    }
    Ok(roots)
}

/// Verifies like rustls' WebPKI verifier, but against a configured name rather than
/// the one the connection was opened with.
#[derive(Debug)]
struct ExpectedName {
    inner: Arc<WebPkiServerVerifier>,
    name: ServerName<'static>,
}

impl ServerCertVerifier for ExpectedName {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.inner
            .verify_server_cert(end_entity, intermediates, &self.name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

impl Tls {
    /// The rustls client configuration. Relative file names are relative to `dir`, the
    /// profile's directory.
    pub fn client_config(&self, dir: &Path) -> Result<Arc<ClientConfig>, String> {
        let provider = provider();
        let roots = match &self.ca_file {
            Some(f) => {
                let path = dir.join(f);
                let mut roots = RootCertStore::empty();
                for cert in read_certs(&path)? {
                    roots.add(cert).map_err(|e| {
                        format!("{}: not a usable CA certificate: {e}", path.display())
                    })?;
                }
                roots
            }
            None => system_roots()?,
        };
        let verifier =
            WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
                .build()
                .map_err(|e| format!("tls: {e}"))?;
        let builder = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| format!("tls: {e}"))?;
        let builder = match &self.server_name {
            Some(name) => {
                let name = ServerName::try_from(name.clone())
                    .map_err(|e| format!("tls.server_name {name:?}: {e}"))?;
                // Not a weaker verifier: the WebPKI one, given the configured name.
                builder
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(ExpectedName {
                        inner: verifier,
                        name,
                    }))
            }
            None => builder.with_webpki_verifier(verifier),
        };
        let config = match (&self.client_cert_file, &self.client_key_file) {
            (Some(cert), Some(key)) => {
                let key_path = dir.join(key);
                let key = PrivateKeyDer::from_pem_file(&key_path)
                    .map_err(|e| format!("{}: {e}", key_path.display()))?;
                builder
                    .with_client_auth_cert(read_certs(&dir.join(cert))?, key)
                    .map_err(|e| format!("tls client certificate: {e}"))?
            }
            (None, None) => builder.with_no_client_auth(),
            _ => return Err("tls: client_cert_file and client_key_file go together".into()),
        };
        Ok(Arc::new(config))
    }
}

/// A client configuration that accepts no server at all, for clients that must never
/// use TLS.
pub fn refuse_all() -> Arc<ClientConfig> {
    // The ring provider supports the safe default protocol versions; this cannot fail.
    #[allow(clippy::expect_used)]
    let builder = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions");
    Arc::new(
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(RejectAll))
            .with_no_client_auth(),
    )
}

/// Install ring as the process-wide rustls provider, if no provider is installed yet.
/// reqwest needs one to build any client (even one that never uses TLS).
pub fn install_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[derive(Debug)]
struct RejectAll;

impl ServerCertVerifier for RejectAll {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Err(rustls::Error::General(
            "this client does not use TLS".into(),
        ))
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General(
            "this client does not use TLS".into(),
        ))
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General(
            "this client does not use TLS".into(),
        ))
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Whether `host` is `localhost` or a loopback address.
pub fn is_loopback(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h.eq_ignore_ascii_case("localhost")
        || h.parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Refuse an unencrypted connection to anything but loopback, unless the profile says
/// `allow_plaintext: true`.
pub fn check_plaintext(host: &str, allow_plaintext: bool, what: &str) -> Result<(), String> {
    if allow_plaintext || is_loopback(host) {
        Ok(())
    } else {
        Err(format!(
            "refusing unencrypted {what} to {host}: anyone on the network could read and alter commands. \
             Configure TLS, or set connection.allow_plaintext to true to accept that risk"
        ))
    }
}

/// An error and all its causes, for messages that must say why a TLS handshake failed.
pub fn chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = s.source();
    }
    out
}

/// Certificates made on the fly for TLS tests: a CA, server certificates that are good,
/// expired, or for another name, a client certificate, and an unrelated CA.
#[cfg(feature = "test-pki")]
pub mod test_pki {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use rcgen::{
        BasicConstraints, CertificateParams, CertifiedIssuer, DnType, IsCa, KeyPair,
        KeyUsagePurpose, date_time_ymd,
    };
    use rustls::ServerConfig;
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    use rustls::server::WebPkiClientVerifier;

    pub struct Pki {
        pub dir: PathBuf,
    }

    /// Server certificates the PKI holds, by file stem.
    pub const GOOD: &str = "server";
    pub const EXPIRED: &str = "expired";
    pub const WRONG_NAME: &str = "wrong-name";
    pub const OTHER_CA: &str = "other-server";

    fn ca(name: &str) -> CertifiedIssuer<'static, KeyPair> {
        let mut p = CertificateParams::new(Vec::<String>::new()).expect("params");
        p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        p.distinguished_name.push(DnType::CommonName, name);
        p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        CertifiedIssuer::self_signed(p, KeyPair::generate().expect("key")).expect("ca")
    }

    fn leaf(
        dir: &Path,
        stem: &str,
        names: &[&str],
        issuer: &CertifiedIssuer<'static, KeyPair>,
        expired: bool,
    ) {
        let mut p = CertificateParams::new(names.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            .expect("params");
        p.distinguished_name.push(DnType::CommonName, names[0]);
        if expired {
            p.not_before = date_time_ymd(2020, 1, 1);
            p.not_after = date_time_ymd(2021, 1, 1);
        }
        let key = KeyPair::generate().expect("key");
        let cert = p.signed_by(&key, issuer).expect("sign");
        std::fs::write(dir.join(format!("{stem}.pem")), cert.pem()).expect("write cert");
        std::fs::write(dir.join(format!("{stem}.key")), key.serialize_pem()).expect("write key");
    }

    /// Write the test PKI into `dir`: `ca.pem`, `other-ca.pem`, and `<stem>.pem`/`.key`
    /// for `server` (localhost, 127.0.0.1), `expired`, `wrong-name` (other.example),
    /// `other-server` (signed by the other CA) and `client`.
    pub fn create(dir: &Path) -> Pki {
        let ca_ = ca("OpenMHS test CA");
        let other = ca("Unrelated CA");
        std::fs::write(dir.join("ca.pem"), ca_.pem()).expect("write ca");
        std::fs::write(dir.join("other-ca.pem"), other.pem()).expect("write other ca");
        let local = ["localhost", "127.0.0.1"];
        leaf(dir, GOOD, &local, &ca_, false);
        leaf(dir, EXPIRED, &local, &ca_, true);
        leaf(dir, WRONG_NAME, &["other.example"], &ca_, false);
        leaf(dir, OTHER_CA, &local, &other, false);
        leaf(dir, "client", &["client.test"], &ca_, false);
        Pki {
            dir: dir.to_path_buf(),
        }
    }

    impl Pki {
        pub fn path(&self, file: &str) -> String {
            self.dir.join(file).display().to_string()
        }

        /// A server configuration presenting `<stem>.pem`. With `require_client`, only
        /// clients with a certificate from the test CA get in.
        pub fn server_config(&self, stem: &str, require_client: bool) -> Arc<ServerConfig> {
            let certs = CertificateDer::pem_file_iter(self.dir.join(format!("{stem}.pem")))
                .expect("cert file")
                .collect::<Result<Vec<_>, _>>()
                .expect("certs");
            let key =
                PrivateKeyDer::from_pem_file(self.dir.join(format!("{stem}.key"))).expect("key");
            let builder = ServerConfig::builder_with_provider(super::provider())
                .with_safe_default_protocol_versions()
                .expect("versions");
            let builder = if require_client {
                let mut roots = rustls::RootCertStore::empty();
                for c in CertificateDer::pem_file_iter(self.dir.join("ca.pem")).expect("ca") {
                    roots.add(c.expect("ca cert")).expect("add ca");
                }
                let verifier =
                    WebPkiClientVerifier::builder_with_provider(Arc::new(roots), super::provider())
                        .build()
                        .expect("client verifier");
                builder.with_client_cert_verifier(verifier)
            } else {
                builder.with_no_client_auth()
            };
            Arc::new(builder.with_single_cert(certs, key).expect("server config"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_is_recognised() {
        for h in [
            "localhost",
            "LOCALHOST",
            "127.0.0.1",
            "127.8.9.10",
            "::1",
            "[::1]",
        ] {
            assert!(is_loopback(h), "{h}");
        }
        for h in [
            "192.168.1.60",
            "10.0.0.1",
            "broker.lab",
            "localhost.evil.example",
            "::2",
        ] {
            assert!(!is_loopback(h), "{h}");
        }
    }

    #[test]
    fn plaintext_to_the_network_needs_an_explicit_choice() {
        assert!(check_plaintext("127.0.0.1", false, "MQTT").is_ok());
        let err = check_plaintext("192.168.1.60", false, "HTTP").unwrap_err();
        assert!(err.contains("allow_plaintext"), "{err}");
        assert!(check_plaintext("192.168.1.60", true, "HTTP").is_ok());
    }

    #[test]
    fn there_is_no_way_to_turn_verification_off() {
        for field in [
            "insecure",
            "verify",
            "skip_verify",
            "danger_accept_invalid_certs",
        ] {
            let doc = format!(r#"{{"{field}": true}}"#);
            assert!(
                serde_json::from_str::<Tls>(&doc).is_err(),
                "{field} accepted"
            );
        }
    }

    #[test]
    fn bad_files_are_named() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("empty.pem"), "").unwrap();
        let t = Tls {
            ca_file: Some("empty.pem".into()),
            ..Default::default()
        };
        let err = t.client_config(dir.path()).unwrap_err();
        assert!(
            err.contains("empty.pem") && err.contains("no PEM certificates"),
            "{err}"
        );
        let t = Tls {
            ca_file: Some("missing.pem".into()),
            ..Default::default()
        };
        assert!(
            t.client_config(dir.path())
                .unwrap_err()
                .contains("missing.pem")
        );
        let t = Tls {
            client_cert_file: Some("c.pem".into()),
            ..Default::default()
        };
        assert!(t.client_config(dir.path()).is_err());
    }

    #[test]
    fn a_ca_file_and_client_certificate_load() {
        let dir = tempfile::tempdir().unwrap();
        let ca = rcgen::generate_simple_self_signed(vec!["ca.test".into()]).unwrap();
        std::fs::write(dir.path().join("ca.pem"), ca.cert.pem()).unwrap();
        std::fs::write(dir.path().join("c.key"), ca.signing_key.serialize_pem()).unwrap();
        let t = Tls {
            ca_file: Some("ca.pem".into()),
            client_cert_file: Some("ca.pem".into()),
            client_key_file: Some("c.key".into()),
            server_name: Some("device.lab".into()),
        };
        t.client_config(dir.path()).unwrap();
    }
}
