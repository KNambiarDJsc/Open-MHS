# HTTP profiles

`"protocol": "http"`. A **declarative mapping** onto a device's JSON API: which URL and
JSON pointer hold each property, and which request with which body sets it or runs an
action. The shared format is described in [profiles.md](profiles.md); the example is
[led-controller.json](../../adapters/openmhs-http/profiles/led-controller.json). The
matching board firmware is in [docs/hardware/](../hardware/esp32-led.md).

## Connection

```json
"connection": {
  "base_url": "http://192.168.1.60",
  "allow_plaintext": true,
  "timeout_ms": 1000,
  "headers": { "Authorization": "Bearer ${env:LED_TOKEN}" },
  "response": { "ok_pointer": "/ok", "error_pointer": "/error" }
}
```

| Field | Default | |
|---|---|---|
| `base_url` | required | where the API is |
| `tls` | system trust store | for `https://`: `ca_file`, `client_cert_file`, `client_key_file`, `server_name` ([transport security](../security/transport-security.md)). Certificates are always verified |
| `allow_plaintext` | false | accept `http://` to a host that is not loopback |
| `timeout_ms` | 1000 | how long to wait for any one response |
| `headers` | none | sent with every request. `${env:NAME}` is replaced by that environment variable, so secrets stay out of the profile |
| `response` | none | how the device says whether a command succeeded: `ok_pointer` to a boolean, optionally `error_pointer` to the reason |
| `health` | first readable property | the property read to check the connection |

Without `response`, any 2xx answer counts as success, and an unreadable 2xx answer cannot
be told apart from success. Set it if the device can say `{"ok": false}`.

## Mapping

```json
"led.brightness": {
  "read":  { "path": "/api/state", "pointer": "/led/brightness" },
  "write": { "method": "PUT", "path": "/api/led", "body": { "brightness": "{{value}}" } }
},
"sensor.temperature": {
  "read": { "path": "/api/state", "pointer": "/sensor/temperature", "timestamp_pointer": "/time" }
},
"led.blink": {
  "invoke": { "method": "POST", "path": "/api/blink", "body": { "count": "{{count}}", "id": "{{command_id}}" } }
}
```

| Field | |
|---|---|
| `read` | `GET path`, value at `pointer`. `timestamp_pointer` says when it was measured: RFC 3339 text or Unix milliseconds. Without it, the arrival time is used |
| `write` | `method` (`POST`, `PUT`, `PATCH`), `path`, JSON `body` using `{{value}}` and optionally `{{command_id}}` |
| `invoke` | the same, using the action's parameters |

Several capabilities may share a path. The device tells them apart by which fields the
body has.

## What each answer means

Each command is one request, sent once and never redirected (a redirect would send it
again, elsewhere).

| The device… | Gateway outcome |
|---|---|
| answers 2xx with `ok: true` (or 2xx, with no `response` rule) | `APPLIED`, then verified by readback if declared |
| answers 2xx with `ok: false`, or 4xx, 501 or 503 | `REJECTED` with the reason |
| cannot be reached (connection refused) | not sent |
| answers 408 or 504, or does not answer in time | `UNKNOWN_OUTCOME` (timeout) |
| answers another 5xx, a 2xx without a readable `ok`, or drops the connection | `UNKNOWN_OUTCOME`; reconciled by readback where declared |

## Limits

- JSON only.
- Each read is its own GET. Properties on the same path are not fetched together.
- `https://` verifies the device's certificate (system trust store, or `connection.tls`).
  Plain `http://` is accepted to loopback only, unless `allow_plaintext` is set
  ([transport security](../security/transport-security.md)). Devices such as an ESP32 on a
  home LAN usually serve plain HTTP: set `"allow_plaintext": true` for them, knowing
  commands and any `headers` credentials cross the network unencrypted.
- No polling or push: values are read when someone asks.
