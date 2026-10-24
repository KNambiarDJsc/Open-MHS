# ESP32 MQTT node (simulated)

Firmware for an ESP32 that speaks the OpenMHS [MQTT profile](../../docs/adapters/mqtt.md), and
a harness that runs it in Espressif's QEMU next to a local Mosquitto and the OpenMHS gateway.
It exists to test the gateway against a state-changing device without owning one.
Results and limits: [Experiment 02](../../docs/experiments/02-esp32-qemu-mqtt/README.md).

**This is a simulation.** The firmware is real ESP-IDF code, but it runs on an emulated chip.
Nothing here has run on a physical ESP32, and the LED output is a GPIO level with nothing
attached. Do not read a passing run as a hardware validation.

## What the node does

| Topic (prefix `esp32/node-01`) | Direction | Payload |
|---|---|---|
| `state` | node → broker, retained, QoS 1, every second and after each command | `{"led":false,"pulses":0,"uptime_s":12.3,"time":"…Z"}` |
| `set` | broker → node | `{"led":true,"id":"…"}` |
| `pulse` | broker → node | `{"n":1..10,"id":"…"}` (adds `n` to a counter; not idempotent) |
| `ack` | node → broker, QoS 0 | `{"id":"…","ok":true}` or `{"id":"…","ok":false,"error":"…"}` |
| `hello`, `clock` | node → broker, broker → node | clock bootstrap, see below |

A message that is not JSON or has no string `id` is ignored: there is no one to acknowledge to.
The same `id` sent twice counts twice; the node does not deduplicate (the gateway never retries,
so it does not need to).

`profile.json` is the gateway side: it declares `led.on` (writable, verified by readback),
`node.uptime`, `led.pulses` and `led.pulse` (an action with no readback).

### Clock

State is published only once the node's clock is set, so every message carries the node's own
time and the gateway can judge staleness from the device, not from when it happened to arrive.
The default build uses SNTP. Under QEMU on Windows the host's UDP port 123 belongs to the
Windows Time service, so the test build sets `CONFIG_NODE_CLOCK_FROM_MQTT=y` (in
`sdkconfig.defaults`): the node publishes `hello` and the harness answers on `clock` with the host
time. **The SNTP path is therefore not exercised by these tests.**

## Requirements

None of these are bundled or installed by OpenMHS.

* ESP-IDF v5.5.x (built and run with v5.5.5) and Espressif's QEMU (`qemu-system-xtensa`, 9.2.2).
* Mosquitto (`mosquitto`, `mosquitto_pub`, `mosquitto_sub`) on `PATH`, or in `OPENMHS_MOSQUITTO_DIR`.
* Python 3.9 or later; the harness uses only the standard library.
* An `openmhs` binary (`cargo build -p openmhs-cli`, or a release).

The broker listens on `127.0.0.1:18830` only. The emulated node reaches it as `10.0.2.2:18830`
(QEMU user-mode networking), so nothing is exposed to the network.

## Build

From an ESP-IDF shell (`export.ps1` / `export.sh` sourced):

```sh
cd firmware/esp32-mqtt-node
idf.py -B ../../target/esp32-node build        # any build directory outside the source tree
cd ../../target/esp32-node
python -m esptool --chip esp32 merge_bin --fill-flash-size 4MB -o flash.bin @flash_args
```

`flash.bin` is a 4 MB image QEMU boots directly.

## Run

```sh
export OPENMHS_QEMU=/path/to/qemu-system-xtensa
export OPENMHS_FLASH_IMAGE=/path/to/flash.bin
export OPENMHS_BIN=/path/to/openmhs

python test_node.py -v            # 18 tests, about two minutes
python demo.py --bin "$OPENMHS_BIN" --out transcript.txt
```

Without the environment the tests skip rather than fail, with the reason. `OPENMHS_KEEP_LOGS=DIR`
copies the node's serial log, the gateway log and Mosquitto's log to `DIR` after the gateway tests.

The tests start Mosquitto, the QEMU node and a gateway in a temporary directory and remove them
afterwards. Stop other Mosquitto instances using port 18830 first.

## Files

| File | |
|---|---|
| `main/main.c` | the firmware |
| `main/Kconfig.projbuild`, `sdkconfig.defaults` | broker URI, topic prefix, clock source, LED pin |
| `profile.json` | the OpenMHS MQTT profile for this node |
| `harness.py` | Mosquitto, the clock source and the QEMU node, as Python classes |
| `test_node.py` | firmware tests, then the numbered tests through the gateway |
| `demo.py` | the walkthrough in the experiment report |

## What QEMU cannot show

* Wi-Fi, a radio, signal loss, or the real board's timing, power and brown-outs.
* What a real GPIO drives. The LED level is set; nothing observes it.
* A network link flapping: QEMU's `set_link` did not recover reliably, so those tests were
  replaced by powering the node off, freezing it (`stop`/`cont`) and stopping the broker.
* Freezing the machine also freezes the guest clock, so after a hang the harness re-sends the time
  and the firmware steps its clock. A real board that hangs keeps (some) time.
