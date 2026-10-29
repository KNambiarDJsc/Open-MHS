# Adapter-host protocol (v0.1)

Decision record: [ADR-0015](../docs/adr/0015-adapter-host.md). Reference host:
`crates/openmhs-host`. Reference adapter SDK: `python/openmhs/adapter.py`
([guide](../docs/adapters/python-sdk.md)). Permission manifest:
[permission-manifest.md](permission-manifest.md).

An out-of-process adapter is a child process of the gateway. The gateway (the *host*)
starts it from a permission manifest and talks to it over the child's stdin and stdout.
The adapter translates between OpenMHS and one device. It is **not** a second policy
engine: authorization, the command lifecycle, approvals, receipts and the audit log stay
in the gateway. An adapter never decides whether a command is allowed, never retries a
state-changing command, and never reports an outcome it has not observed.

## 1. Framing

- JSON-RPC 2.0. One message per line: UTF-8 JSON with no raw newline inside, terminated
  by `\n`. A trailing `\r` is tolerated.
- The host writes to the child's stdin and reads its stdout. **stdout carries protocol
  messages only.** Anything else on stdout is a protocol violation (§7). Logs go to
  stderr, which the host forwards to its own log (with credential values redacted) and
  keeps the last 20 lines of for error messages.
- A message may be at most `limits.max_message_bytes` (default 1 MiB). A longer line is
  a protocol violation.
- The host sends **requests**; the adapter answers each with exactly one **response**
  (same `id`) and may send **notifications** (no `id`). The adapter never sends requests
  to the host.
- Requests may be outstanding concurrently. Responses may arrive in any order.

## 2. Lifecycle

```
spawn ──► initialize ──► describe ──► [connect] ──► read / execute / health / ping …
                                                         │
                                                    shutdown ──► stdin closed ──► exit
```

1. The host spawns the process with a cleared environment (§6) and sends `initialize`.
   Until it has answered `initialize` the adapter answers every other request with
   `-32001` (not initialized). `ping` is the one exception.
2. The host sends `describe` and validates the descriptor: it must be a valid
   descriptor ([descriptor.md](descriptor.md)), its `adapter.id` must be the manifest's,
   and every capability and operation it declares must be allowed by the manifest. A
   restarted process must return an identical descriptor.
3. `connect` is sent when the gateway connects the device, and again after a restart if
   the device was connected before.
4. On `shutdown` the adapter releases the device and answers; the host then closes
   stdin and waits `limits.shutdown_grace_ms` before killing the process. An adapter
   whose stdin reaches end of file exits.

## 3. Methods

All `params` and `result` objects are JSON objects unless shown as `null`.

### `initialize`

```json
{"protocol_versions": ["0.1"],
 "host": {"name": "openmhs", "version": "0.1.0"},
 "fixture": false,
 "config": {"seed": 42},
 "permissions": {"network": ["opc.tcp://10.0.0.5:4840"], "serial": [],
                 "filesystem": {"read": [], "write": []}},
 "limits": {"max_message_bytes": 1048576}}
```

Result:

```json
{"protocol_version": "0.1",
 "adapter": {"id": "acme.lamp", "version": "0.1.0"},
 "fixture": {"faults": ["link_down", "no_response"], "deliveries": true}}
```

- **Version negotiation.** The host offers the versions both it and the manifest's
  `runtime.protocol` list. The adapter picks one of them, or answers `-32002` with
  `data.supported` listing its own. Choosing a version that was not offered, or `-32002`,
  quarantines the adapter (§5) — restarting would not help.
- `adapter.id` must equal the manifest's.
- `config` is the manifest's `config`, passed through untouched.
- `permissions` is what the manifest declares. The adapter SDK refuses to open anything
  not listed (see [permission-manifest.md](permission-manifest.md) for what that does and
  does not guarantee).
- `fixture` (result) is present only when the host asked for fixture mode (`"fixture":
  true`) **and** the adapter can inject faults. Fixture mode exists for conformance runs
  against simulators; the CLI never enables it for a real device.

### `describe`

`params: null`. Result: the device descriptor ([descriptor.md](descriptor.md)).

### `connect`, `health`

`params: null`. `connect` result: `null`. `health` result:

```json
{"status": "healthy"}
{"status": "degraded", "message": "humidity sensor not responding"}
{"status": "disconnected", "message": "no route to 10.0.0.5"}
```

### `read`

`params: {"capability": "sensor.temperature"}`. Result: a state value.

```json
{"value": {"value": 21.5, "unit": "Cel"}, "timestamp": "2026-10-24T10:00:00.000Z",
 "source": "device", "quality": "GOOD"}
```

`timestamp` is when the device measured the value if the device says so, otherwise when
the adapter received it. The gateway checks freshness against it.

### `execute`

```json
{"command": {"command_id": "01J…", "capability": "led.on", "operation": "write",
             "value": true, "params": {}, "idempotent": true,
             "deadline": "2026-10-24T10:00:05.000Z"}}
```

Values are already validated and converted to the descriptor's units. Result:

```json
{"status": "applied", "result": {}}
{"status": "rejected", "reason": "interlock open"}
```

- `applied`: the device reports the change applied. `result` holds an action's named
  results. The gateway then verifies the change as the descriptor says.
