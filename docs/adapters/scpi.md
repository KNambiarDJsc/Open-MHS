# SCPI profiles

`"protocol": "scpi"`. A **command set**: which query reads each property, which command
sets it, and which command runs each action, over TCP or a serial port. The shared
format is described in [profiles.md](profiles.md); the example is
[bench-psu.json](../../adapters/openmhs-scpi/profiles/bench-psu.json).

## Connection

```json
"connection": {
  "transport": { "tcp": { "address": "192.168.1.40:5025" } },
  "reply_timeout_ms": 1000
}
```

```json
"transport": { "serial": { "port": "COM3", "baud": 9600 } }
```

| Field | Default | |
|---|---|---|
| `transport` | required | `tcp` with `address` (instruments usually listen on 5025), or `serial` with `port` and `baud` (default 9600, 8N1) |
| `reply_timeout_ms` | 1000 | how long to wait for any one reply |
| `completion` | `*OPC?` | appended to every command; the instrument answers `1` when it has finished |
| `error_query` | `SYST:ERR?` | reads one error-queue entry: `0,"No error"` or `-222,"Data out of range"` |
| `identify` | `*IDN?` | used to connect and for health checks |

Lines end with `\n`.

## Mapping

```json
"psu.voltage.setpoint": { "query": "SOUR:VOLT?", "set": "SOUR:VOLT {{value}}" },
"psu.output.enabled":   { "query": "OUTP?", "set": "OUTP {{value}}", "booleans": ["ON", "OFF"] },
"psu.protection.clear": { "send": "OUTP:PROT:CLE" },
"psu.trigger":          { "send": "*TRG" }
```

| Field | |
|---|---|
| `query` | readable properties: a fixed query ending in `?` |
| `set` | writable properties: a command using `{{value}}` exactly once |
| `send` | actions: a command, optionally using the action's parameters (`TRIG:DEL {{delay}}`) |
| `booleans` | how a boolean is written: `[true, false]` text. Default `["1", "0"]`. Reading accepts `1`/`0` and `ON`/`OFF` |

Numbers are written in the capability's declared unit, without a unit suffix. Each
command must be a single SCPI command: no `;` and no line breaks. The adapter adds its
own completion query.

## What each answer means

A command is sent once, as `<command>;*OPC?`. Before it, the adapter empties the error
queue; after it, the adapter reads the queue.

| The instrument… | Gateway outcome |
|---|---|
| answers `1` and has no error queued | `APPLIED`, then verified by readback if declared |
| answers `1` and queues an error (`-222,"Data out of range"`) | `REJECTED` with that error |
| cannot be reached, or the error queue could not be cleared | not sent |
| does not answer in time, drops the connection, or answers something other than `1` | `UNKNOWN_OUTCOME`; reconciled by readback where declared |

## Timeouts, cancellation and retries

An instrument answers in order and finishes a slow command before it reads the next
line, so after a timeout its answer can still arrive. The adapter makes sure that answer
is never read as the reply to anything else:

1. **Every exchange runs to completion or its own timeout.** If the caller stops waiting
   (a command deadline, a client that disconnects), the line is still written whole and
   the reply is still read. A half-written command or an unread reply never stays on the
   link.
2. **After a timeout, the link resynchronizes before sending anything else.** It sends
   `identify` (`*IDN?`) and discards every line until that query's answer. Late answers
   to earlier requests are dropped there, including late answers to an earlier `*IDN?`.
3. **A busy instrument is sent nothing new.** If the resync answer does not come within
   `reply_timeout_ms`, the instrument is still working on the earlier command. The new
   command is **not sent** and fails with `adapter.not_sent`, so the gateway knows for
   certain it had no effect. The next request tries the resync again.
4. **The command that timed out is never re-sent.** It ends `UNKNOWN_OUTCOME`. If the
   capability has a readback, `openmhs commands reconcile <id>` reads the instrument and
   settles it as `RECONCILED_APPLIED` or `RECONCILED_NOT_APPLIED`; if not, an operator
   checks the instrument and runs `openmhs commands resolve`. Until then, a new command
   to the same capability is refused with `command.outcome_unresolved`, unless the
   capability is declared `idempotent` (setting a level twice is the same as once;
   triggering twice is not).

A `stop` capability skips the device lock, but on SCPI it still waits its turn on the
link, up to `reply_timeout_ms` behind the command in flight. An instrument that is
wedged mid-command has to be stopped by its own front panel or interlock.

## Serial ports

`serial` uses the operating system's port (`COM3` on Windows, `/dev/ttyUSB0` on Linux).
The simulator test exercises it over a pseudo-terminal pair on Unix only. On Windows,
serial support compiles and shares all logic with TCP, but no test in this repository
has opened a Windows serial port.

## Limits

- No IEEE 488.2 status-byte polling and no binary block data (`#…`).
- No VXI-11 or HiSLIP; use the instrument's raw socket port.
- One command in flight at a time per instrument. A `stop` waits behind it on the link.
- The resync needs an `identify` query with a fixed answer; `*IDN?` is fixed on every
  instrument the project knows of.
