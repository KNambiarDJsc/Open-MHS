# Experiment 03: Micro-Manager demo microscope through OpenMHS

## 1. Research objective

Do the abstractions from experiments 01 and 02, built around one composite action, carry over
to a microscope? A microscope has several devices behind one controller, mostly positional
writes, and images whose provenance matters. And how does this compare with the OpenFlexure
integration?

## 2. Upstream project and version

- **Software:** `pymmcore-plus` **0.18.1** (BSD-3) over `pymmcore` (LGPL-2.1), MMCore
  **12.5.1**, device API 75, on Python 3.12.11.
- **Device adapters:** Micro-Manager's test device adapters only, build
  `Micro-Manager-75.20261014.post1` (DemoCamera, Utilities), installed with
  `mmcore install --test-adapters --dest D:\...`: 1 MB.

## 3. Simulator environment

Micro-Manager's `MMConfig_demo.cfg`, unchanged:
- **Devices:** DemoCamera's camera (`DCam`), XY stage, Z stage, white-light shutter, three
  filter wheels, objective turret, light path and autofocus.
- **Config groups:** `Channel` (Cy5, DAPI, FITC, Rhodamine) and `Objective`.

The demo camera synthesizes 512×512 16-bit images.

## 4. Hardware represented

A motorized widefield fluorescence microscope. Nothing physical; **no image here is a
biological or optical measurement**.

## 5. Architecture

```text
agent ── writes: stage.x/y/z.position, camera.exposure, channel; invoke: camera.snap(image_id)
  ▼
OpenMHS gateway ── bounds and units, grants, leases, preconditions, journal, readback,
  │                unknown outcomes, audit
  ▼ adapter protocol
mm_adapter.py ── one OpenMHS device = one Micro-Manager core; MM devices are components;
  │              each image saved with a provenance record
  ▼
pymmcore-plus CMMCorePlus ── DemoCamera devices
```

## 6. Integration boundary

One Micro-Manager core owns every device of a microscope, and calls on one core are
sequential. An OpenMHS adapter describes one device. So:
- the microscope is **one OpenMHS device** (`mm.scope-01`);
- its Micro-Manager devices are **components** (`xy_stage`, `z_stage`, `camera`,
  `filters`).

The consequence: OpenMHS serializes all commands on the microscope, which matches what the
core allows anyway.

Positions are **writable properties**, verified by readback, not actions. A pair of numbers
cannot be one OpenMHS value, and a readback compares one property with one value, so an XY
move is two writes (`stage.x.position`, then `stage.y.position`). The channel is the
`Channel` group's preset. A snap is the one action.

## 7. Native baseline (A)

[native_baseline/probe.py](native_baseline/probe.py) loads the demo configuration through
pymmcore-plus. An exposure, an XY move, a Z move and one snap take about 50 ms.

**The demo stage has no limits.** Asked to go to x = 10⁹ µm, it did not refuse. The wait
timed out after 5 s, mid-travel, with the stage at an odd negative position
(−35,291, −35,441 µm). There are no property limits on either stage.

## 8. OpenMHS implementation

[adapters/python/micromanager/mm_adapter.py](../../../adapters/python/micromanager/mm_adapter.py)
(345 lines), on the existing SDK with no change to OpenMHS:
- It refuses any configuration with a device outside the DemoCamera and Utilities libraries.
- It imports pymmcore-plus and numpy at module level (experiment 02, finding 1).
- A snap saves the image (`.npy`) and a provenance record: the image's SHA-256, the OpenMHS
  command id, the time, and the positions, exposure and channel as Micro-Manager reported
  them just before the snap.
- `state_file` restores positions, exposure and channel at start, as a physical stage keeps
  its place.
- Its fault hook can also take the camera offline: it unloads the camera device, a real
  Micro-Manager operation, and loads it again later.

## 9. Commands and capabilities

| Capability | Contract |
|---|---|
| `stage.x.position`, `stage.y.position` (write) | ±5000 µm, `hazardous`, idempotent, readback within 0.5 µm |
| `stage.z.position` (write) | 0..100 µm, as above |
| `camera.exposure` (write) | 1..1000 ms |
| `channel` (write) | enum of the `Channel` presets |
| `camera.snap` (invoke) | `image_id` (new per image); precondition `camera.available`; readback `camera.last_image_id`; result SHA-256, width, height |
| `camera.available`, `camera.last_image_id`, `camera.images` (read) | |

