# The adapter host

Code: `crates/openmhs-host`. Wire protocol: [spec/adapter-protocol.md](../../spec/adapter-protocol.md).
Manifest: [spec/permission-manifest.md](../../spec/permission-manifest.md). Decision:
[ADR-0015](../adr/0015-adapter-host.md).

Adapters for protocols with no good Rust library (OPC UA, ROS 2) run as separate
processes, usually Python. The adapter host is the gateway's half of that arrangement:
`ProcessAdapter` implements the same `HardwareAdapter` trait as an in-process adapter, so
the engine, policy, journal and conformance suite treat both alike.

```
 agent ──► gateway API ──► engine ──► policy, lifecycle, journal
                              │
                              ▼ HardwareAdapter
                        ProcessAdapter ── stdin/stdout (JSON-RPC, one line per message) ──► adapter process ──► device
                              │                                                            (Python SDK)
                              └─ supervisor: heartbeats, deadlines, restart budget, quarantine
```

## What stays in the gateway

The adapter process is not trusted with decisions. Authorization, approval, the command
lifecycle, verification by readback, receipts and the audit journal all run in the
gateway, before and after the call to the process. The process sees only commands that
policy has already allowed, already validated and converted to the descriptor's units. It
answers "applied", "rejected", or a classified error; the engine decides what that means.

## Failure handling

| What happens | What the host does | What the engine records |
|---|---|---|
| Process writes non-protocol text to stdout | Kills it ("protocol violation"), restarts | In-flight commands `UNKNOWN_OUTCOME` |
| Process crashes | Restarts with backoff | In-flight commands `UNKNOWN_OUTCOME`; reads fail `adapter.disconnected` |
| Process hangs | Heartbeats go unanswered; kills after `missed_heartbeats`, restarts | As for a crash |
| One call is slow | That call fails `adapter.timeout`; the process keeps running | The command `UNKNOWN_OUTCOME` |
| Process down or restarting | Refuses `execute` without sending it | `FAILED` with `adapter.not_sent` |
| Crash loop (budget exhausted) | Quarantines | `adapter.not_sent`; health disconnected with the reason |
| Unsupported protocol version, descriptor outside the manifest, descriptor changed after restart, missing credential | Quarantines at once | As above |
| Gateway shuts down | `shutdown`, close stdin, grace period, kill | — |

A command that was in flight when the process died is **never** sent again, to the old
process or the new one. Whether it reached the device is unknown, so it ends
`UNKNOWN_OUTCOME` and an operator or agent reconciles it by reading the device
([lifecycle.md](../../spec/lifecycle.md)). The host also never retries a call on its own.

## What the tests prove

`crates/openmhs-host/tests/` runs real child processes:

- `conformance.rs`: the Python port of the simulated power supply, through the host,
  passes the same conformance checks as the Rust `VirtualPsu`, check for check (L2).
- `process.rs`: stray stdout output, a crash on read, a crash after a command was applied
  (`UNKNOWN_OUTCOME`, delivered once, not resent), a hang (heartbeat kill), garbage on
  stdout, a request from the child, an unsupported protocol version, a descriptor beyond
  the manifest, calls outside the manifest, a crash loop (quarantine), a descriptor change
  across restarts, environment scrubbing, a missing credential, and shutdown.
- `tests/adapters/misbehave.py` is the adapter that misbehaves on request.

The OPC UA and ROS 2 adapters reach L1 through the host against simulators
(`openmhs conformance --adapter-manifest …`).

## Limits

- No OS sandbox: see the enforcement table in
  [permission-manifest.md](../../spec/permission-manifest.md).
- No subscriptions over the protocol yet; the adapter contract has none either.
- One process per adapter, one device per adapter.
- Quarantine is cleared only by restarting the gateway.
