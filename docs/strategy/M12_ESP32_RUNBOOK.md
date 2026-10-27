# M12 runbook: bounded, independently observed ESP32 test

A procedure for the first physical validation, **not yet run**. It turns
[HARDWARE_VALIDATION_PLAN.md](HARDWARE_VALIDATION_PLAN.md) into steps for one board. Nothing
here has been tried on a physical ESP32; the firmware has only run in QEMU
([Experiment 02](../experiments/02-esp32-qemu-mqtt/README.md)). Read it once end to end
before starting, and change it where reality disagrees, recording what changed.

## 1. Bounds

This test may only do what is harmless by construction. If any condition below is false, stop.

- **The only actuation is one GPIO output at 3.3 V, driving an LED through a resistor sized for
  the board's pin limit (at most 10 mA).** No relay, motor, heater, mains, battery pack or
  anything that can hurt a person or the board. The `led.pulse` counter is a software counter
  and drives nothing.
- The board is powered from a USB port or a bench supply limited to 5 V and 500 mA.
- A person is present for the entire run and can pull the USB cable. Pulling it is the stop.
- The OpenMHS gateway is not a safety function and is not relied on for one in this test.
- The board is on an isolated bench network (a router or hotspot with no route to anything
  important); the broker listens only on that network.
- No native protection is removed, bypassed or modified, and the firmware is the project's test
  fixture, not production firmware.

## 2. Equipment

| Item | Why | Record |
|---|---|---|
| ESP32 dev board | Device under test | Exact module, board revision, MAC |
| USB cable and host | Power, flashing, serial console | OS, ESP-IDF version (built with v5.5.x) |
| LED, resistor, breadboard | The only load | LED colour, resistor value |
| Multimeter (calibrated, date noted) | Independent measurement of the GPIO level and of the supply | Make, model, calibration date |
| Logic analyzer or scope (optional) | Independent timing of the pulse | Make, model |
| A phone camera | Timestamped evidence of the LED and of the meter | — |
| Wi-Fi access point on the isolated network, a second machine as broker host | The real network path | SSID band, channel |
| Mosquitto on the broker host | The MQTT broker | Version, config |
| `openmhs` built from a **tagged commit** | The runtime | `openmhs --version`, commit hash |

## 3. Setup

1. Build and flash `firmware/esp32-mqtt-node` per its README, **with SNTP on**
   (`CONFIG_NODE_CLOCK_FROM_MQTT` off): a real board should take its clock from NTP, and the
   test must exercise that path, which QEMU did not. Record the build configuration.
2. Point the node's Wi-Fi and broker settings at the bench network.
3. Run Mosquitto on the broker host; confirm the node's retained `esp32/node-01/state` appears.
4. Measure the clock offset between the node and the gateway host for 5 minutes (compare the
   `time` field of each state message with the host clock on receipt). Record min, max and
   median. **The result sets `--readback-skew-ms`**: use the measured maximum offset rounded
   up, not a convenient large number, and report it. If it is in seconds, fix the clock source
   first.
5. Start the gateway: `openmhs serve --profile firmware/esp32-mqtt-node/profile.json
   --readback-skew-ms <measured> --journal-max-mib 64 --journal-reserve-mib 8` (the quota is
   small on purpose, to exercise it in §5).
6. Create a scoped agent credential and a grant for `led.*` only (see the quickstart); keep
   the operator token out of the agent's environment.
7. Photograph the setup, including the meter reading with the LED off.

## 4. Reference measurements (before any OpenMHS command)

Record with the multimeter and camera only: GPIO voltage with the LED off and on, set from the
board's serial console or by the firmware's own button/command, not through OpenMHS. Expected:
about 0 V and about 3.3 V. These define what "the device did X" looks like independently.

## 5. Cases

Each case is one row per run in `cases.csv`: id, run, OpenMHS result (state and codes),
physical observation (meter, camera frame), observer, time, notes. A case passes only if both
sides match. Repeat C1, C4, T2, T3 and R1 20 times; the rest once.

