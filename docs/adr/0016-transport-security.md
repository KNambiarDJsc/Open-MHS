# ADR-0016: Transport security for networked adapters

- Status: Accepted
- Date: 2026-10-24
- Builds on: [ADR-0014](0014-protocol-adapters.md)

## Context

M6 shipped MQTT and HTTP without TLS. Commands to real equipment crossed the network in
the clear, along with any credentials in the profile. Production use needs verified TLS,
and development needs a way to run locally that does not teach people to turn
verification off.

## Decision

- **rustls with the ring provider** for MQTT (`rumqttc`'s preconfigured rustls
  transport) and HTTP (`reqwest` with a preconfigured client config). One shared
  configuration builder (`openmhs-profile::tls`) so both protocols behave the same.
- **Verification cannot be disabled.** The only trust inputs are the system store or a
  `ca_file`. `server_name` changes the expected name, not the checks (it wraps rustls'
  own verifier). `tls` accepts no unknown keys, so `insecure` fails at load.
- **Plaintext is refused beyond loopback** unless the profile says
  `allow_plaintext: true`, before anything is sent (`adapter.not_sent`). Local
  development uses loopback, or a bench CA via `ca_file`.
- **Errors carry the certificate reason.** Test certificates are generated per run
  (`rcgen`, behind the `test-pki` feature); none are committed.
- OPC UA applies the same rules in the adapter: the adapter checks the server
  certificate itself and pins it for the secure channel; the unsecured policy is accepted
  only to loopback or with `allow_plaintext`.
- No global switch, environment variable or CLI flag weakens any of this.

## Alternatives considered

- **native-tls / OpenSSL.** Rejected: platform differences, and a C dependency.
- **A `--insecure` flag for development.** Rejected: it ships to production. A bench CA
  is little more work.
- **Plaintext default with a warning.** Rejected: the breaking change to M6 profiles is
  small and the failure message says exactly what to set.

## Consequences

- **Breaking:** M6 profiles that used plain MQTT/HTTP on a LAN now need
  `allow_plaintext: true`, or TLS.
- rumqttc 0.25.1 links a vulnerable webpki 0.102 and rustls-pemfile, reachable only via
  `TlsConfiguration::Simple`, which is not used. `deny.toml` ignores those advisories
  with a comment saying why. If that is not acceptable, the alternatives are dropping
  MQTT TLS or switching MQTT client.
- Not covered: certificate revocation (CRL/OCSP), DDS security, TLS on the gateway's own
  API.