## 10. Simulation and verification semantics

| Claim | Holds here? |
|---|---|
| Stage is where requested | Micro-Manager **reports** it there, within 0.5 µm. A real stage's encoders would say more; the demo stage is a model |
| Image content | Synthetic |
| Provenance | Attests what the software reported at the snap, bound to the command id and the image's SHA-256. It does not attest optical truth |
| Physical effect | **No** |

## 11. Test scenarios

[experiments/run_scenarios.py](experiments/run_scenarios.py), on the shared harness.

## 12. Results

Three runs on the release gateway, **all checks passed in all three**:
[run-1](results/run-1.json), [run-2](results/run-2.json), [run-3](results/run-3.json).

| Id | What happened |
|---|---|
| MM-01 | A 2×2 grid, FITC and DAPI at each position: 8 moves, 8 channel changes and 8 snaps, all `VERIFIED`. Each image's file hash, provenance record and receipt agree, and each record shows the requested position and channel. 8 distinct images |
| MM-02 | x = 6000 µm, x = 10⁹ µm, z = −5 µm, exposure 5 s: `DENIED` (`command.value.out_of_range`). Channel "UV": `not_allowed`. x in volts: `unit_mismatch`. None dispatched; the stage never moved |
| MM-03 | **Without a lease**, each command is serialized but a sequence is not. Agent B moved the stage between agent A's move and A's snap, so A's image was taken at x = 2000 µm, not A's 100. The provenance record shows 2000, so nothing is hidden, but A got the wrong field. **With a lease**, B's move was `DENIED` (`lease.held`), B could still read, and A's image was at 100 µm. After release B moved |
| MM-04 | Camera unloaded: `camera.available` false. Snap `DENIED` by the precondition (`command.precondition.failed`), never dispatched, while the stage still worked. Camera reloaded: snap `VERIFIED` |
| MM-05 | (a) The move happened and the answer was lost: `UNKNOWN_OUTCOME`. Reconciled straight away: `RECONCILED_APPLIED`. (b) Another lost move, to 700 µm. Because a position is idempotent, a new move to 800 µm was allowed before reconciliation, as `spec/lifecycle.md` §4 rule 3 says. Reconciling the 700 µm move afterwards gave **`RECONCILED_NOT_APPLIED`, although the stage did reach 700 µm** (finding 2) |
| MM-06 | Image, provenance record and receipt agree; the exported evidence verifies offline, and the exported receipt holds the image's SHA-256. One changed pixel in the saved image no longer matches the receipt |
| MM-07 | The gateway was killed after a move. On restart: `UNKNOWN_OUTCOME`. Reconciled: `RECONCILED_APPLIED`, against the restored stage position |

Conformance: **L1**, 87 pass and 2 fail ([conformance.json](results/conformance.json)). The
two failures are `camera.snap`'s, with the same cause as experiments 01 and 02 (a reused
`image_id`). **Every stage, exposure and channel check passes, at L2 and L3.**

## 13. Performance

Release gateway, 20 positions per run. Each position is one verified move and one verified
snap with provenance:

| | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| Through OpenMHS: median / p95 / max | 112 / 136 / 137 ms | 114 / 131 / 131 ms | 110 / 136 / 181 ms |
| Micro-Manager alone (setXYPosition, wait, snap) | 32 / 40 / 41 ms | 33 / 38 / 38 ms | 32 / 38 / 39 ms |
| Gateway start | 0.91 s | 0.89 s | 0.89 s |

About 80 ms per position is OpenMHS. That covers two commands, each with its journal writes
and a readback, plus saving the image and its record. Not measured: CPU, memory.

## 14. Reliability comparison

