# Conformance

Conformance describes how an **adapter + descriptor** behaves through the OpenMHS
contract under test. Levels are cumulative: L2 includes L1 and L0. The executable suite
is the `openmhs-conformance` crate ([ADR-0013](../docs/adr/0013-conformance-suite.md));
`openmhs conformance` runs it against the built-in simulators.

**A conformance level is not a safety certification.** It says nothing about whether a
device is safe to operate, only that its integration behaves as specified.

## 1. Levels

| Level | Name | Requirements |
|---|---|---|
| L0 | Discoverable | Valid descriptor; adapter connects, reports health, describes capabilities matching the descriptor; clean shutdown |
| L1 | Observable | Reads of every readable property return typed values in the declared unit, with timestamp, source and quality; subscriptions deliver updates (deferred, §4); disconnects surface as `BAD` quality or `adapter.disconnected`, never as stale-but-`GOOD` values |
| L2 | Controllable | Every write/invoke is honoured within declared timing; readback verification behaves as declared; device rejections map to `REJECTED`; timeouts map to `UNKNOWN_OUTCOME`; the adapter never retries a non-idempotent command on its own; malformed responses map to `adapter.malformed_response` |
| L3 | Coordinated | Takes part in plans and leases: a lease keeps others' commands off the device, its commands work as plan steps, failures stop the plan as declared and an unknown outcome is never guessed, and a declared stop gets through a lease ([plans.md](plans.md), [topology.md](topology.md)) |

Fault behaviour (disconnects, timeouts, malformed data, crashes) is part of every level
above L0 and is tested by fault injection, not assumed.

## 2. Reaching a level

Every check ends with one status:

| Status | Meaning |
|---|---|
| `pass` | The requirement held |
| `fail` | It did not; the message says what happened and what to fix |
| `untested` | It could not be tested, e.g. the fixture cannot inject the fault it needs |
| `not_applicable` | It does not apply, e.g. no-retry checks for an idempotent capability |

A level is **reached** when that level and every level below it have no `fail` and no
`untested` checks, and at least one check at that level passed. Nothing is assumed: a
check that could not run blocks the level just as a failure does, and a read-only sensor
cannot reach L2 because nothing at L2 passed.

A check that depends on another check that failed is `untested`, and its message names
that check (`see L2.command`). Each problem is reported once, where it starts, and the
fault checks give advice only about commands that work.

A descriptor may declare the level its author claims in `adapter.conformance`
([descriptor.md](descriptor.md)). The report's `passed` is true when the level reached is
at least the declared one (or, when none is declared, at least L0).

## 3. Fixtures and faults

