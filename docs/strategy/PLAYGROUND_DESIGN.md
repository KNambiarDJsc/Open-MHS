# OpenMHS Playground

**Goal:** anyone can watch an AI agent use hardware, hit the limits, cause faults and read
the evidence, without buying equipment, in under five minutes on their own machine.

**Non-goals:** a public website that runs visitors' code; multi-tenant simulator hosting;
re-creating physical machines with physics. Public arbitrary-code execution is a security
product of its own and is out of scope.

## 1. What already exists

Most of a local playground is already in the repository and tested:

| Piece | Where | Notes |
|---|---|---|
| Virtual lab: thermometer, power supply, linear stage | `crates/openmhs-sim` | Seeded noise, 13 fault kinds, device-specific faults (`overheat`, `trip`, `jam`) |
| Fault injection API | `sim.fault` (operator only, `serve --sim` only), `openmhs sim fault` | `docs/simulators.md` |
| Guided tour | `openmhs demo` (`crates/openmhs-cli/src/demo.rs`) | Runs a real gateway over its API and checks each outcome |
| Protocol simulators | Modbus, MQTT, SCPI, HTTP sims behind `serve --profile-sim` | Real protocol code, simulated peer |
| External simulators | OctoPrint Virtual Printer (Experiment 01), ESP32 in Espressif QEMU with Mosquitto (Experiment 02) | Not in CI (ESP32); OctoPrint is |
| Scripted agent | `examples/agent-loop/mock_agent.py` (no model, no key) | And `llm_agent.py` with the user's own key |
| Evidence | Receipts, `openmhs audit verify`, `audit.tail` | JSON only |

What is missing is packaging and two views: a single command that sets everything up, and
a timeline of what happened.

## 2. Simulator connector vs protocol adapter

They are different things and must stay different:

| | Protocol adapter | Simulator connector |
|---|---|---|
| Purpose | Talk to a device, real or simulated, over its real protocol | Start, reset, fault and stop a simulator |
| Called by | The engine, after authorization | The playground, as the operator |
| Examples | `openmhs-modbus`, `octoprint_adapter.py` | `ScpiSim::serve_tcp`, the OctoPrint harness that starts Virtual Printer, the QEMU launcher |
| Sees agents' commands | Yes | Never |
| Ships to production | Yes | Never |

A simulator connector is test infrastructure. The adapter that talks to a simulated
OctoPrint is the same adapter that would talk to a real one; only the connector differs.
That is what makes simulator results meaningful at all, and it is why integration-tested
rows in the compatibility matrix mean "real protocol, simulated peer".

Connector interface (proposed, Python and Rust):

```text
start(config) -> endpoint          # returns what the adapter's profile points at
health() -> ready | starting | failed
reset()                            # back to the seeded initial state
faults() -> [name]                 # what this simulator can inject
inject(name, args)                 # operator-only, audited as sim.fault
stop()
```

The in-process virtual lab already implements this shape as `SimControl`
(`crates/openmhs-gateway/src/lib.rs`, the `sims` map).

## 3. The local playground

```text
openmhs playground [--with octoprint|esp32|modbus|mqtt|scpi|http]
```

One command that:

1. Starts a gateway in a throwaway home (`--home` under the OS temp directory), with the
   virtual lab and any `--with` simulators.
2. Creates a principal `agent:playground`, a grant for the simulated devices only (read and
   write, with the hazardous stage move held for approval so the approval flow is visible),
   and a scoped token, and prints the MCP configuration with that token.
3. Prints three things to try, each one command.
4. On Ctrl+C, stops everything and deletes the home unless `--keep`.

It never connects to a non-simulated device: it refuses `--profile` and any manifest
without `simulate: true`. That is the safety property that makes it safe to recommend.

### Scenarios (each a script under `examples/playground/`)