- `rejected`: the device refused and applied nothing.
- Anything else is an error (§4). The adapter sends the command **at most once**. It does
  not retry it, even after a timeout, and it does not wait past `deadline`.
- Once the command has been handed to the device, the adapter should send the
  notification `progress` `{"command_id": "01J…", "event": "accepted", "external_ref": "…"}` (the SDK's
  `progress.accepted(external_ref=…)`; `external_ref` is optional). `external_ref` is an
  opaque identifier the device or its server gave the accepted work, for example a LabThings
  invocation id. The gateway stores it in the receipt (`adapter_result.external_ref`) so an
  unresolved command can later be matched to the work it started. At most 256 characters, no
  control characters; a longer or malformed one is dropped, not truncated. It is adapter text:
  never used to authorize or decide anything. It is informational: the outcome is the response.

### `ping`

`params: null`, result `null`. Heartbeat (§5). Answered even before `initialize`.

### `shutdown`

`params: null`, result `null`.

### Fixture methods (fixture mode only)

| Method | Params | Meaning |
|---|---|---|
| `fixture.inject` | `{"fault": "link_down"}` | Start a fault from [conformance.md](conformance.md) §3 |
| `fixture.clear` | `null` | End all faults |

Notification `fixture.deliveries` `{"count": 3}`: how many state-changing commands have
reached the simulated device so far. The conformance suite uses it to prove that a
command was not delivered twice.

## 4. Errors

| Code | Meaning |
|---|---|
| `-32700` | Parse error |
| `-32600` | Invalid request |
| `-32601` | Method not found (the host treats it as `adapter.unsupported`) |
| `-32602` | Invalid params |
| `-32603` | Internal error |
| `-32000` | Adapter error; `data.kind` is one of the codes below |
| `-32001` | Not initialized |
| `-32002` | Unsupported protocol version; `data.supported` lists the adapter's versions |

`data.kind` is an adapter error code from [errors.md](errors.md): `adapter.timeout`,
`adapter.disconnected`, `adapter.malformed_response`, `adapter.device_failure`,
`adapter.not_sent`, `adapter.unsupported`, `adapter.internal`. Errors without a known
kind count as `adapter.internal`. For `execute`, the kind decides the command's outcome:
`not_sent` and `unsupported` mean nothing reached the device, `device_failure` means the
device reported a failure; `timeout`, `disconnected`, `malformed_response` and `internal`
mean the device may or may not have changed, and the command ends `UNKNOWN_OUTCOME`
([lifecycle.md](lifecycle.md)).

## 5. Supervision

The host supervises each adapter process; the adapter needs to do nothing beyond
answering requests promptly.

- **Deadlines.** Every call has a timeout: `limits.call_timeout_ms` (default 5000), the
  start sequence `limits.start_timeout_ms` (default 30000), and `execute` the command's
  deadline plus one second. A timed-out call fails with `adapter.timeout`; the process
  keeps running. If writing a request itself times out, framing can no longer be trusted
  and the process is killed.
- **Heartbeats.** Every `limits.heartbeat_ms` (default 2000) the host sends `ping`. After
  `limits.missed_heartbeats` (default 3) consecutive unanswered pings the process is
  killed as hung.
- **Crash and hang.** When the process exits or is killed, every outstanding call fails:
  reads with `adapter.disconnected`, and in-flight `execute` calls with
  `adapter.disconnected`, so the gateway records those commands as `UNKNOWN_OUTCOME`. They
  are **never** resent to the new process.
- **Restart.** A process that died is restarted with exponential backoff
  (`restart.initial_backoff_ms`, doubling up to `restart.max_backoff_ms`). While it is
  down, `execute` fails with `adapter.not_sent`.
- **Quarantine.** More than `restart.max_restarts` restarts within `restart.window_s`, or a
  fault that a restart cannot fix — unsupported protocol version, a descriptor beyond the
  manifest, a changed descriptor, a missing credential, an `initialize` from a different
  adapter id — quarantines the adapter: it is not restarted again, `execute` fails with
  `adapter.not_sent`, and `health` reports it disconnected with the reason. Only restarting
  the gateway clears quarantine.

## 6. Process environment

The host clears the child's environment and sets only:

- a small allow-list the OS needs to run programs (`PATH`, `HOME`, `TMPDIR`, `LANG`,
  `LC_ALL`, `TZ` on Unix; `PATH`, `SYSTEMROOT`, `TEMP`, `USERPROFILE`, `APPDATA`,
  `LOCALAPPDATA` and similar on Windows — `openmhs_host::OS_ENV`),
- the manifest's `runtime.env`,
- each variable named in `credentials`, copied from the gateway's environment. A named
  credential that is not set stops the adapter from starting. Credential values are
  redacted from forwarded stderr.

The working directory is `runtime.cwd`, or the manifest's directory.

## 7. Protocol violations

The host kills the process (and the restart policy applies) when the adapter:

- writes a line to stdout that is not a JSON-RPC 2.0 message, or a response to an id the
  host never used,
- sends a request to the host,
- exceeds `limits.max_message_bytes`,
- closes stdout while still running.

The kill reason, with the stderr tail, is in the error the gateway reports.
