# Transport security

Decision record: [ADR-0016](../adr/0016-transport-security.md). Code:
`adapters/openmhs-profile/src/tls.rs` (MQTT, HTTP), `adapters/python/opcua` (OPC UA).

How OpenMHS protects the link between the gateway and a networked device, and what you
configure in a device profile.

## The rules

1. **Certificates are always verified.** Every TLS connection checks the server's chain
   to a trusted root, its validity period, and that it was issued for the expected name.
   There is no setting that turns this off, per profile or globally. A profile with an
   unknown key such as `"insecure": true` is rejected when it is loaded.
2. **Plaintext needs a written decision.** Unencrypted MQTT or `http://` is accepted
   without comment only to a loopback address (a simulator, a broker on the same
   machine). To any other host it is refused unless the profile sets
   `"allow_plaintext": true`. That refusal happens before anything is sent: reads fail and
   commands end `FAILED` with `adapter.not_sent`, and the message names
   `allow_plaintext`.
3. **Failures say why.** Errors carry the certificate problem as rustls reports it, e.g.
   `invalid peer certificate: UnknownIssuer`,
   `invalid peer certificate: certificate expired: verification time …, but certificate is not valid after …`,
   `certificate not valid for name "127.0.0.1"; certificate is only valid for DnsName("other.example")`.

**This changes M6 behaviour:** a profile that reached a LAN device over plain MQTT or
HTTP now needs `"allow_plaintext": true`, or better, TLS.

## Configuration (MQTT and HTTP profiles)

TLS settings live in `connection.tls`. Relative file names are relative to the
profile's directory.

```json
"connection": {
  "host": "broker.lab.example", "port": 8883,
  "tls": {
    "ca_file": "pki/lab-ca.pem",
    "client_cert_file": "pki/gateway.pem",
    "client_key_file": "pki/gateway.key",
    "server_name": "broker.lab.example"
  }
}
```

| Key | Meaning |
|---|---|
| (no `tls`) | MQTT: plaintext. HTTP: `https://` URLs use the system trust store; `http://` is plaintext. |
| `tls: {}` | MQTT: TLS, trusting the operating system's store. |
| `ca_file` | PEM file of CA certificates to trust **instead of** the system store: a lab's own CA, or one device's self-signed certificate. |
| `client_cert_file`, `client_key_file` | PEM certificate chain and key for mutual TLS. Both or neither. |
| `server_name` | The name the server's certificate must be issued for, when the profile reaches it by another name or by IP address. The certificate is still fully verified, against this name. |

- **MQTT:** `tls` switches the connection to TLS (usual port 8883).
- **HTTP:** use an `https://` `base_url`. `tls` with an `http://` URL is an error.
  Redirects are never followed.
- **Timeouts:** an HTTP request, including connecting and the TLS handshake, is bounded
  by `connection.timeout_ms`. An MQTT call waits at most three times `reply_timeout_ms`
  (at least 3 s in total) for a connected session; a failed connection is retried in the
  background, and each failure's reason is what the next call reports.
- **Protocols:** TLS 1.2 and 1.3 (rustls, ring provider). No SSLv3, TLS 1.0 or 1.1.

### Local development without disabling verification

Make a CA for your bench, issue the device a certificate from it, and point `ca_file` at
the CA, so verification stays on and the configuration is the one you will use in
production. To reach a device by IP while its certificate names a host, set
`server_name`. Plain MQTT or HTTP to `localhost` needs no setting.

## OPC UA

OPC UA security is configured in the adapter's manifest (`config.security`); see
[docs/adapters/opcua.md](../adapters/opcua.md). The same rules hold: the server
certificate is always checked (pinned, or issued by a trusted CA, within its validity
period, optionally for an expected application URI), and `policy: "None"` to a
non-loopback server needs `allow_plaintext`.

## ROS 2

DDS traffic is outside these rules: the ROS 2 adapter joins the robot's DDS domain as
configured in its container ([docs/adapters/ros2.md](../adapters/ros2.md)). Use SROS 2
on networks you do not control.

## Tests

Certificates are generated for each test run; none are committed:

- `adapters/openmhs-http/tests/tls.rs` and `adapters/openmhs-mqtt/tests/tls.rs`: a
  trusted certificate connects; an unknown CA, the system store against a private CA, an
  expired certificate and a certificate for another name are refused with the reason;
  `server_name` verifies against the configured name; a server requiring a client
  certificate refuses a client without one and accepts one with it; a failed handshake
  means the command was not sent; plaintext beyond loopback needs `allow_plaintext`;
  `insecure` is not a setting.
- `adapters/openmhs-profile` (`tls` unit tests, `test-pki` feature for the generator).
- `adapters/python/opcua/test_opcua.py`: the OPC UA equivalents.

## Not covered yet

Certificate revocation (CRL, OCSP), certificate pinning by public key for MQTT/HTTP
(use a one-certificate `ca_file`), and hardware-backed keys. The gateway's own local API
listens on loopback over plain HTTP by design ([spec/api.md](../../spec/api.md));
`--allow-remote` should be put behind a TLS-terminating proxy.