| ID | Case | Expected OpenMHS | Expected physical |
|---|---|---|---|
| C1 | `led.on` true with a grant | `VERIFIED` by readback | Meter ~3.3 V, LED lit |
| C2 | `led.on` false | `VERIFIED` | ~0 V |
| C3 | `led.pulse` n=1 once | per descriptor (no readback: `UNVERIFIABLE`) | `led.pulses` counter +1, no more |
| C4 | Same `led.pulse` request twice, same idempotency key | One command, same receipt | Counter +1 |
| P1 | No grant | `DENIED policy.denied.no_grant` | No change |
| P2 | Credential scoped to another device | `DENIED auth.scope.denied` | No change |
| P3 | Revoked credential | rejected | No change |
| T1 | Pull the Wi-Fi (power off the access point) before a write | `OFFLINE` after a probe, `DENIED device.unavailable` | No change |
| T2 | Cut the access point **during** a write (script the cut within the ack window) | `UNKNOWN_OUTCOME`; a further `led.pulse` refused (`command.outcome_unresolved`) | Whatever the board did, from the meter and camera |
| T3 | Restore the network, `reconcile` T2 | `RECONCILED_APPLIED` / `_NOT_APPLIED` | **Must agree with the meter and the board's own console. A disagreement is a critical finding** |
| T4 | Reset the board (EN button) | `OFFLINE` then `ONLINE`; LED back at its power-on default | Meter agrees |
| T5 | Kill the gateway mid-command, restart | Command `UNKNOWN_OUTCOME` after recovery, never resent | At most one effect |
| T6 | Start the gateway with the board unplugged | Gateway starts, device `OFFLINE`, comes online when plugged in | Nothing until then |
| R1 | **Stale readback.** Hang the node (hold it in the debugger or pause its task) so the broker keeps its last retained state, then send `led.on` | `UNVERIFIABLE`, **not** `VERIFIED`; reason says readings were older than the command | LED does not change (node hung) |
| R2 | Same, but let the command's effect happen and answer be lost, then reconcile while the broker still serves the retained pre-command state | `reconcile` refused (`NotReconcilable`, older than the command), never `RECONCILED_NOT_APPLIED` | Board applied it; meter shows it |
| S1 | Fill the journal (small quota from §3) with reads until `storage.exhausted` | New writes refused with nothing sent, audited reads refused, `audit verify` intact. This profile has no stop capability, so the stop-in-reserve path is not exercised on hardware; the board's power is the stop | No change |

R1 and R2 are the cases the QEMU run could not exercise, and the reason the readback rule
exists. Expect surprises; they are the point.

## 6. Pass and fail

- **Pass**: every case matches on both sides, and the repeated cases agree on all 20 runs
  (T2's physical result may vary between runs; T3 must match each one).
- **Fail**: a physical effect OpenMHS did not report; a reported effect the meter does not
  show; a resend after an unknown outcome; any command without authorization; R1 verified;
  R2 concluded "not applied". One such event fails the validation for this firmware and
  adapter, and it is reported.
- **Incomplete**: a case could not be run. State why; it is never counted as a pass.

## 7. Evidence bundle

Committed under `conformance/hardware/esp32-mqtt-node/<date>/` (create the directory with the
first bundle):

```text
README.md        device, firmware commit, build config, equipment with calibration dates,
                 measured clock offset and the --readback-skew-ms used, who ran it, what was not run
cases.csv        one row per case per run
journal.sqlite   the gateway journal for the session
audit-verify.txt output of `openmhs audit verify`
conformance.json `openmhs conformance` report (sources: device) if run
photos/          timestamped photos or frames: meter at C1, C2, T2, T3, R1, R2
```

Report failures as prominently as passes. Do not edit a bundle afterwards; a new run is a new
directory.

## 8. Before the first run

- [ ] The PR carrying ADR-0019/0020/0021 is merged and `openmhs` is built from a tagged commit.
- [ ] A second person has read this runbook and the bounds in §1.
- [ ] The multimeter's calibration date is known.
- [ ] `cases.csv` header and the bundle README are drafted.
- [ ] Time budget: about two days for setup and one pass; repeats take another day.

## 9. After

Add `hardware_report` to the registry entry (schema change, M16), publish the bundle, and ask
someone else to reproduce C1, T3 and R1 on their own board. Independent reproduction is what
makes the result credible.
