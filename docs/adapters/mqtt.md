# MQTT profiles

`"protocol": "mqtt"`. For devices that publish their state as JSON through an MQTT
broker and take commands on topics. The shared format is described in
[profiles.md](profiles.md); the example is
[greenhouse-node.json](../../adapters/openmhs-mqtt/profiles/greenhouse-node.json).

The device must follow three conventions. This adapter cannot drive a device that does
not:

1. It publishes its state as **retained** JSON on one or more state topics, ideally with
   a timestamp of when it measured.
2. Command payloads carry an id (`{{command_id}}`).
3. It answers every command on an **acknowledgement topic** with that id and whether it
   acted.

Without acknowledgements, a command over MQTT is fire-and-forget. OpenMHS will not report
such a command as anything but uncertain.

## Connection

```json
"connection": {
  "host": "192.168.1.10",
  "port": 1883,
  "client_id": "openmhs-greenhouse-01",
  "username": "openmhs",
  "password": "${env:MQTT_PASSWORD}",
  "reply_timeout_ms": 1000,
  "ack": { "topic": "greenhouse/node-01/ack", "id_pointer": "/id", "ok_pointer": "/ok", "error_pointer": "/error" }
}
```

| Field | Default | |
|---|---|---|
| `host`, `port` | required, 1883 | the broker |
| `client_id` | required | must be unique on the broker: a second client with the same id disconnects the first |
| `username`, `password` | none | `${env:NAME}` is replaced by that environment variable |
| `keep_alive_s` | 5 | MQTT keep-alive |
| `reply_timeout_ms` | 1000 | how long to wait for the broker, and for an acknowledgement |
| `ack` | required | `topic`, and JSON pointers to the command id, to a boolean `ok`, and optionally to the reason for a refusal |

## Mapping

```json
"air.temperature": {
  "state": { "topic": "greenhouse/node-01/state", "pointer": "/air/temperature", "timestamp_pointer": "/time" }
},
"fan.on": {
  "state":   { "topic": "greenhouse/node-01/state", "pointer": "/fan", "timestamp_pointer": "/time" },
  "command": { "topic": "greenhouse/node-01/set", "payload": { "fan": "{{value}}", "id": "{{command_id}}" } }
},
"irrigation.dose": {
  "command": { "topic": "greenhouse/node-01/dose", "payload": { "ml": "{{volume}}", "id": "{{command_id}}" } }
}
```

| Field | |
|---|---|
| `state` | readable properties: `topic` (no wildcards), `pointer` to the value, optional `timestamp_pointer` (RFC 3339 text or Unix milliseconds) |
| `command` | writable properties and actions: `topic` and a JSON `payload` template. It must use `{{command_id}}`, plus `{{value}}` for writes or the action's parameters |

Several capabilities may share a state topic (one JSON document) or a command topic (the
device tells them apart by which fields the payload has). The acknowledgement topic must
not also be a state topic.

## How it behaves

**Reads.** The adapter subscribes to every state topic and keeps the newest message on
each. A read first confirms that the broker still answers, with a SUBSCRIBE round trip
that also makes the broker send the retained state again. It then returns the value with
the device's timestamp, or the arrival time when the profile has no
`timestamp_pointer`. A cached value is never returned over a dead connection: the read
fails with `adapter.disconnected`.

**Freshness.** A reading's age is the age of the device's own timestamp. A device that
stops publishing keeps its last retained message on the broker. The descriptor's
`freshness.max_age_ms` makes the gateway flag such a value as stale. Set it for every
measured property, and have the device stamp its messages; the time of arrival says
nothing about when a retained message was measured.

**Commands** are published once at QoS 0 and only while connected. The adapter keeps no
queue, and no session survives a reconnect (`clean_session`). Each connection uses a
fresh client, so nothing published during an earlier connection can be delivered by a
later one. One command is in flight at a time.

| What happens | Gateway outcome |
|---|---|
| an acknowledgement with the command's id and `ok: true` | `APPLIED`, then verified by readback if declared |
| an acknowledgement with `ok: false` | `REJECTED` with the reason |
| not connected to the broker | not sent |
| no acknowledgement within `reply_timeout_ms` | `UNKNOWN_OUTCOME` (timeout); reconciled by readback where declared |
| the connection drops before the acknowledgement | `UNKNOWN_OUTCOME` |
| something unreadable on the acknowledgement topic | `UNKNOWN_OUTCOME` (malformed answer) |

Acknowledgements with another id, such as a late answer to an earlier command, are
ignored.

## The simulator

The simulator is a small MQTT 3.1.1 broker written for the purpose. It supports QoS 0
and 1, retained messages, and `+` and `#` filters, and has the device built in. The
device publishes its state topics every 250 ms and after each command. The simulator is
for tests, not a broker to deploy.

## Limits

- MQTT 3.1.1 over plain TCP. TLS, websockets and MQTT 5 are not supported yet; keep the
  broker on a trusted network, or put it behind a local TLS-terminating bridge.
- No support for the device's last-will (online/offline) topic yet. A device that dies
  shows up as stale readings and commands that are not acknowledged.
- Payloads are JSON.
