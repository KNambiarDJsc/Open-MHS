# Protocol profiles

A **profile** connects one real device to OpenMHS without writing code. It is one JSON
file that holds:

- the device descriptor, which says what the device can do;
- how to reach the device;
- how each capability maps onto the device's protocol.

Four protocols are built in:

| `protocol` | Devices | Mapping reference |
|---|---|---|
| `modbus-tcp` | PLCs, controllers, meters, drives | [modbus.md](modbus.md) |
| `mqtt` | Sensor and actuator nodes publishing JSON through a broker | [mqtt.md](mqtt.md) |
| `scpi` | Power supplies, meters, loads (TCP or serial) | [scpi.md](scpi.md) |
| `http` | Boards and appliances with a small JSON API | [http.md](http.md) |

If your device speaks something else, write an adapter instead ([README.md](README.md)).

## Using a profile

```sh
openmhs profile check my-device.json             # validate; connects to nothing
openmhs conformance --profile my-device.json     # test the adapter against the profile's simulator
openmhs serve --profile-sim my-device.json       # gateway with the simulated device
openmhs serve --profile my-device.json           # gateway with the REAL device
```

- `--profile-sim` and `conformance --profile` start the profile's **simulator** on a
  loopback port and never touch the real device.
- `serve --profile` connects to the address in the profile. It reads from the device
  right away to check the connection. It sends commands only when a principal asks and
  policy allows: the same gateway, policy, approvals, journal and audit apply as for any
  other device.

Every option can be repeated, and `--sim` (the virtual lab) can be combined with any of
them.

## Layout

```jsonc
{
  "openmhs_profile": "0.1",
  "protocol": "mqtt",
  "descriptor": { /* a device descriptor: spec/descriptor.md */ },
  "connection": { /* where the device is: per protocol */ },
  "mapping": {
    "<capability name>": { /* how it maps onto the protocol: per protocol */ }
  },
  "simulator": {
    "initial": { "<property>": { "value": 24.5, "unit": "Cel" } }
  }
}
```

The **descriptor** is the same document a hand-written adapter returns. It is validated
the same way and is what agents see. Its `adapter.conformance` field is a claim:
`openmhs conformance --profile` checks it.

`load` rejects a profile and names every problem by JSON pointer when:

- the descriptor is invalid;
- a `mapping` key names no capability;
- a readable, writable or invokable capability has no mapping;
- a mapping does not fit its capability (a `set` command on a read-only property, a
  template naming a parameter the action does not have, …);
- a `simulator.initial` value does not fit its capability's value spec.

```text
invalid profile:
  /mapping/led.brightness: the 'write' body must use {{value}}
  /mapping/led.blink: path 'api/blink' must start with '/'
```

## Templates

Commands and payloads that carry values are templates:

| Placeholder | Is replaced by |
|---|---|
| `{{value}}` | the value being written (writes) |
| `{{<param>}}` | an action parameter, e.g. `{{volume}}` |
| `{{command_id}}` | the gateway's command id (MQTT requires it, to match acknowledgements) |

In **JSON** templates (MQTT, HTTP), a string that is exactly one placeholder becomes the
typed JSON value, so `{"on": "{{value}}"}` sends `{"on": true}`. A placeholder inside a
longer string is replaced by text. In **text** templates (SCPI), values are written as
text: numbers in the capability's declared unit, booleans as the profile says, enums by
name.

The simulator reads the same templates in reverse to understand requests. That is how a
profile is tested end to end before it meets hardware.

## Values on the wire

Values on the wire are in the capability's **declared unit**. A request in another unit
of the same quantity (`500mV` for a property declared in `V`) is converted by the
gateway before the adapter sees it.

| Value type | Numeric protocols (Modbus) | Text and JSON protocols |
|---|---|---|
| number, integer | the number | the number |
| boolean | 1 / 0 | `true`/`false`; text also accepts `1`/`0` and `on`/`off` |
| enum | index in `values` (0, 1, …) | the name |
| string | — | the text |

Readings are checked against the value spec. A value out of range, of the wrong type or
not a declared enum value is `adapter.malformed_response`, never passed on.

## The simulator

Each protocol crate has a simulator. It holds the device's state, starting from the
descriptor's defaults and `simulator.initial`, and serves it over the real protocol on a
real socket. It carries out commands as the descriptor says:

- preconditions are checked;
- writes set the property;
- an action with readback verification sets its readback property to the expected
  value.

The simulator also injects the conformance suite's faults (`link_down`, `no_response`,
`lost_ack`, `garbled`, `refuse`), so a profile can be tested to L2 without hardware.

It simulates the **profile**, not your device. A profile that passes against its
simulator is internally consistent. Whether it matches the real device's register map,
topics or command set is checked only by trying it against the device, carefully (see
[docs/hardware/](../hardware/README.md)).

## Limits

- One device per profile file.
- Profiles are read at start. Restart the gateway to change one.
- L3 (coordinated, multi-device) cannot be claimed yet; see the
  [roadmap](../roadmap.md).
