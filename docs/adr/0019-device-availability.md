# ADR-0019: Device availability

- Status: Accepted
- Date: 2026-10-28
- Builds on: [ADR-0001](0001-single-trusted-gateway.md), [ADR-0010](0010-engine-execution-model.md),
  [ADR-0018](0018-orchestration.md)

## Context

Up to M10 the gateway connected to every device before it listened
(`Engine::register` returned the adapter's `connect` error, and `openmhs_gateway::start`
propagated it). One switched-off instrument therefore kept every other device out of
reach, and kept crash recovery from running, so commands left in flight by a crash stayed
unaccounted for until all devices were back. A lab always has something switched off.

`HardwareAdapter::health` was implemented by every adapter and called only by the
conformance runner. Agents could not tell "the supply is off" from "the supply refused",
and a command to a device that was off went to the adapter and ended, at best, `FAILED`
after a connection timeout, at worst `UNKNOWN_OUTCOME`, which then held the capability.

## Decision

### States

Each device has an availability: `ONLINE`, `DEGRADED` (answers, reports a problem) or
`OFFLINE` (did not answer), with the adapter's reason, `since` and `checked_at`.

### Registration

`Engine::register_allow_offline` registers a device whose descriptor validates even if
`connect` fails; it is `OFFLINE`. The gateway uses it unless `require_devices` is set
(`openmhs serve --require-devices`), which keeps the strict start. A descriptor that does
not validate is still refused: that is a configuration error, not an outage.
`Engine::register` keeps its strict behavior for embedders.

### What updates it

- **Probes.** `connect` until it has once succeeded, `health` after, each bounded by
  `probe_timeout_ms` (2 s). Both are side-effect free for a conforming adapter
  (`L0.connect`, `L0.health`). The gateway probes every `health_interval_ms` (5 s; `0`
  disables), only devices not checked within the interval and not busy, at most 16 at a
  time; a sweep finishes before the next sleep, so sweeps never overlap.
- **Passive observation.** A read or command result that reached the adapter: a value or
  a report means it answers (an `OFFLINE` device becomes `ONLINE`); `adapter.disconnected`
  means it does not. Other failures say nothing about reachability.
- **On demand.** `devices.status {refresh: true}` probes devices not checked in the last
  second; callers cannot turn it into a flood.

Only changes are audited (`device.availability`, with `from`, `to`, `reason`); a probe that
finds the same state writes nothing.

### What it gates

A write or invoke to an `OFFLINE` device is probed once more, then refused with
`device.unavailable` before anything is sent, at evaluation (so `commands.preflight` and
`check_command` say the same) and again at approval. The message is the gateway's own; the
adapter's reason is device-written text and is shown, quoted, in status.

**A stop is never refused for availability.** This is narrow: a stop skips this one gate,
as it skips the device lock and leases (ADR-0018). It is still authenticated, scoped,
authorized and validated like every other command. Availability is bookkeeping, and
bookkeeping must not stand between an operator and a stop; if the device really is
unreachable, the stop fails on the link as it would without status. A gateway stop is not
an emergency stop, which belongs to the hardware.

**What a stop's receipt means.** Skipping the gate is not a claim that anything stopped.
The existing lifecycle already distinguishes what a stop needs, and no new states are added:

| Question | State |
|---|---|
| The gateway accepted the request | `APPROVED` (authenticated, authorized, validated) |
| Transmission was attempted | `DISPATCHED` (intent journaled first) |
| The device acknowledged | `ACCEPTED` |
| The relevant state was confirmed | `VERIFIED`, only by readback |
| The gateway cannot say whether the device stopped | `UNKNOWN_OUTCOME` (timeout, disconnect, crash) |
| It reported applied but nothing confirmed it | `UNVERIFIABLE` |

A stop to an unreachable device ends `UNKNOWN_OUTCOME`, never `VERIFIED`
(`a_stop_is_never_refused_for_availability`). A stop is exempt from the unresolved-outcome
hold, so it can be sent again at once. MCP therefore gives an unconfirmed stop different
advice from any other command: it is not confirmed, treat the equipment as still moving or
energized, use its own emergency stop, and sending the stop again is safe
(`an_unconfirmed_stop_is_not_reported_as_stopped`). An independent emergency stop or
interlock must remain available for hazardous equipment; nothing here replaces it.

`DEGRADED` gates nothing. Availability never makes anything more permissive: every other
check still runs and still fails closed on its own readings.

### Visibility

`devices.status` returns, per device, availability, reason, times, `busy` (a state-changing
command holds the device lock) and the active lease. It is visible to every caller, like
`devices.list` and `leases.list` it is built from, so it discloses nothing new. MCP
`list_devices` carries `availability`, `reason` (quoted), `busy` and `lease_holder`; the
CLI has `openmhs devices status`; the Python SDK has `Client.device_status`.

## Consequences

- The gateway starts, and recovery runs, with devices switched off.
- A command to a switched-off device is a clear, immediate `DENIED device.unavailable`
  instead of a timeout or an uncertain outcome.
- Probes are device traffic the operator did not ask for. They are side-effect free by
  contract, bounded, skip busy devices, and can be turned off.
- Status can be stale by up to one interval. That is safe in both directions: a device
  wrongly shown `ONLINE` fails as before; one wrongly shown `OFFLINE` is probed again before
  a command is refused.

## Not decided here

Fleet-scale scheduling (adaptive intervals, exponential backoff with jitter for offline
devices, per-adapter probe budgets, hysteresis against flapping, a `RECOVERING` state) is
the next step once there are measurements to tune it against; see
[docs/strategy/STORAGE_AND_SCALING_RFC.md](../strategy/STORAGE_AND_SCALING_RFC.md) §6.
