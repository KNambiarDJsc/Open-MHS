# OPC UA (read-only)

`adapters/python/opcua/opcua_adapter.py` reads variables from an OPC UA server: PLCs,
industrial controllers, lab equipment with an OPC UA interface. It is an out-of-process
adapter ([Python SDK](python-sdk.md)) built on
[asyncua](https://github.com/FreeOpcUa/opcua-asyncio).

**Read-only in this release.** The adapter reads; it does not write variables or call
methods, and it refuses a descriptor that declares `write` or `invoke`. Writing to
industrial equipment is a later milestone and will need its own review.

> **A conformance level is not a safety certification.** Point the adapter at a real
> server only with the operator's approval.

## Install

asyncua is LGPL-3.0-or-later. It is installed separately and imported at run time by
the adapter process; OpenMHS does not bundle it. Its dependencies are permissive (MIT,
BSD-3-Clause, Apache-2.0, PSF-2.0). The pinned set (each release at least two weeks old
when pinned):

```sh
python -m venv .venv-opcua
.venv-opcua/bin/pip install -r adapters/python/opcua/requirements.txt
```

Use that interpreter in the manifest's `runtime.command`.

## Try it against the simulator

```sh
openmhs conformance --adapter-manifest adapters/python/opcua/manifest.sim.json
```

`config.simulate` starts a small OPC UA server inside the adapter process, on loopback
(a climate chamber: temperature, humidity, door switch, mode). It reaches L1, including
the `link_down` fault (the simulated server stops, then starts again).

## Configure a real server

The manifest's `config`:

```json
{
  "endpoint": "opc.tcp://10.0.0.5:4840/line1",
  "security": {
    "policy": "Basic256Sha256",
    "mode": "SignAndEncrypt",
    "certificate": "pki/openmhs-client.der",
    "private_key": "pki/openmhs-client.pem",
    "trusted": ["pki/trusted"],
    "server_uri": "urn:acme:line1:plc"
  },
  "user": { "username_env": "OPCUA_USER", "password_env": "OPCUA_PASSWORD" },
  "timeout_ms": 2000,
  "descriptor": { "...": "the device descriptor" },
  "nodes": {
    "oven.temperature": "nsu=urn:acme:line1;s=Oven.Temperature",
    "oven.mode": { "node": "nsu=urn:acme:line1;s=Oven.Mode", "enum": ["idle", "heating", "fault"] }
  }
}
```

with, in the manifest, `"endpoints": {"network": ["opc.tcp://10.0.0.5:4840"]}`,
`"filesystem": {"read": ["pki"]}` and `"credentials": ["OPCUA_USER", "OPCUA_PASSWORD"]`.

- **`nodes`** maps every readable property to one variable. Name the namespace by URI
  (`nsu=`), not by index: indexes can change when the server restarts, so `ns=2;…` is
  refused (only namespace 0 may use `ns=0;` or a bare `i=…`). URIs are resolved to the
  server's current indexes on every connect. `enum` maps an integer variable to the
  descriptor's enum values by position.
- **`timeout_ms`** bounds every request; connecting gets twice that.
- **Find node ids** with the browse command. It uses the manifest's security settings
  and endpoint permissions:

  ```sh
  python adapters/python/opcua/opcua_adapter.py browse --manifest my-plc.json --depth 3
  ```

## Security

- **Policies:** `Basic256Sha256`, `Aes128_Sha256_RsaOaep`, `Aes256_Sha256_RsaPss`; mode
  `Sign` or `SignAndEncrypt` (default). The adapter needs its own application instance
  certificate and key (`certificate`, `private_key`); its subjectAltName URI should be
  `urn:openmhs:adapter:opcua`. Add that certificate to the server's trusted clients.
- **Server certificate verification is always on.** `trusted` lists certificate files or
  directories (`.der`, `.pem`, `.crt`). The server's certificate is accepted only if it is
  one of them (a pinned, often self-signed, certificate) or was issued directly by a CA
  among them, and if it is within its validity period. `server_uri`, when set, must
  appear in the certificate's subjectAltName. There is no option to skip this.
- The adapter checks the certificate **itself** before opening the secure channel, and
  then pins it, so a different certificate later in the handshake is refused. (asyncua
  alone, given no server certificate, would fetch one and trust it without checking.)
- **`policy: "None"`** (unsigned, unencrypted) is accepted only to a loopback server
  unless `allow_plaintext: true` is set. Username and password would then cross the
  network in the clear.
- Failures name the cause: `server certificate CN=… is not trusted…`,
  `…is expired or not yet valid (valid … to …)`,
  `server certificate is for [...], not urn:…`, `the server offers no Basic256Sha256/Sign
  endpoint`. They are `adapter.not_sent`: nothing was read.

Not yet supported: intermediate CA chains (only direct issuance), certificate revocation
lists, hostname checks against the certificate (OPC UA identifies servers by application
URI, which `server_uri` checks), and user certificates.

## Readings

- Each `read` is a direct Read service call; nothing is cached.
- **Timestamp:** the value's `SourceTimestamp`, else its `ServerTimestamp`, else when
  the adapter received it.
- **Quality:** StatusCode Good → `GOOD`; Uncertain → `UNCERTAIN` (the value is returned);
  Bad → an error: `adapter.disconnected` for StatusCodes that mean the server lost the
  device (`BadCommunicationError`, `BadNotConnected`, `BadNoCommunication`,
  `BadWaitingForInitialData`, …), `adapter.device_failure` for the rest. The StatusCode
  name is in the message.
- **Types:** Double/Float/integers → `number` (with the descriptor's unit) or `integer`;
  Boolean → `boolean`; String → `string` or `enum`. Anything else, NaN or infinity is
  `adapter.malformed_response`.
- **Reconnection:** a lost connection fails the call in progress; the next call connects
  again. Health reads the server's `ServerStatus.State` and is healthy only when it is
  `Running`.
- **Subscriptions:** not used. The adapter contract has no subscriptions yet
  (`L1.subscribe` is not applicable); polling `read` gives the server's current value.