| | A. Micro-Manager alone | B. Custom wrapper | C. Through OpenMHS |
|---|---|---|---|
| Out-of-envelope move | Attempted; the demo stage wanders (§7) | Would need limits | Refused (MM-02) |
| Two clients | Interleave freely | Would need locking per sequence | Commands serialized; sequences exclusive under a lease (MM-03) |
| Camera gone | The snap fails at the core | Would need a check | Refused before dispatch (MM-04) |
| Lost answer, crash | Not recorded | Would need intent records | `UNKNOWN_OUTCOME`, reconciled (MM-05, MM-07), with the caveat of finding 2 |
| Image provenance | Metadata if the script saves it | Would need its own | Bound to the command and to a verifiable audit chain (MM-06) |

Column B was not built.

## 15. Failures and unexpected findings

1. **One controller, one device.** Micro-Manager's single core makes the microscope one
   OpenMHS device with components. Fine for one microscope; it rules out OpenMHS locking the
   stage while another agent uses the camera. Micro-Manager could not run them in parallel
   anyway.
2. **Reconciling a superseded idempotent write is misleading.** The spec lets a new write to
   an idempotent capability through while an older one is unknown. A later reconciliation of
   the older one then compares the *current* value with its target and reports
   `RECONCILED_NOT_APPLIED` although it was applied. For positions, reconciliation answers
   "is the target there now?", not "did this command take effect?". Options for the core:
   - refuse reconciliation once a newer write to the same capability has completed;
   - resolve such a command as superseded rather than "not applied";
   - hold idempotent capabilities too.
   This needs a design note and a test before any change.
3. **A move-then-snap is only safe as a sequence under a lease.** Command-level serialization
   is not enough. The provenance record made the interleaving visible, so nothing was
   misreported, but the agent received the wrong image.
4. **The adapters' simulated "no response" held a lock.** Found by conformance here: reads
   timed out after the first hang, because a real device that stops answering one command
   does not stop answering reads. Fixed in all three adapters; experiments 01 and 02 were
   re-run afterwards and still pass ([01 final](../01_pylabrobot/results/final.json),
   [02 final](../02_opentrons/results/final.json)).
5. **No vector values.** An XY move is two commands, each verified alone; there is no
   atomic two-axis move with one readback.

## 16. What OpenMHS contributes

- Stage and exposure envelopes the demo devices lack.
- Leases for exclusive acquisition sequences.
- A precondition on the camera.
- Explicit unknown outcomes and crash-safe reconciliation, with finding 2's caveat.
- Image provenance bound to command ids and to offline-verifiable evidence.

## 17. What OpenMHS does not contribute

- Acquisition engines: Micro-Manager's multi-dimensional acquisition (MDA) is not used here.
- Autofocus.
- Any optical truth.

It also adds about 80 ms per position, and two commands for an XY move.

### Compared with the OpenFlexure integration (§7.4 of the brief)

| | OpenFlexure (existing) | Micro-Manager (here) |
|---|---|---|
| How it is integrated | Imported from the server's WoT Thing Description into the HTTP adapter, with a reviewed profile of bounds | A Python adapter over a library |
| Stage | Governed moves, bounds from the profile | Governed writes, bounds in the descriptor |
| Camera | Outside OpenMHS: a separate MCP server captures and measures | Inside: a governed action with provenance and a camera precondition |
| What carried over unchanged | Bounds, grants, unknown outcomes, reconciliation, audit export | Same |
| What stayed vendor-specific | WoT import and its overrides | One core per device, the Channel group, unloading a device as a fault |

The contracts (bounded positions, readback within a tolerance) and every failure scenario
transferred unchanged. The adapter code did not: one is generated from a WoT description, the
other is hand-written. A consistent developer experience across both would need a shared
"microscope" capability vocabulary: positions in µm, a channel enum, a snap action with an
image id. Neither integration defines one yet.

## 18. Reproduction

[README.md](README.md). Three runs reproduced every outcome.

## 19. Open questions

- Reconciliation semantics for superseded idempotent writes (finding 2).
- Vector values or multi-property readback for XY moves.
- Should Micro-Manager's MDA be governed as a whole (like an Opentrons protocol), not step by
  step?
- A shared microscope capability vocabulary across OpenFlexure and Micro-Manager.

## 20. Recommended next experiment

SiLA 2 (04): the first experiment where the device description comes from a standard (SiLA
Feature Definitions) instead of being written by hand.
