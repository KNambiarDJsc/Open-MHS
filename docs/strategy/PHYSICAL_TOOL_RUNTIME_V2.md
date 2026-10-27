# Physical tool runtime v2

How OpenMHS should turn devices into tools for agents, building on what exists. The goal
is that a new device becomes usable by any MCP or Python agent without that agent, or the
runtime's core, learning anything about the device's protocol.

## 1. Where v0.1 already is

The current design gets the hard part right: **agents never see protocols.** The MCP
server (`crates/openmhs-mcp/src/lib.rs`) has eleven generic tools (`list_devices`,
`describe_device`, `read_state`, `check_command`, `submit_command`, `get_command`,
`list_pending_approvals`, `reconcile_command`, `cancel_command`, plus four plan tools).
A capability is addressed as `(device, capability)` with a typed value checked against the
descriptor. Adding a device adds data, not tools.

That is deliberate, and v2 keeps it. The alternative — one MCP tool per capability, which
most projects in `docs/research/landscape.md` do — gives a model a nicer-looking tool list
but puts the safety contract in tool descriptions, loses the uniform lifecycle, and makes
tool lists unbounded in a lab with many devices.

## 2. Capability-to-tool registry (projection, not generation)

**Proposal:** keep generic tools as the contract, and add an optional *projection* that
renders capabilities as named tools for hosts that work better with them:

```text
tool name:   psu_01__set_voltage            (derived: device id + capability name)
input:       { "value": { "type": "number", "unit": "V", "minimum": 0, "maximum": 12 } }
calls:       submit_command(device="sim.psu-01", capability="power.output.voltage.setpoint", ...)
```

- Generated from the descriptor at `list_tools` time, filtered to what the credential may do
  (gap A1), capped (e.g. 64 tools), and off by default (`openmhs mcp --project-tools`).
- Every projected tool is a thin alias: the same gateway call, the same checks, the same
  receipts. It adds no permission and no path.
- Descriptor free text stays quoted as untrusted (`crates/openmhs-mcp/src/untrusted.rs`).
- JSON Schema for inputs comes from the descriptor's value spec, with the unit fixed, so a
  model cannot send millivolts to a volts input.

## 3. Dynamic discovery

Today devices are registered when the gateway starts. v2 adds three things, in this order:

1. **Availability (built on this branch).** Every device has `ONLINE`, `DEGRADED` or
   `OFFLINE` with a reason and times, a `busy` flag and its active lease
   (`devices.status`, MCP `list_devices`, CLI `devices status`, SDK `device_status`).
   A device that is off at start is registered `OFFLINE` and comes online when it answers.
   See ADR-0019.
2. **Change notification.** `devices.watch` (long-poll) on the API; MCP
   `notifications/tools/list_changed` and a `devices` resource with subscriptions once the
   MCP server has a transport that can push (gap A3).
3. **Hot add and remove**, operator only: `devices.add {profile | manifest}` and
   `devices.remove {id}`. Removing a device with commands in flight is refused; a removed
   device's journal stays. The descriptor hash is pinned at add time and audited, so a
   changed descriptor is a new registration, never a silent change.

Network discovery (mDNS, Modbus scans, MQTT topic sniffing) is **out of scope** for the
runtime: discovering that something answers is not knowing what it does. Discovery tools
may *propose* a profile for an operator to review.

## 4. Context-aware operation

What an agent needs to see before acting, and where it comes from:

| Context | Source | Status |
|---|---|---|
| Availability, reason, since | Health monitor and passive results | Built here |
| Busy (a command in progress) | Device lock | Built here |
| Leased, by whom, until when | Leases | Built here (in status) |
| Operating mode (local/remote, armed, fault) | A descriptor `mode` capability, read like any other | Descriptor convention; document it |
| Prerequisites | Preconditions already in the descriptor, evaluated by `check_command` | Exists |
| Permitted operations | Scopes ∩ grants | Gap A1: `capabilities.allowed` |
| Unresolved outcomes on a capability | Journal | Exists as a refusal; add to status |

## 5. Multi-agent coordination

Leases (ADR-0018) already coordinate: all-or-nothing, through `contains` and
`shares_resource`, visible to every caller, released when a credential is revoked. v2
adds only:

- `busy` and `lease` in device status (built here), so an agent sees contention before it
  is refused;
- lease queueing (`leases.acquire {wait_ms}`) so two agents do not poll each other;
- a per-device "who has commands in flight" view for operators.

## 6. Multi-device workflows

Plans (M9) already run ordered steps with halt, compensate or ask, twin preflight and
crash recovery into `AWAITING_DECISION`. v2:

- Parallel steps where the graph shows no shared resource.
- Plans stop early with `device.unavailable` when a step's device goes `OFFLINE`, instead
  of waiting for a timeout (falls out of this branch's gating).
- Resumable plans after restart, with a person's decision (already the rule) shown with the
  evidence that decision needs.

## 7. Execution evidence

Receipts carry the request, policy decision and rules, approval, adapter report and
readback (expected vs observed, with the reading's timestamp, quality and source). Gaps:

- Readback freshness relative to dispatch (gap H4).
- Signed audit export (threat model, not built): an Ed25519 signature over a range of the
  hash chain, so evidence can leave the machine and still be checked.
- Device-observation provenance: `source` is `device`, `simulator` or `twin` today; add the
  adapter version and descriptor hash to every receipt so a report can say exactly what
  code produced it.

## 8. Human oversight

What exists is the right shape: approvals bound to a command fingerprint, single use,
self-approval refused, agents cannot approve or resolve. To keep routine work light:

- Approvals are by side-effect class: `hazardous` needs approval by default, `reversible`
  does not. Keep that default.
- Standing approvals: "approve this exact plan up to N times in the next hour", still bound
  to fingerprints. Avoids approval fatigue without a blanket allow.
- Operator intervention: `devices.hold {id}` puts a device in maintenance: every non-stop
  command refused with `device.held`, shown in status. It is the operator's version of
  `OFFLINE`, and it is the obvious next step after availability.

## 9. Fleet management (evaluate, do not build yet)

Multi-gateway inventory, policy distribution and fleet health are what a commercial
deployment would want. They should wait until at least three independent sites run single
gateways, because the single-gateway trust model (ADR-0001) would have to change: who signs
policy, how revocation propagates, how an operator on one site approves a command on
another. Building that before anyone runs one gateway would be guessing.

What *is* worth doing now, because fleet tooling would need it anyway: OpenTelemetry export
of command transitions and device availability (one span per command; a gauge per device).
