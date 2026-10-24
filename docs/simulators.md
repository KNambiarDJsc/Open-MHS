# Simulators and fault injection

You can try every part of OpenMHS without hardware. There are two kinds of simulator.

## The virtual lab

`openmhs serve --sim` (and `openmhs demo`) registers three in-process devices:

| Device | What it is | Conformance |
|---|---|---|
| `sim.thermo-01` | Ambient temperature sensor with noise | L1 |
| `sim.psu-01` | Single-channel programmable power supply: 0–12 V, 0–3 A, slew limit, protection trip | L3 |
| `sim.axis-01` | Single-axis linear stage: homing, absolute moves | L3 |

State resets when the gateway restarts, and noise is seeded (`--seed`, default 42), so
runs repeat. These are simulations of *plausible* devices to exercise the contract. They
are not models of any real product.

## Protocol simulators

Each protocol adapter has a simulator that speaks the real protocol over loopback
sockets: Modbus TCP, an MQTT broker, a SCPI instrument, an HTTP device. Serve a profile's
simulated device instead of the real one:

```sh
openmhs serve --profile-sim adapters/openmhs-modbus/profiles/climate-chamber.json \
              --profile-sim adapters/openmhs-mqtt/profiles/greenhouse-node.json
```

`--profile` (without `-sim`) connects to the real device the profile names. Do not use it
unless you mean to; read [Trying real hardware](hardware/README.md) first.

## Injecting faults

As the operator, break a simulated device on purpose with `openmhs sim fault <device>
<fault>`. Faults apply to the virtual lab and to protocol simulators served by this
gateway.

| Fault | Effect |
|---|---|
| `link-down` / `link-up` | Reads and commands fail with a disconnect, then recover |
| `stale=<ms>` | Readings are timestamped that many milliseconds in the past |
| `bad-quality` | Readings are flagged bad |
| `readback-offset=<x>` | Readback differs from what was written |
| `latency=<ms>` | Every call is that much slower |
| `hang` | Commands are never answered and nothing is applied (a lost command) |
| `apply-then-hang` | The command is applied, then the answer is lost: `UNKNOWN_OUTCOME` |
| `apply-then-malformed` | Applied, then a garbled answer |
| `reject` | The device refuses commands |
| `disconnect` | Commands fail with a disconnect before anything is sent |
| `crash` | The adapter panics while handling the command |
| `clear` | Remove all faults |
| device-specific | `overheat` (thermometer), `trip` (supply), `jam` (stage) |

For example, to see a lost acknowledgement and then resolve it:

```sh
openmhs sim fault sim.psu-01 apply-then-hang
openmhs exec sim.psu-01 power.output.voltage.setpoint --value 2V --deadline-ms 3000   # UNKNOWN_OUTCOME
openmhs sim fault sim.psu-01 clear
openmhs commands reconcile <command id>                                               # reads the device back
```

`openmhs sim faults` lists the fault names. The guided tour (`openmhs demo`)
replays these scenarios and checks the outcomes.

## Simulation is not validation

A simulator passing says the contract and the adapter agree with each other. It says
nothing about a particular real device. The [compatibility matrix](compatibility.md)
labels every row accordingly, and no row is "physically validated".
