# Architecture gap analysis

Where OpenMHS falls short of being a reliable hardware tool runtime, found by reading the
code on `main` at `37c56fb` (2026-10-26) and running its tests. Each gap names the file and
line that shows it. Severity is about what can go wrong on a real bench, not about effort.

Companion documents: [CURRENT_STATE_AUDIT.md](CURRENT_STATE_AUDIT.md) (what exists),
[NEXT_MILESTONE_ROADMAP.md](NEXT_MILESTONE_ROADMAP.md) (what to do about it).

**Status of each gap.** *Fixed here* means fixed on the `strategy/availability` branch with
tests ([what was built](CURRENT_STATE_AUDIT.md#7-what-was-implemented-on-this-branch)). *Open* means
documented only.

## Summary

| # | Gap | Severity | Status |
|---|---|---|---|
| H1 | One unreachable device stops the whole gateway from starting, and crash recovery never runs | High | Fixed here |
| H2 | Device health is never checked at runtime; agents cannot tell an offline or busy device from a refusing one | High | Fixed here |
| H3 | SCPI link wedges for good after requests are lost on a serial line (CI red on Linux and macOS) | High | Fixed here |
| H4 | Readback verification and reconciliation accept a reading older than the command | High | Fixed on `fix/stale-readback` ([ADR-0020](../adr/0020-readback-must-follow-dispatch.md)) |
| H5 | A command timeout aborts the adapter's future; Modbus and HTTP can drop a half-written frame | Medium | Open |
| H6 | The unresolved-outcome hold is per capability; related capabilities are not held | Medium | Open |
| H7 | Rate history resets on restart | Low (documented) | Open |
| H8 | The journal grows without bound: every read and preflight is an audit record, and nothing is ever retired | Medium now, high for long-running labs | Minimum protection on `fix/stale-readback` ([ADR-0021](../adr/0021-journal-quota-and-reserve.md)): a size limit and a critical reserve. Segments, retention and archives still open |
| H9 | Availability probes ran one device at a time | Medium at scale | Fixed here (bounded concurrency, one outstanding probe per device) |
| H10 | The gateway registers devices one after another at start, each waiting up to a probe timeout | Medium at scale | Fixed (concurrent, bounded) |
| H11 | No backoff for long-offline devices, no hysteresis for flapping ones, no per-adapter probe budget | Medium at scale | Backoff fixed; hysteresis and per-adapter budget open ([RFC](STORAGE_AND_SCALING_RFC.md) §7) |
| A1 | No per-caller view of what it may do: agents find out by being refused | Medium | Open |
| A2 | Devices can only be added by restarting the gateway | Medium | Open (design in [PHYSICAL_TOOL_RUNTIME_V2.md](PHYSICAL_TOOL_RUNTIME_V2.md)) |
| A3 | MCP is stdio only; no resources, no notifications for approvals or status | Medium | Open (documented) |
| S1 | Same-OS-user processes can read the operator token | High in shared machines | Open (documented, threat model) |
| S2 | Adapter processes are not sandboxed | High for third-party adapters | Open (documented) |
| S3 | A command already dispatched survives credential revocation | Low | Open (documented) |
| S4 | Audit chain is tamper-evident but unsigned; whoever writes the home can rewrite it | Medium | Open (documented) |
| D1 | No published binary: first success needs a Rust toolchain and a 5–15 minute build | High for adoption | Open (owner actions) |

## 1. Hardware execution

### What is already right

The command path is the project's strongest asset. One path, traced from an agent to the
device (`crates/openmhs-engine/src/engine.rs`):

1. `submit_inner` (L490) validates against `command.schema.json` and journals the command
   with an idempotency key, deduplicated per principal and device in one SQLite transaction
   (`journal.rs` L248–271). A replay with a different body is `command.idempotency.conflict`.
2. `evaluate` (L579) runs, in order: the credential scope (before any device lookup, so a
   credential cannot probe for devices), device and capability lookup, leases, the policy
   decision, stateless checks (units, range, enum), then takes the device lock and runs the
   unresolved-outcome hold and live checks (preconditions, freshness, rate, slew) on readings
   taken *now*. A missing reading fails closed (L833–841).
3. `run` (L680) holds the command for approval or moves it to `APPROVED`.
4. `dispatch` (L856) journals the intent and the readback expectation **before** the adapter
   is called (L879–892), then races the adapter against the capability timeout.
5. `verify` (L1037) polls readback until it matches, times out to `VERIFICATION_FAILED`, or
   ends `UNVERIFIABLE` when there is no usable reading.
6. Any timeout, crash or disconnect after dispatch ends `UNKNOWN_OUTCOME`, never retried.
   `reconcile` (L1370) settles it by reading; `resolve` (L1426) lets a person settle it.
   `recover` (L1485) turns everything in flight at a crash into `UNKNOWN_OUTCOME`.

Acknowledgement and physical execution are distinct states (`ACCEPTED`, `APPLIED`,
`VERIFIED`). The adapter contract (`crates/openmhs-adapter/src/lib.rs`) classifies every
failure by whether the device might have acted (`outcome_uncertain`, L78). `NotSent` is the
only failure an adapter may use to say "nothing reached the device". These semantics are the
same for every protocol, because adapters never decide lifecycle states; the engine does.

### H1. One unreachable device stops the gateway — fixed here

`Engine::register` calls `adapter.connect()` and returns its error (engine.rs L311), and
`openmhs_gateway::start` propagates it (`crates/openmhs-gateway/src/lib.rs` L197–205). Every
protocol adapter's `connect` probes the device (Modbus `lib.rs` L279, HTTP L482, SCPI L321,
MQTT L755). So a powered-off instrument means:

- no other device is reachable either;
- `recover()` (lib.rs L223) never runs, so commands left in flight by a crash are not moved
  to `UNKNOWN_OUTCOME` until every device is back;
- the CLI explains it but cannot help (`crates/openmhs-cli/src/lib.rs` L722–748).

A lab always has something switched off. This was not an ADR decision; no ADR or spec
mentions it. **Fix:** register the device as `OFFLINE`, start, and bring it online when it
answers. Commands to an offline device are refused before any traffic; stops are not.
The registration `connect` is itself bounded by the probe timeout, so a device that drops
packets instead of refusing them cannot hold the start (test:
`a_hung_device_has_at_most_one_outstanding_probe`).

### H2. Health is never consulted at runtime — fixed here

`HardwareAdapter::health` is implemented by every adapter but called only by the
conformance runner (`crates/openmhs-conformance/src/runner.rs` L487, L690, L731). The
gateway, API, MCP server and SDK expose no availability. An agent cannot tell "the PSU is
off" from "the PSU refused". The device lock (engine.rs L82) tells the engine a command is
in progress, but nothing tells a caller. **Fix:** a status per device (`ONLINE`,
`DEGRADED`, `OFFLINE`, with reason and times), a `busy` flag, the active lease, a
`devices.status` method, and the same fields in MCP `list_devices`, the CLI and the SDK.

### H3. SCPI link wedges after lost requests — fixed here

Commit `37c56fb` made the SCPI link resynchronize after a timeout by counting sync queries
(`adapters/openmhs-scpi/src/link.rs`, `resync`). It assumed every query it sent would be
answered. On a serial line the instrument can drop what it receives during an outage (the
simulator does exactly that, `sim.rs` L170–175), so the count never returns to zero and the
link refuses every request forever. CI run 40376433981 failed on Linux and macOS in
`bench_psu_over_a_serial_line_reaches_its_declared_level`; Windows passed only because the
serial test is `#[cfg(unix)]`. **Fix:** only sync queries are sent after a timeout, and an
instrument answers those at once and in order, so a long silence after one has been
answered means the rest were lost, not queued. A portable regression test
(`requests_lost_while_the_line_was_down_do_not_wedge_the_link`) reproduces it on every OS.

### H4. Readback is not required to be newer than the command — fixed on `fix/stale-readback`

`verify` and `reconcile` accept any GOOD reading: `v.is_usable(self.now(), None)`
(engine.rs L1082, L1400; `None` means no age limit, `state.rs` L45–54). An adapter that
returns a cached or retained value (MQTT retained state, a hung node's last message) can
produce:

- `VERIFIED` from a reading taken before the device acted, if the old value already matched;
- worse, `RECONCILED_NOT_APPLIED` from a reading taken before a slow device applied the
  command. An agent told "not applied" may send it again: the duplicate physical effect the
  lifecycle exists to prevent.

Experiment 02 shows the MQTT adapter happens to behave, but the engine should not depend on
every adapter getting it right. **Done:** verification and reconciliation require `observed.timestamp >= dispatched_at`
(minus a configured clock allowance, default 0); otherwise `UNVERIFIABLE` / not
reconcilable. A reading that does not match must also postdate the moment the gateway gave up.
See ADR-0020 and `tests/stale_readback.rs`. **Still open:** a conformance check
(`L2.stale_readback`) so adapters that serve cached values fail the suite, and the
vacuous-match case (setting a value the device already had).

### H5. A timeout aborts the adapter mid-flight — open

On timeout the engine calls `handle.abort()` on the adapter's `execute` task (engine.rs
L926). Aborting a future at an `.await` inside a write can leave half a frame on the wire.
SCPI fixed this for itself by running every exchange detached (`link.rs` L270). Modbus and
HTTP do their I/O inside `execute` with no such protection. **Recommendation:** stop
aborting. Let the task finish in the background, ignore its result for the lifecycle (the
command is already `UNKNOWN_OUTCOME`), and log what it eventually reported as evidence for
reconciliation. Add a conformance fault that cancels mid-write.

### H6. The unresolved-outcome hold is per capability — open

`unresolved_refusal` keys on `(device, capability)` (journal.rs L690). An uncertain
`psu.output.enabled` does not hold `psu.voltage.setpoint`, though on a real supply one
determines what the other does. The graph (`spec/topology.md`) already has the vocabulary
(`shares_resource`). **Recommendation:** let a descriptor declare `holds_with` groups;
default to the capability alone.

### H7. Rate history resets on restart — open, documented

Mutation history is in memory (engine.rs L84). Slew limits are checked against a fresh
reading, so they survive; rate limits do not. Persist the history in the journal.

### H8. The journal grows without bound — open

`Engine::read` appends a `read` audit record for every allowed or refused read (engine.rs,
`read`), and every `commands.preflight` appends `command.preflight`. Nothing deletes or
rolls over records, because the audit log is one hash chain verified from the first record
(`journal.rs`, `verify_audit`). An agent that polls one reading a second writes about 86,400
records a day; at a few hundred bytes each that is tens of megabytes a day per agent, kept
forever, and `audit verify` time grows with it.

**Recommendation (needs an RFC, since it touches audit integrity):**

1. **Segments with checkpoints.** Close the chain into segments (by size or day). Each
   segment ends with a checkpoint record carrying its last hash, so a new segment's chain
   starts from it and old segments can be archived or deleted without breaking
   verification of what remains. Signing checkpoints is the signed-audit-export item of the
   threat model.
2. **Retention policy** per record kind, operator-configured: keep command, approval,
   grant, token and availability records for the retention period; keep reads as counts.
3. **Aggregate reads.** One `read.summary` record per principal, device and capability per
   minute (count, first and last value), instead of one record per read. Refused reads stay
   individual: they are security evidence.
4. **Bounded verification.** `audit verify --since <checkpoint>`.
5. **Expose size.** `gateway.info` reports journal size and record count, so growth is
   visible before it is a problem.

Availability records are already bounded by design: only *changes* are audited (a probe
that finds the same state writes nothing). A device that flaps every few seconds would
still write two records per flap; damping (require N consecutive probes before a change)
is the follow-up if that is seen in practice.

### H10. Registration was sequential — fixed here

`openmhs_gateway::start` registers adapters in a loop. With every connect bounded by the
probe timeout (2 s) a fleet of 1,000 devices that all drop packets takes about 2,000 s to
start, though each is registered `OFFLINE`. The gateway now registers devices concurrently
under the same probe budget (at most 16 at a time) unless `--require-devices` asks for the
strict, ordered start. The order of `device.registered` audit records is therefore no longer
deterministic; no test or document depended on it. Measured in
`many_wedged_devices_do_not_make_the_gateway_start_one_timeout_at_a_time`: 40 devices whose
connect never returns start in about three timeouts (about 6 s) instead of 40 (80 s).

### H9. Probes ran one at a time — fixed here

The first version of the availability monitor probed devices in sequence, so a sweep of
100 switched-off devices with a 2 s probe timeout took over three minutes. Probes now run
concurrently, at most 16 at a time (`MAX_CONCURRENT_PROBES`), and the monitor waits for a
sweep to finish before sleeping, so sweeps never pile up. A probe that outlives its timeout
is abandoned, not cancelled (cancelling could cut an exchange in half), and its device is
not probed again until it returns, so a hung device costs one task, not one per sweep.
Measured with virtual time in `a_thousand_offline_devices_start_and_one_sweep_brings_them_all_online`:
1,000 devices answering in 100 ms swept in under 64 x 100 ms with at most 16 probes at
once, against 100 s sequentially. Virtual time shows the schedule, not CPU or memory; those
need the M14 benchmark.

### H11. Probe policy is minimal — backoff fixed, the rest open

The periodic sweep now backs off for a device that keeps not answering: the base interval
doubled per consecutive failure, capped at 5 minutes, plus up to a quarter more derived from
the device id so devices that went down together are not probed on the same tick
(`probe_backoff_ms`). An answering device, or one a command reaches, is back on the base
interval at once; commands and `devices status` still probe on demand, so backoff never
makes a command refuse on old news. Still open: hysteresis against flapping, a `RECOVERING`
state, and a per-adapter budget (a serial bus or Modbus gateway is one link for many devices).
Designed in [STORAGE_AND_SCALING_RFC.md](STORAGE_AND_SCALING_RFC.md) §7;
to be tuned from measurements.

### Concurrency, cancellation, leases

- Non-stop commands are serialized per device by the lock, held from live checks through
  verification (engine.rs L647–651, L689). Correct, and tested (`invariants.rs`).
- Leases are all-or-nothing, follow `contains` and `shares_resource`, and are checked again
  at approval (`leases.rs`, `leases_http.rs`).
- Cancellation is only before dispatch (L1339). There is no "cancel in flight", correctly:
  that would be a stop, which is an ordinary authorized command (ADR-0010 amendment).
- Stops skip the lock and can run concurrently with a command on the same adapter. The
  adapter contract does not say adapters must tolerate that. Document it in the trait.

## 2. Agent usability

Can an arbitrary MCP or Python agent use a device without protocol knowledge? **Mostly
yes, before this change; more so after.**

| Need | Before | After this change |
|---|---|---|
| Discover devices | `list_devices` (MCP), `devices.list` | same |
| Inspect capabilities, types, units | `describe_device`: full descriptor, free text quoted as untrusted | same |
| Know a device is unavailable or busy | **No** | `availability`, `reason`, `busy`, `lease` in `list_devices`; `devices.status` |
| Know what it is permitted to do | **No**: `check_command` per action | No (A1) |
| Ask before acting | `check_command`, `check_plan` | `check_command` also says `device.unavailable` |
| Meaningful results | `CommandView`: `confirmed`, `reasons`, `next_steps` (`crates/openmhs-mcp/src/guidance.rs`) | plus a step for `device.unavailable` |
| Understand unresolved outcomes | Instructions rule 6, `reconcile_command` | same |
| New devices without coding | Restart the gateway (A2) | Restart, but the restart no longer fails when one is off |

Nothing in MCP is protocol-specific: `submit_command` takes a device, a capability and a
typed value. Agents never see registers, topics or SCPI strings. That is the core design
property, and it holds.

**A1. No "what may I do" view.** An agent learns its permissions by being refused. A
`capabilities.allowed` view (the intersection of the credential's scopes and the grants,
per device) would cut wasted turns. It must not reveal other principals' grants.

**A2. Static registration.** Devices come only from `serve` flags. Hot add/remove is a
design question (who may add a device, how its descriptor is pinned); see
[PHYSICAL_TOOL_RUNTIME_V2.md](PHYSICAL_TOOL_RUNTIME_V2.md) §3.

**A3. MCP transport.** stdio only, tools only. Approvals and status changes cannot be pushed;
agents poll (`get_command(wait_ms)`). Streamable HTTP and resource subscriptions are the
standard answer; they need the gateway's TLS story first (S-series).

## 3. Interoperability

OpenMHS should sit above control systems, not replace them. Current position:

| System | State | Recommendation |
|---|---|---|
| Modbus TCP, MQTT, SCPI (TCP, serial), HTTP | Profile adapters, integration-tested against in-repo protocol simulators | Keep. Add Modbus RTU only with a real device to test |
| SCPI/VISA | Raw socket and serial only | Do not write a VISA stack; a Python adapter over PyVISA reuses it |
| OPC UA | Read-only Python adapter (asyncua), CI only, no committed report | Next: writes and methods mapped through the descriptor, against an open62541 or asyncua server. Reuse companion-spec information models, do not reinvent them |
| ROS 2 | Observation-only Python adapter, CI only, no committed report | Next: supervisory **actions** (goal, feedback, result) map naturally to OpenMHS invoke with `ACCEPTED`, `APPLIED`, readback. Never sit inside a ros2_control loop |
| ros2_control | Not integrated | Integrate as a client of controller_manager services (switch controllers, read state), never as a hardware interface |
| LeRobot | Not integrated | A Python adapter over its robot classes for observation and bounded supervisory commands; policies keep running in LeRobot |
| SiLA 2 | Not integrated | The best lab fit: SiLA features carry commands, properties and observable commands with progress. A bridge is cheaper than profiles per instrument |
| CAN, serial line protocols | Not integrated | Wait for a real device and a maintainer |

## 4. Security

The M8 threat model (`docs/security/M8_THREAT_MODEL.md`) is thorough and honest; each
threat names its tests. What is not mitigated is listed there. Additions from this review:

| Threat | Impact | Mitigation |
|---|---|---|
| Availability status used as an oracle for device existence | An out-of-scope agent learns which devices exist | Same as today: `devices.list` and `leases.list` are visible to every caller by design (server.rs L337, L358). Status adds no new disclosure. If device existence ever becomes secret, filter all three by scope together |
| Spoofed "online" from a malicious adapter | Commands sent to a device that is off | Status is advisory for agents; the engine still fails closed on reads and readback. A lying adapter can already lie about readings (threat 10) |
| Health probes as unauthorized device traffic | Gateway talks to devices with no agent request | Probes are the adapter's `health()`/`connect()`, which conformance requires to be side-effect free (`L0.health`). Interval is operator-configurable, `0` disables |
| Offline gating bypassed by a stop | A stop reaches a device marked offline | Intended: status bookkeeping never blocks a stop (ADR-0019) |
| Replay of a captured command | Duplicate effect | Idempotency keys are per principal and device; without a key, every submission is new. MCP instructions require keys; the API does not. Consider requiring keys for non-idempotent capabilities |
| Unsafe recovery: an operator resolves `UNKNOWN_OUTCOME` wrongly | Duplicate or missing effect | Recorded with reason and principal; agents cannot resolve (engine.rs L1434). Acceptable |

Process isolation (S2) is the largest open item for a third-party adapter ecosystem: a
signed manifest says who vouches for an adapter, not what it can do. Before community
adapters run by default, run adapter processes under an OS sandbox (Landlock and seccomp on
Linux, AppContainer on Windows, sandbox-exec on macOS) or WASM components for pure-protocol
adapters. ADR-0015 already anticipates this.

## 5. Developer experience (summary; detail in the ecosystem plan)

D1 dominates: there is no release, so the README's one-minute quickstart begins with
`cargo install`, which needs Rust 1.89 and builds about 300 crates. Everything needed to fix
this exists (`dist-workspace.toml`, `release.yml`, `pypi.yml`); it is blocked on owner
actions listed in `docs/release/M10_TEST_REPORT.md`, plus green CI on all three platforms,
which H3 broke.
