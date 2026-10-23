# Real hardware

Everything in this repository is tested against simulators: over real sockets, with the
real protocol code, but never against a physical device. Nothing here has been verified
on hardware by the project. This is a pre-alpha project, and OpenMHS is not a safety
system: see the [README](../../README.md).

If you try a real device:

1. **Start with something that cannot hurt anyone**: an LED, a relay with nothing
   attached, a sensor. Not mains voltage, motors, heaters, valves, or anything that
   moves or gets hot.
2. **Write the profile and test it against its simulator first**:
   `openmhs profile check` and `openmhs conformance --profile`. The suite injects faults
   and sends many commands. It only ever runs against the simulator, never the device.
3. **Talk to the device directly before the gateway does**: read a value with `curl`,
   `mosquitto_sub`, a Modbus tool or a terminal, and compare it with what you expect.
4. **Then** run `openmhs serve --profile device.json`. Grant reads first; grant writes
   one capability at a time, and require approval (`effect: require_approval`) for
   anything that acts.
5. Keep the hardware's own protections: fuses, current limits, interlocks, an emergency
   stop. OpenMHS must never be the only thing between a command and harm.

## Guides

| Guide | Hardware | Protocol |
|---|---|---|
| [esp32-led.md](esp32-led.md) | ESP32 board, one LED | HTTP |

These guides are **unverified**: written to match the example profiles, but not built,
flashed or run by the project. Corrections from people who try them are welcome.
