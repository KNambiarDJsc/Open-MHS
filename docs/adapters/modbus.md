# Modbus TCP profiles

`"protocol": "modbus-tcp"`. A **register map**: which coil or register holds each
capability, and how its raw value is scaled. The shared format is described in
[profiles.md](profiles.md); the example is
[climate-chamber.json](../../adapters/openmhs-modbus/profiles/climate-chamber.json).

## Connection

```json
"connection": { "address": "192.168.1.50:502", "unit_id": 1, "reply_timeout_ms": 1000 }
```

| Field | Default | |
|---|---|---|
| `address` | required | `host:port`; Modbus TCP uses 502 |
| `unit_id` | 1 | the unit identifier; behind a Modbus gateway, the downstream device |
| `reply_timeout_ms` | 1000 | how long to wait for any one reply |

## Mapping

```json
"chamber.temperature": { "table": "input", "address": 0, "type": "i16", "scale": 0.1 },
"fan.speed":           { "table": "holding", "address": 1 },
"heater.enabled":      { "table": "coil", "address": 0 },
"alarm.acknowledge":   { "table": "coil", "address": 1, "write": true },
"chamber.purge":       { "table": "holding", "address": 2, "type": "u32", "scale": 0.1, "param": "duration" }
```

| Field | Default | |
|---|---|---|
| `table` | required | `coil` (read-write bits), `discrete_input` (read-only bits), `input` (read-only words), `holding` (read-write words) |
| `address` | required | zero-based protocol address. Data sheets that say "40001" usually mean holding address 0 |
| `type` | `u16` | `u16`, `i16`, `u32`, `i32`, `f32`; 32-bit types use two registers |
| `word_order` | `big` | for 32-bit types: `big` (high word first, the Modbus convention) or `little` |
| `scale`, `offset` | 1, 0 | value = raw × scale + offset, in the capability's declared unit |
| `param` | — | actions: the parameter written to this register |
| `write` | — | actions: a fixed value to write (`true`/`false` for a coil, a raw number for a register) |

The mapping must fit the capability:

- A writable property needs a coil or holding register.
- Booleans live in bit tables, or in a register as 0 or 1.
- Enums are stored as their index.
- An action writes exactly one thing: its single parameter (`param`), or a fixed value
  (`write`). Actions with more than one parameter cannot be mapped.

Writes are refused before anything is sent when the value cannot be represented
exactly: a fraction finer than `scale`, or a value outside the type's range. Nothing is
silently rounded or clipped.

## What each answer means

Each command is a single Modbus write (function 5, 6 or 16), sent once. The adapter
checks that the device's echo matches the request.

| The device… | Gateway outcome |
|---|---|
| echoes the write | `APPLIED`, then verified by readback if declared |
| answers exception 01, 02, 03 or 06 (illegal function, address or value; busy) | `REJECTED` with the reason |
| answers exception 05 (acknowledge: accepted, still working) | applied; readback decides |
| answers exception 0A or 0B (a Modbus gateway could not reach it) | not sent |
| answers exception 04 (device failure) | `FAILED`, device failure |
| cannot be reached (connection refused) | not sent |
| does not answer in time, drops the connection, or answers something that does not match | `UNKNOWN_OUTCOME`; reconciled by readback where declared |

After a timeout or a protocol error the connection is closed and opened again. A late
reply could otherwise be taken as the answer to the next request.

## Limits

- Modbus TCP only; Modbus RTU over serial is not supported yet.
- One value per register or coil. Bit fields inside a register, strings and 64-bit
  values are not supported.
- Reads fetch one capability at a time. There is no block read.
- No security: Modbus has none. Keep devices on an isolated network.
