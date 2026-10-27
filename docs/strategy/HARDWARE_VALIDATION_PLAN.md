# Independent hardware validation plan

**Today no adapter has been run against physical hardware by the project.** Every
conformance report on file has `sources: ["simulator"]`. This plan is how the first
physically validated rows get into the compatibility matrix, honestly.

**Scope and limits.** Low-voltage, low-energy, bounded operations only, under a person's
supervision. Nothing here removes an interlock, defeats a device's own protection, or moves
anything that can hurt someone. A passing validation says the adapter and runtime behaved
as specified on that device, with that firmware, under these tests. It is not a safety
certification.

## 1. Choosing the first devices

| Candidate | Cost | Risk | Controllable | Independent observation | Fault testing | Already in repo |
|---|---|---|---|---|---|---|
| **ESP32 dev board** (LED, GPIO, pulse counter) | ≈ US$10 | Very low (3.3 V, USB powered) | Fully: our own firmware | Multimeter or logic analyzer on the GPIO; the board's serial console | Easy: pull Wi-Fi, reset, stop the broker | Firmware `firmware/esp32-mqtt-node`, MQTT profile, 18 tests run in QEMU (Experiment 02) |
| **Programmable bench PSU** (SCPI over LAN or USB) into a resistor | US$300–600 | Low with limits: ≤ 12 V into a rated resistor, current limit set on the front panel | Fully, via SCPI | A separate calibrated DMM across the load | Easy: pull the LAN cable, power-cycle | SCPI adapter with a bench-PSU profile, serial and TCP |
| 3D printer | US$200+ | **Medium**: 200 °C+ heaters, motion | Partly | Thermocouple, camera | Risky | Read-only OctoPrint adapter (Experiment 01) |
| USB sensor (temperature, light) | US$10–50 | Very low | Read only | A reference sensor next to it | Easy | Would need a profile |
| Low-voltage robot platform | US$100+ | Medium: motion | Partly | Camera, encoder | Risky | ROS 2 observation adapter |

**Recommendation: ESP32 first, bench PSU second.** The ESP32 reuses tested firmware and
tests; only the transport changes (real Wi-Fi instead of QEMU's network). The PSU is the
first third-party device: firmware we did not write, a protocol we did not invent, and an
independent instrument can check every claim. Leave printers and robots until both pass
and the readback-freshness gap (H4 in [ARCHITECTURE_GAP_ANALYSIS.md](ARCHITECTURE_GAP_ANALYSIS.md))
is fixed.

## 2. Test equipment

| Item | Purpose |
|---|---|
| The device under test, its exact model, hardware revision and firmware version recorded | Subject |
| A second, independent measuring instrument (DMM with a calibration date; or logic analyzer) | Reference measurement that does not go through OpenMHS |
| A network switch the tester can unplug, or a managed one | Disconnect tests |
| A USB power meter (ESP32) or the PSU's own front panel (PSU) | Cross-check |
| A camera on the device's display or LED, timestamped | Evidence of physical state over time |
| A host machine with `openmhs` built from a tagged commit | Runtime |

## 3. Reference measurements

Before any OpenMHS command, the tester records with the reference instrument only:

- ESP32: GPIO voltage with the LED off and on (set by the board's own serial console).
- PSU: output voltage at 0 V, 1 V, 2.5 V, 5 V set from the front panel, into the load.

These establish what "the device did X" looks like independently of the software under
test.

## 4. Test cases

Each case: steps, the expected OpenMHS result, the expected physical result, and how the
physical result is observed. A case passes only if **both** match.

### 4.1 Commands

| ID | Case | Expected OpenMHS | Expected physical |
|---|---|---|---|
| C1 | Set within range (PSU 2.5 V; ESP32 LED on) | `VERIFIED` | DMM reads 2.5 V ± spec; GPIO high |
| C2 | Set out of range (PSU 15 V when max is 12) | `DENIED command.value.out_of_range` | No change on the DMM |
| C3 | Slew violation | `DENIED command.slew_exceeded` | No change |
| C4 | Ramp in allowed steps | each step `VERIFIED` | DMM follows each step |
| C5 | Action once (ESP32 pulse) | `VERIFIED` or `UNVERIFIABLE` per descriptor | Counter increments by exactly 1 |
| C6 | Same request twice with one idempotency key | One command, same receipt | One effect |

### 4.2 Permissions

| ID | Case | Expected OpenMHS | Expected physical |
|---|---|---|---|
| P1 | No grant | `DENIED policy.denied.no_grant` | Nothing |
| P2 | Read-only grant, write attempted | `DENIED` | Nothing |
| P3 | Credential scoped to another device | `DENIED auth.scope.denied` | Nothing |
| P4 | Revoked credential | Rejected at the API | Nothing |
| P5 | Approval-required write, not approved | `AWAITING_APPROVAL` | Nothing until approved |

### 4.3 Timeouts, disconnects, restarts

| ID | Case | Expected OpenMHS | Expected physical |
|---|---|---|---|
| T1 | Unplug the network before a write | `OFFLINE`, then `DENIED device.unavailable` or `FAILED adapter.not_sent` | Nothing |
| T2 | Unplug the network during a write (time it with a slow command, e.g. PSU with `*OPC?` after a long ramp) | `UNKNOWN_OUTCOME`, no resend; later writes to that capability refused | Whatever the device did, recorded by DMM and camera |
| T3 | Reconnect, reconcile T2 | `RECONCILED_APPLIED` or `_NOT_APPLIED` | Must agree with the DMM. **A disagreement is a critical finding** |
| T4 | Power-cycle the device during idle | `OFFLINE` → `ONLINE` | Device back at its power-on default; the next read shows it |
| T5 | Kill the gateway mid-command, restart | Command `UNKNOWN_OUTCOME` after recovery, never resent | One effect at most |
| T6 | Start the gateway with the device off | Gateway starts; device `OFFLINE`; turns `ONLINE` when switched on | Nothing until then |
| T7 | Stale readings (ESP32: stop the node's publishing) | Reads `usable: false`; commands with freshness preconditions denied | — |

### 4.4 Repeatability

C1, C4, T2 and T3 run 20 times each. Report every run, not a summary. A case is repeatable
if all 20 runs agree on both the OpenMHS result and the physical result; T2's physical
result may vary (that is the point), but T3 must match it every time.

## 5. Pass and fail

- **Pass:** every case in §4 matches on both columns, and repeatability holds.
- **Fail:** any physical effect OpenMHS did not report, any reported effect the reference
  instrument does not show, any resend after an unknown outcome, any action without
  authorization. One such event fails the whole validation for that device.
- **Incomplete:** a case could not be run (state why). Incomplete is reported as
  incomplete, never as pass.

## 6. Evidence bundle

Committed under `conformance/hardware/<device-id>/<date>/`:

```text
README.md            device, firmware, equipment with calibration dates, who ran it, what was not run
cases.csv            one row per case per run: id, run, openmhs_result, physical_result, observer, notes
journal.sqlite       the gateway journal for the session (audit chain verifies)
audit-verify.txt     output of `openmhs audit verify`
conformance.json     `openmhs conformance` report against the device (sources: device)
photos/              timestamped photos or video frames of the reference instrument for T2/T3
```

The registry entry gains a `hardware_report` pointing at the bundle, and the matrix gains a
"Physically validated" row only from such a bundle.

## 7. Who runs it

The maintainer for the ESP32. For the PSU and anything after it, the plan is designed so
that someone else can run it: a lab with the instrument, following this page, submitting
the bundle as a pull request. Independent reproduction by a second person is what makes the
result credible.