| Scenario | Shows | Built from |
|---|---|---|
| `limits` | Slew refusal, range refusal, ramp, readback | The current quickstart |
| `lost-ack` | `apply-then-hang` → `UNKNOWN_OUTCOME` → blocked resend → reconcile | `docs/simulators.md` |
| `offline` | `link-down` → `OFFLINE` in `list_devices` → `device.unavailable` refusal → `clear` → back `ONLINE` | **New in this branch** (availability) |
| `competing-agents` | Two agents, one lease; the second is refused with the holder's name | `leases_http.rs` |
| `approval` | A held move, a human approves, the move runs once | `approvals_http.rs` |
| `restart` | Kill the gateway mid-command; restart; the command is `UNKNOWN_OUTCOME`; reconcile | `scenarios.rs` |
| `workcell` | Plan with compensation when the stage jams | `examples/workcell` |

Each scenario is a script that asserts what it expects, so scenarios are also tests.

### Execution timeline

`openmhs commands show <id> --timeline` prints the command's audit records as a vertical
timeline: created, validated (with rules), approved (by whom), dispatched, accepted,
applied, verified (expected vs observed), with times relative to creation. The data is
already in the journal (`command.transition` records); this is presentation only.

### Reset and recovery

`openmhs sim reset <device>` (operator) puts a simulator back to its seed state, audited.
For external simulators, the connector's `reset()`. The journal is not reset: evidence of
what happened before the reset stays, which is the point.

## 4. External simulators

| Simulator | Fit | Plan |
|---|---|---|
| OctoPrint Virtual Printer | Good, already tested (L1, read-only) | Package its harness as a connector; keep the adapter read-only until a write path has a validation plan |
| ESP32 in Espressif QEMU + Mosquitto | Good, already tested (18 tests, state-changing) | Connector behind `--with esp32`; needs ESP-IDF and QEMU, so optional and documented, not default |
| Wokwi | Poor for local use | Its cloud ESP32 reaches a local broker only through a paid gateway (Experiment 02 README) |
| SCPI instrument simulators | In-repo `ScpiSim` | Independent ones (PyVISA-sim) would test the adapter against someone else's idea of SCPI, which is more convincing. Add as a second peer |
| Modbus simulators | In-repo sim | Add `pymodbus` server as an independent second peer, same reason |
| MQTT devices | Mosquitto + in-repo device sim, and the ESP32 node | Enough |
| ROS 2 / Gazebo | ROS 2 adapter is observation-only; CI uses a simulated publisher in a Jazzy container | Gazebo is heavy (GPU, gigabytes). A TurtleBot3 Gazebo scenario is worth it only after supervisory actions exist |

The rule from the compatibility matrix applies: a simulator result is reported as
simulated, never as validated.

## 5. Hosted playground (later, carefully)

A hosted demo is valuable for the 30-second impression, and dangerous if done naively. If
ever built:

- Read-only replay of recorded sessions first (a terminal recording, the timeline), no
  execution. This gets most of the value.
- If live: one ephemeral VM per visitor, no network egress, the virtual lab only, a fixed
  scripted agent, a 10-minute lifetime. No visitor-supplied code, no visitor-supplied
  descriptors or profiles.
- Never real devices.

## 6. Proof of concept in this branch

The `offline` scenario works end to end on this branch with existing commands. The
transcript below comes from `crates/openmhs-gateway/tests/availability.rs`, which runs the
same steps automatically:

```sh
openmhs serve --sim                                    # terminal 1
openmhs sim fault sim.psu-01 link-down                 # terminal 2, operator
openmhs devices status                                 # sim.psu-01 OFFLINE (simulated disconnect)
openmhs exec sim.psu-01 power.output.voltage.setpoint --value 2V
                                                       # DENIED device.unavailable; nothing sent
openmhs sim fault sim.psu-01 clear
openmhs devices status                                 # sim.psu-01 ONLINE
```

`openmhs playground` itself is not built; it is the first item of milestone M11 in
[NEXT_MILESTONE_ROADMAP.md](NEXT_MILESTONE_ROADMAP.md).
