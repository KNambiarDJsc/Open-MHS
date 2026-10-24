# Experiment 02: an agent and a simulated ESP32 over MQTT

Can OpenMHS be tested against a device that *changes state*, with commands, telemetry and
acknowledgements, without owning one? This experiment runs real ESP-IDF firmware for an ESP32 in
Espressif's QEMU, beside a local Mosquitto broker, and puts an OpenMHS gateway in front of it
using the stock [MQTT profile](../../adapters/mqtt.md). An agent switches an LED output and
counts pulses; the node is frozen, powered off and cut off from its broker.

[Results](REPORT.md) · [Firmware and harness](../../../firmware/esp32-mqtt-node) · [Transcript](evidence/demo-transcript.txt)

> **Simulated, not physical.** The firmware is real and compiled for the ESP32, but the chip, its
> Ethernet and its GPIO are emulated. Nothing here says anything about a physical ESP32, Wi-Fi,
> a real board's timing and power, wiring, or whatever an output would drive. The "LED" is a
> GPIO level with nothing attached. No model and no paid API are used, and no account was needed:
> everything runs on loopback.

## Why QEMU and not Wokwi

The request was to use Wokwi's ESP32 simulator. Wokwi's cloud simulation can reach a broker on
this machine only through its paid *Private IoT Gateway*; the free alternative is a public
broker, which would put commands for the device on the open internet. The maintainer chose a
local QEMU ESP32 and Mosquitto instead. Wokwi was not used and is not claimed. The firmware does
not depend on the simulator: it builds for a board by switching `protocol_examples_common` to
Wi-Fi, and the broker address and topic prefix are Kconfig options.

## What it validates, and what it does not

| Validated (real firmware in QEMU, real Mosquitto, the gateway's stock MQTT adapter) | Not validated |
|---|---|
| A state-changing command is delivered, acknowledged by the node and **verified by reading the state back**: `VERIFIED` | A physical ESP32, any real board, USB or Wi-Fi |
| An action with no readback is sent once and reported `UNVERIFIABLE`, never `VERIFIED` | Wi-Fi association, radio loss, signal strength, roaming |
| No grant, or a value outside the declared range, means **no message reaches the node** (counted on the broker) | What a real GPIO drives; electrical behaviour; actuators; anything safety-related |
| A node that does not answer (frozen, or powered off) yields `UNKNOWN_OUTCOME` / `adapter.timeout`; the command is **not retried** | The SNTP clock path (the tests use a clock sent over MQTT, see the report) |
| A write to a frozen node that the node applied after release **reconciles to `RECONCILED_APPLIED`** by readback; an action without readback is `command.not_reconcilable` | A flapping network link: QEMU's link toggling did not recover reliably, so it was not tested |
| A hung node's readings age out as `STALE` and are not usable; a rebooted node is visible in its uptime and counter | Brokers with authentication, TLS, ACLs or QoS 2; the profile uses an anonymous loopback broker |
| Broker down: reads fail `adapter.disconnected`, a command is `FAILED` `adapter.not_sent`, and the gateway recovers | Long soak runs, many nodes, retained-message races at scale |
| The audit hash chain verifies after all of the above | Other platforms than Windows 11 x86_64; other ESP-IDF or QEMU versions |

## What was built

* [`firmware/esp32-mqtt-node/`](../../../firmware/esp32-mqtt-node): the firmware (C, ESP-IDF
  v5.5), the OpenMHS MQTT profile for it, a harness that runs Mosquitto, a clock source and the
  node under QEMU, 18 tests and a demo.
* No change to the OpenMHS core, its schemas, policy or tests. No registry entry or conformance
  report: the MQTT profile has no simulator block, so `openmhs conformance --profile` does not
  apply, and the compatibility matrix is unchanged (it has no physically verified row either).

## Reproduce it

You need ESP-IDF v5.5.x, Espressif's QEMU, Mosquitto, Python 3.9+ and a built `openmhs`. The
steps, with the build commands and environment variables, are in the
[firmware README](../../../firmware/esp32-mqtt-node/README.md). In short:

```sh
python firmware/esp32-mqtt-node/test_node.py -v                       # 18 tests, about two minutes
python firmware/esp32-mqtt-node/demo.py --bin target/debug/openmhs    # the walkthrough
```

Without the environment the tests skip with the reason. Everything binds to `127.0.0.1` and is
removed afterwards.
