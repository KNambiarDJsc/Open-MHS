# Experiment 02: results

Run on Windows 11 x86_64 on 2026-10-25 (local time), Python 3.11, ESP-IDF v5.5.5, Espressif QEMU
9.2.2 (`esp_develop_9.2.2_20260417`), Mosquitto 2.1.2, and an `openmhs` debug build of this
branch. See the [README](README.md) for what the experiment is and how to rerun it. Raw evidence
is in [`evidence/`](evidence):

| File | What it is |
|---|---|
| [`demo-transcript.txt`](evidence/demo-transcript.txt) | Output of `demo.py`: ten steps with the real CLI, tokens redacted |
| [`test-run.txt`](evidence/test-run.txt) | `python -W error::ResourceWarning -m unittest test_node -v`: 18 tests, OK, exit 0 |
| [`node-serial.log`](evidence/node-serial.log) | The node's serial console across that test run (every command it applied, every reconnect, every boot) |

> **Simulated vs physically verified.** Every result below is *simulated*: the firmware is real
> ESP-IDF code, the ESP32 is QEMU's model, the broker is real Mosquitto. Nothing is *physically
> verified*, and no claim about a physical ESP32, Wi-Fi or any actuator follows from it.

## Summary

| Check | Result |
|---|---|
| Firmware builds for `esp32` (ESP-IDF v5.5.5) and boots in QEMU, joins the emulated network, connects to the broker | yes |
| `openmhs profile check` on the node's MQTT profile | ok |
| Tests | 18 run, **18 passed**, 0 failed, 0 skipped, 102.5 s, no `ResourceWarning` |
| End-to-end demo | 10 steps, exit 0, audit chain verified (53 records) |
| Rust workspace regression | see [Regression](#regression-and-security-tests) |

## What the tests cover

| Group | Tests | Needs |
|---|---|---|
| Firmware alone, over MQTT: retained state with the node's own clock; an LED command is acknowledged and visible in the state; bad commands are refused or ignored and change nothing; a pulse counts once per message and a repeated id counts again (the node is not idempotent, by design); acknowledgements carry the command's id when commands overlap | 5 | QEMU, Mosquitto |
| Through a real gateway (numbered, they build on each other): discovery; no grant means nothing is published; granted reads carry the node's timestamps; a write is acknowledged and verified by readback; a value outside the declared range never leaves the gateway; an action runs once; a powered-off node is uncertain and not retried; a hung node's readings go stale; a write to a hung node reconciles to what the node did; an action to a hung node runs at most once; broker down; a reboot is visible; the audit log verifies | 13 | QEMU, Mosquitto, `openmhs` |

## How the lifecycle behaved

| Situation | Gateway result | What the node did |
|---|---|---|
| `exec led.on --value true` with a grant | `VERIFIED`: acknowledged, then read back `true` | applied, acknowledged, published state |
| `exec led.pulse --param n=2` with a grant | `UNVERIFIABLE`: "device says applied; not confirmed" (no readback is declared) | counter 0 to 2 |
| No grant | `DENIED` `policy.denied.no_grant`, nothing sent | received nothing (broker tap: 0 messages) |
| `n=11` (range is 1..10) | `DENIED` `command.value.out_of_range`; `n=2.5` is `command.schema` | received nothing |
| Node frozen, write | `UNKNOWN_OUTCOME`, `adapter.timeout: no result within 3000 ms`, not retried | applied it **after** being released |
| Same command, later | `commands reconcile`: `RECONCILED_APPLIED`, readback `true` matches | led on |
| Node frozen, reading | value flagged `STALE: older than 4000 ms; not usable for decisions`, quality still `GOOD` | n/a |
| Node powered off, action | `UNKNOWN_OUTCOME`, `adapter.timeout`; published once, never retried; `commands reconcile` is `command.not_reconcilable` (no readback to compare) | gone; after power-on the counter is 0, nothing applied late |
| Broker stopped | reads `adapter.disconnected`; a command is `FAILED` `adapter.not_sent`; recovers when the broker returns | reconnects by itself |

## Findings

Things the run showed that reading the profile and the MQTT adapter documentation did not:

1. **A node that stops answering may still apply the command later.** With the machine frozen,
   the broker held the connection and delivered the command; on release the node applied it. In
   the saved run both the write (`led -> on`, serial log line 141) and the pulse (`pulses += 1`,
   line 144) were applied after the gateway had already reported `UNKNOWN_OUTCOME`. This is the
   case the state exists for. It is also why the gateway must not retry: a retried pulse would
   have been applied twice. For the write, reconciling by readback concluded the right thing in
   every run; the test asserts that the gateway's conclusion matches the node's actual state.
2. **A pulse that may have been applied cannot be reconciled.** With no readback the gateway
   says so (`command.not_reconcilable`) and leaves it to the operator. That follows from the
   design, and this run is the first test of it against a device that does apply late.
3. **QEMU's `stop` freezes the guest clock.** The node resumed minutes behind real time, so its
   timestamps were wrong. The harness re-sends the time after a freeze and the firmware steps its
   clock if it is more than a second out. A physical board that hangs does not lose its clock
   this way; this is a simulator artefact the harness works around.
4. **The clock comes over MQTT here, not SNTP.** On Windows the host's UDP port 123 belongs to
   the Windows Time service, so QEMU's user-mode network cannot reach an SNTP server on the host.
   The firmware has both paths; the tests exercise only the MQTT one. The SNTP path is untested.
5. **Toggling the emulated Ethernet link did not recover reliably** (`set_link` on the open_eth
   NIC). Tests that cut the link were replaced by powering the node off, freezing it and
   stopping the broker. A link that drops and returns, the commonest real failure, is therefore
   *not* covered.
6. **A message without a usable `id` gets no acknowledgement.** The node ignores it (there is
   no one to answer), so a client that publishes without an id waits for its timeout. The gateway
   always sends one.
7. **QoS and sessions.** Commands are QoS 0 on a clean session, as documented. The late
   application in finding 1 is explained by the TCP connection staying open while the machine
   was frozen (the broker delivered into a socket the guest read after release), not by the
   broker queueing anything: a node that is powered off receives nothing, as the demo's
   power-off step shows (counter 0 after power-on). The explanation is inferred from the
   behaviour and the broker log; it was not isolated further.

## Regression and security tests

This experiment added no file under `crates/`, `spec/` or `Cargo.*`; the workspace results are
recorded in the [M10 test report](../../release/M10_TEST_REPORT.md). The security-relevant tests
of this experiment are: no grant means no message on the broker; out-of-range values never leave
the gateway; commands are published once and never retried; a stale reading is not marked usable;
a broker failure surfaces as an error and not as a stale value; and the audit chain verifies at
the end. The broker is bound to `127.0.0.1`, anonymous, with persistence off, and the node
reaches it through QEMU's user-mode network, so nothing is exposed.

## Known limitations

* Simulated only. Real boards differ in boot time, Wi-Fi behaviour, reconnect timing, GPIO
  electrical behaviour and brown-outs, none of which QEMU models.
* The node publishes its state every second, which hides how a slower device would age out; the
  freshness limit in the profile (4000 ms) is a test setting, not a recommendation.
* No authentication, TLS, ACLs or QoS 2; no multi-node or long-running test; one platform.
* The firmware is a test fixture, not a reference design: no OTA, no watchdog (QEMU's was disabled
  to keep the guest alive across freezes), no persistent state, no input validation beyond what
  the profile's tests exercise.
* Nothing was run in CI. A job that builds the firmware in Espressif's container and runs these
  tests has not been written; the build needs ESP-IDF and the tests need QEMU and Mosquitto.
* Not run: any reconnection of a flapping link, SNTP, a second node on the same broker, a node
  that sends malformed state.

## What this validates

That OpenMHS's MQTT adapter, grant and validation layers, timeout handling, no-retry rule,
reconciliation and audit log behave as documented against a device that really applies commands
late and loses its network, with no change to the core. It says nothing about the safety of
operating a physical device, and it does not replace running the same profile on a real ESP32.