An adapter is tested together with a **fixture**: whatever stands where the device would
be (a simulator, a fake server speaking the device's protocol) and can be made to
misbehave on request. The fixture reports which faults it supports.

| Fault | What the fixture does |
|---|---|
| `link_down` | All communication with the device fails, reads included |
| `no_response` | State-changing requests get no answer and are not applied (a lost command) |
| `lost_ack` | State-changing requests are applied, but the answer never arrives |
| `garbled` | State-changing requests are applied, and the answer cannot be parsed |
| `refuse` | The device refuses state-changing requests and applies nothing |

Every fault except `link_down` affects only state-changing requests, so the gateway's
precondition reads and readback keep working while it is active.

A fixture can also count **deliveries**: state-changing requests that reached the device,
applied or not. The no-retry requirement needs the count.

## 4. Requirements

Ids are stable: they are only ever added, and a released id never changes meaning. The
suite's registry (`crates/openmhs-conformance/src/requirements.rs`) must match this table
exactly; a test enforces it. Per-capability requirements produce one check for each
readable property (`L1.read*`, `L1.disconnect.read`) or each writable property and action
(`L2.command` to `L2.no_retry`).

| Id | Level | Fault | Requirement |
|---|---|---|---|
| `L0.describe` | L0 | | `describe` returns a descriptor |
| `L0.descriptor` | L0 | | The descriptor passes schema and semantic validation |
| `L0.adapter_id` | L0 | | `adapter_id` equals the descriptor's `adapter.id` |
| `L0.describe_stable` | L0 | | Describing twice returns the same descriptor |
| `L0.connect` | L0 | | `connect` succeeds and the device registers with the engine |
| `L0.health` | L0 | | `health` reports healthy after connecting |
| `L0.shutdown` | L0 | | `shutdown` succeeds |
| `L1.read` | L1 | | Every readable property can be read |
| `L1.read.value` | L1 | | Readings have the declared type and unit, and stay within the declared range or values |
| `L1.read.timestamp` | L1 | | Readings are not timestamped in the future, and are as fresh as the descriptor declares |
| `L1.read.quality` | L1 | | Readings from a healthy device have `GOOD` quality |
| `L1.subscribe` | L1 | | Subscriptions deliver updates. Deferred: the adapter contract has no subscriptions yet |
| `L1.disconnect.read` | L1 | `link_down` | With the link down, a read fails with `adapter.disconnected` or `adapter.timeout`, or returns a reading that is not `GOOD` |
| `L1.disconnect.health` | L1 | `link_down` | With the link down, `health` does not report healthy |
| `L1.reconnect` | L1 | `link_down` | When the link returns, health and `GOOD` readings come back without re-registering |
| `L2.command` | L2 | | A valid write or invoke is carried out within the declared `timing.timeout_ms` |
| `L2.verification` | L2 | | Verification behaves as declared: readback confirms the change (`VERIFIED`); `mode: none` claims nothing (`UNVERIFIABLE`) |
| `L2.reject` | L2 | `refuse` | A command the device refuses ends `REJECTED` with `adapter.rejected` |
| `L2.malformed` | L2 | `garbled` | An answer that cannot be parsed ends `UNKNOWN_OUTCOME` with `adapter.malformed_response` |
| `L2.timeout` | L2 | `no_response` | A command that gets no answer ends `UNKNOWN_OUTCOME`, never `FAILED` or a success |
| `L2.lost_ack` | L2 | `lost_ack` | A command applied without an answer ends `UNKNOWN_OUTCOME`, and reconciling by readback finds it applied |
| `L2.no_retry` | L2 | `no_response` | The adapter never retries a non-idempotent command: each reaches the device at most once |
| `L2.recovery` | L2 | | After faults clear, commands are carried out again without re-registering |
| `L3.leases` | L3 | | While another principal holds the device's lease, its commands are refused (`lease.held`) before they reach the device; the holder's are carried out |
| `L3.plan` | L3 | | A valid command runs as a plan step and the plan releases its lease. Under `refuse` the plan halts at that step and sends nothing more; under `no_response` it stops for a decision and never guesses |
| `L3.stop` | L3 | | A declared stop action is carried out while another principal holds the device's lease (the stop lane) |

Notes:

- **`L1.subscribe` is `not_applicable` for now.** The adapter contract (v0.1) has no
  subscription API, so there is nothing to test. It becomes a real check when the
  contract gains one.
- **L3 runs on one device.** The suite drives one adapter, so L3 checks what one device
  contributes to coordination: its commands under a lease, as plan steps, under faults,
  and its stop. They use the capability whose command L2 showed is carried out, and a
  second principal, `conformance:other`. Multi-device behaviour (claims through
  relationships and shared resources, ordering across devices, compensation) is the
  engine's, and is tested there.
- **`L3.plan`** injects `refuse` and `no_response` when the fixture supports them, and
  says in its message which it could not inject. It passes on the one-step plan alone
  when the fixture supports neither.
- **`L3.stop`** is `not_applicable` without a capability marked `stop`. It shows that a
  stop is not blocked by a lease; it does not show that a stop interrupts a command in
  flight, which the simulators' own tests cover.
- **`L2.lost_ack` with `verification.mode: none`** passes on `UNKNOWN_OUTCOME` alone: with
  nothing to read back, an operator resolves it. When the expected readback already held
  before the command, the check passes but says that reconciliation could not tell this
  command apart.
- **`L2.no_retry`** counts deliveries during the `refuse`, `garbled`, `no_response` and
  `lost_ack` checks. For non-idempotent capabilities the `no_response` command gets the
  capability's full `timing.timeout_ms`, so an adapter that would retry has time to.

## 5. How the suite exercises a device

- L0 and L1 call the adapter directly, so the report can show exactly what it returned.
  L2 and L3 go through a real engine with an in-memory journal: lifecycle states are the
  engine's, and L2 is about the adapter's answers making the engine reach the right ones.
- The suite acts as principal `conformance:suite` (and, for the lease checks,
  `conformance:other`), under a policy that allows every operation, so only the device
  and the descriptor decide what happens.
- **Values come from the descriptor.** A write moves a small step from the current
  reading: 5% of the declared range, at most half the slew limit, and enough to tell
  apart by readback. Booleans toggle; enums move to another declared value. Action
  parameters that verification reads back move a small step too; others take the middle
  of their range. A caller can pass sample values instead.
- **Preconditions are established from the descriptor.** Before sending, the suite asks
  the engine for a preflight. If a precondition fails, it sets the precondition's property
  (when writable), or invokes an action whose readback declares it sets that property to
  the required value (for example `axis.home` for `axis.homed = true`). Anything else
  leaves the capability's checks `untested`, with the reason.
- **Rate and slew limits are respected** by waiting out their windows between commands.
- **Hazardous capabilities** are exercised by default, because the suite is meant for
  fixtures and simulators. Switching that off leaves their checks `untested`.
- **Commands are really sent.** Run the suite against fixtures and simulators, not
  against hardware that could hurt someone.

## 6. Report

Schema: [`schemas/conformance-report.schema.json`](schemas/conformance-report.schema.json).
Example: [`examples/conformance/sim.psu-01.json`](examples/conformance/sim.psu-01.json).

The report names the subject (adapter id and version, device id, SHA-256 of the
descriptor) and the `sources` its readings carried (`simulator` means a simulator was
tested, not hardware). It lists the fixture's faults, the declared and reached levels,
`passed`, and every check with its status, message and evidence (receipts, readings,
delivery counts).
