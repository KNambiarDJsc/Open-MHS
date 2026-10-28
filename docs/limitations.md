# Limitations

What OpenMHS does not do, and what has not been tested. This page is the single list;
the README links here.

## Safety

- **OpenMHS is not a safety system.** It reduces what an agent can do wrong. Emergency
  stops, interlocks and protective devices belong to the hardware and keep final
  authority.
- **A stop is a command.** Actions marked `stop` skip the device lock and leases but are
  still authorized and validated, and only interrupt a move if the adapter and device do
  ([ADR-0010](adr/0010-engine-execution-model.md)).
- **No safety certification is claimed**, for the software or for any device. A
  conformance level says an integration behaved as specified under the tested faults.
- **The conformance suite sends real commands.** Run it against simulators and fixtures
  only.

## Evidence

- **No adapter has been run against physical hardware by the project.** All results are
  from simulators. The [compatibility matrix](compatibility.md) shows it per adapter, and
  the [hardware guides](hardware/README.md) are unverified.
- **Passing against a simulator** shows the contract and the adapter agree with each
  other, not that a particular real device works.
- **OPC UA and ROS 2** adapters are checked at L1 in CI only; no report is committed, so
  the matrix shows them as not verified.
- **Platforms.** Windows 11 and Linux (Ubuntu) are tested. macOS arm64 is configured in
  CI; macOS x86_64 is untested. Serial SCPI is untested on Windows.

## Scope

- **Twin preflight has no physics.** A twin does what it is told; `ok` means a plan fits
  the contract and current readings, not that it will succeed.
- **OPC UA is read-only;** ROS 2 is observation-only (no services or actions). No Modbus
  RTU.
- **MCP is stdio only:** no HTTP transport, resources or approval notifications.
- **Plans** run steps in order; there are no parallel steps.
- **No signed descriptors or signed audit export.**
- **Rate and slew history, and simulator state, reset on restart.**
- **Readback cannot prove an effect that leaves no difference:** a command that sets a
  value the device already had looks the same as one that did nothing. Devices whose clocks
  run behind the gateway's need `--readback-skew-ms` or their commands stay `UNVERIFIABLE`
  ([ADR-0020](adr/0020-readback-must-follow-dispatch.md)).
- **Journal limits are a floor, not a lifecycle.** The journal still grows until the limit;
  nothing is archived or deleted ([ADR-0021](adr/0021-journal-quota-and-reserve.md)).
- **Availability is an observation.** It can be stale by one probe interval (5 s by
  default). Probes are the adapter's side-effect-free `connect` and `health`. Periodic
  sweeps back off for long-offline devices, but there is no damping for flapping ones and
  no per-adapter probe budget yet ([ADR-0019](adr/0019-device-availability.md)).
- **Events are notifications, not truth, and live in memory.** `events.poll` keeps the newest
  8 192 events and reports how many a slow consumer missed; a restart loses them and ends
  every watch. Watches sample through `read` (at most every 200 ms, 16 per principal), so
  event-kind capabilities without `read` cannot be watched yet, and there is no push
  transport. Individual watch samples are not audited; creation and end are
  ([ADR-0023](adr/0023-state-fabric.md)).
- **Insights are evidence, not knowledge.** `get_insights` summarizes the newest 500
  commands a credential can list; a value confirmed earlier says nothing certain about
  now, and nothing in it changes a limit or a permission. It does not model the device
  or tune anything ([ADR-0022](adr/0022-insights-from-the-journal.md)).

## Security

- **Single-user trust boundary.** Any process running as your OS user can read the
  operator token in the gateway home. Give agents scoped credentials.
- **The adapter host has no OS sandbox.** Permission manifests are declared and checked,
  not enforced by the operating system. Signed manifests say who vouches for an adapter,
  not that it is safe.
- **Not security audited.** See the [threat model](security/M8_THREAT_MODEL.md) and
  [SECURITY.md](../SECURITY.md).
- **Local use.** The API is for a gateway on the same machine; exposing it beyond
  loopback is possible (`--allow-remote`) but is not a supported deployment.

## Project

- **Pre-alpha.** Before 1.0 the specification, adapter contract and API can change.
- **Independent.** Not affiliated with, endorsed by, or compatible with Anthropic's Model
  Hardware Standard, whose specification is not public
  ([ADR-0007](adr/0007-mhs-independence.md)).
- **Releases.** No binaries, crates or packages have been published. Crates cannot be
  published to crates.io yet because `openmhs-types` embeds files from `spec/`.
