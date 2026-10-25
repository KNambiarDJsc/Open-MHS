# Connect your own device

From nothing to an AI agent operating a device you own, in six steps. Every step works
against a built-in simulator first, so you can try the whole path before the hardware is
on the bench.

> OpenMHS is not a safety system. Physical interlocks, emergency stops and limits enforced
> by the device itself keep final authority. Start with something harmless (an LED, a
> relay with nothing attached) and read [Trying real hardware](hardware/README.md).

## How it fits together

```text
 AI agent ──MCP──┐
 script ──Python─┼──► OpenMHS gateway ──profile or adapter──► your device
 you ──────CLI───┘    (grants, limits, audit)
```

The gateway is one program, `openmhs`. It knows your device from a **profile** (a JSON
file) or an **adapter** (a small program). Nothing reaches the device unless a **grant**
allows it, and every request is checked against the device's declared units, ranges and
limits and written to a hash-chained audit log.

## 1. Install

Until the first release is published, build from source (Rust 1.89 or newer from
[rustup](https://rustup.rs)):

```sh
git clone https://github.com/KNambiarDJsc/Open-MHS
cd Open-MHS
cargo install --locked --path crates/openmhs-cli
openmhs --version
```

Other ways, and platforms: [Installation](installation.md).

## 2. Pick how OpenMHS talks to your device

| Your device | Use | Start with |
|---|---|---|
| Speaks **Modbus TCP** (PLCs, meters, drives, chambers) | a profile | `openmhs profile new modbus` |
| Speaks **MQTT** (ESP32, Raspberry Pi, IoT nodes) | a profile | `openmhs profile new mqtt` |
| Speaks **SCPI** over TCP or serial (power supplies, meters, scopes) | a profile | `openmhs profile new scpi` |
| Serves a **JSON HTTP API** (your own firmware, a small web service) | a profile | `openmhs profile new http` |
| Has its own API or SDK (for example [OctoPrint](adapters/octoprint.md)) | a Python adapter | [Python adapter SDK](adapters/python-sdk.md) |
| Needs Rust, or a protocol not listed | a Rust adapter | `openmhs adapter new my-device` |
| OPC UA server, ROS 2 topics | the bundled adapters (read-only, observe-only) | [OPC UA](adapters/opcua.md), [ROS 2](adapters/ros2.md) |

A bare microcontroller has to run firmware that speaks one of these first.
[`firmware/esp32-mqtt-node`](../firmware/esp32-mqtt-node) is a complete ESP32 example
with its profile; [ESP32 LED](hardware/esp32-led.md) is a smaller HTTP one.

The rest of this page uses a profile. Adapters follow the same steps from 4 on.

## 3. Describe the device

```text
$ openmhs profile new mqtt --id lab.node-01
Created lab.node-01.json: mqtt profile for device lab.node-01, copied from the greenhouse node example (6 capabilities).

Next:
  1. Edit it: "connection" is where your device is; "descriptor"."capabilities" and "mapping"
     are what it can do and how. Reference: docs/adapters/mqtt.md
  2. openmhs profile check lab.node-01.json
  3. openmhs serve --profile-sim lab.node-01.json    try it against the profile's simulator
  4. openmhs serve --profile lab.node-01.json        connect to the real device
```

The file starts as a working example. Edit three parts:

* **`connection`**: where the device is (host and port, broker and topics, serial port).
* **`descriptor.capabilities`**: what it can do. For each value, its type, **unit** and
  **range**; whether it can be written; how risky a change is (`side_effect`); how to confirm
  a write (`verification: readback`); how old a reading may be (`freshness`). These are what
  the gateway enforces, so be strict.
* **`mapping`**: how each capability maps onto the protocol (register, topic, command, URL).

Field reference: [Device profiles](adapters/profiles.md), then the page for your protocol
([Modbus TCP](adapters/modbus.md), [MQTT](adapters/mqtt.md), [SCPI](adapters/scpi.md),
[HTTP](adapters/http.md)). Check it as you go; this connects to nothing:

```text
$ openmhs profile check lab.node-01.json
ok       lab.node-01.json: mqtt device lab.node-01, 6 capabilities
```

## 4. Start the gateway

Try the profile against its built-in simulator first, then the real device:

```sh
openmhs serve --profile-sim lab.node-01.json     # simulated: safe to experiment
openmhs serve --profile lab.node-01.json         # the real device at "connection"
```

```text
OpenMHS gateway listening on http://127.0.0.1:7400
  device   lab.node-01 (openmhs.mqtt, simulated mqtt at 127.0.0.1:49962)
  access   deny by default; `openmhs grants list` shows what is allowed (docs/connect-a-device.md)
Press Ctrl+C to stop.
```

Leave it running and use a second terminal. `openmhs devices list` and
`openmhs devices inspect lab.node-01` show what the gateway understood from your file.

## 5. Decide what is allowed

Nothing is allowed at first, not even for you:

```text
$ openmhs read lab.node-01 air.temperature
error [policy.denied.no_grant]: no grant allows 'operator:local' to read 'air.temperature' on 'lab.node-01'
```

Give an agent exactly what it needs. This grant allows reads and writes but not actions
(`invoke`), so the irreversible `irrigation.dose` stays out of reach; and
`--max-side-effect reversible` refuses any write the profile marks as irreversible or
hazardous:

```sh
openmhs grants add --id agent-node --principal agent:demo --device lab.node-01 \
  --capability '*' --ops read,write --effect allow --max-side-effect reversible --reason "first device"
```

Use `--effect require_approval` instead of `allow` when a person should confirm each
command (`openmhs approve <id>`).

Then issue the agent a credential. Its scopes are a second ceiling: it can do nothing the
grants do not also allow. The token is printed once:

```text
$ openmhs tokens create agent:demo --scope 'lab.node-01:*:read' --scope 'lab.node-01:*:write' --ttl 7d
created tok_c6634d5dd1edd058 for agent:demo (agent)
scope: lab.node-01 / * [read]
scope: lab.node-01 / * [write]
expires: 2026-11-01T11:15:46.797Z
token: omhs_…
This is shown once. Pass it with --token or OPENMHS_TOKEN.
```

## 6. Connect an agent, a script or yourself

All three use the same token and go through the same checks.

**From the command line** (set `OPENMHS_TOKEN` to act as the agent):

```text
$ openmhs read lab.node-01 air.temperature
lab.node-01 air.temperature = 24.5 Cel  (GOOD, simulator, 145 ms old)

$ openmhs exec lab.node-01 fan.on --value true
state    VERIFIED  -> succeeded (confirmed by readback)
verify   readback fan.on  expected true  observed true  (GOOD, simulator)  MATCH

$ openmhs exec lab.node-01 vent.position --value 140%
state    DENIED  -> denied (nothing sent to the device)
  denied command.value.out_of_range: 140 % is outside the allowed range 0..100 %
```

**From an AI agent over MCP.** Register the gateway's MCP server with your host. For Claude
Code:

```sh
claude mcp add openmhs -e OPENMHS_TOKEN=<token> -- openmhs mcp
```

Any other MCP host: a stdio server, command `openmhs`, argument `mcp`, environment
`OPENMHS_TOKEN`. The agent then has tools to list and describe devices, read, check and
submit commands, and reconcile uncertain ones. Details and the full tool list:
[Connecting an AI agent](mcp.md). Never give the MCP server the operator token.

**From Python** (the client has no dependencies;
[install it from the checkout](installation.md#the-python-client) until it is on PyPI):

```python
from openmhs import Client, Quantity

gw = Client.from_home(token="<agent token>")       # finds the running gateway
r = gw.read("lab.node-01", "air.temperature")
if r.usable:                                       # fresh enough to act on
    print(r.value)
receipt = gw.write("lab.node-01", "vent.position", Quantity(40, "%"))
print(receipt.state)                               # VERIFIED: read back and matched
```

More: [Python SDK](../python/README.md).

## Check what happened

As the operator (no `OPENMHS_TOKEN`):

```sh
openmhs commands list              # every command, its state and why
openmhs audit tail                 # the latest audit records
openmhs audit verify               # audit log intact: N records, hash chain verified
```

## When something says no

| You see | It means | Do |
|---|---|---|
| `policy.denied.no_grant` | No grant covers this principal, device, capability and operation | `openmhs grants add …` |
| `auth.scope.denied` | The grant allows it but the token's scopes do not | Create a token with that scope |
| `command.value.type_mismatch` with a `hint` | A bare number where a unit is required; units are never guessed | `--value 40%` or `--value 40 --unit %` |
| `command.value.out_of_range`, `command.slew_exceeded` | Outside the limits your profile declares | Working as intended; change the request or the profile |
| `STALE … not usable for decisions` | The last reading is older than `freshness.max_age_ms` | Check the device is publishing; readings carry the device's own time |
| `UNKNOWN_OUTCOME` | The command was sent but no answer came; it may or may not have happened. It is never retried | `openmhs commands reconcile <id>` reads the device to settle it |
| `serve --profile` stops with `error: engine: adapter: …` | The gateway connects to each device before it starts, and this one did not answer as the profile says (the message names it) | Check `connection` and that the device is on; `--profile-sim` runs without it |
| `adapter.disconnected`, `adapter.not_sent` | The gateway cannot reach the device or broker | Check `connection`; nothing was sent |
| `gateway.forbidden` | An agent token tried an operator action | Grants, tokens and approvals need the operator |

More: [Troubleshooting](troubleshooting.md).

## Where next

* Prove your profile or adapter behaves: [Conformance testing](../spec/conformance.md).
* Several devices that interact: [leases and plans](../spec/plans.md).
* Sharing it: [the adapter registry](../registry/README.md).
