# Micro-Manager adapter (demo devices)

An out-of-process OpenMHS adapter for a [Micro-Manager](https://github.com/micro-manager/micro-manager)
core through [pymmcore-plus](https://github.com/pymmcore-plus/pymmcore-plus), with Micro-Manager's
**demo devices only**: it refuses any configuration with a device outside the DemoCamera and
Utilities libraries.

One Micro-Manager core owns every device of a microscope, so the adapter exposes **one OpenMHS
device** (`mm.scope-01`) whose components are the Micro-Manager devices.

| Capability | Kind | Notes |
|---|---|---|
| `stage.x.position`, `stage.y.position` | write | ±5000 um, hazardous, idempotent, readback within 0.5 um. An XY move is two writes |
| `stage.z.position` | write | 0..100 um |
| `camera.exposure` | write | 1..1000 ms |
| `channel` | write | A preset of the `Channel` group |
| `camera.snap` | invoke | `image_id` (new per image). Precondition: a camera is loaded. Saves the image and a provenance record (SHA-256, command id, time, positions, exposure, channel as Micro-Manager reported them). Readback: `camera.last_image_id` |

**What an image proves.** The demo camera synthesizes images. The provenance record attests
what the software reported at the snap, nothing optical.

Install: `pip install -r requirements.txt`, then `mmcore install --test-adapters --dest mm`
next to the manifest, and set `config.adapter_dir` to the folder it creates. pymmcore-plus and
numpy are imported at module level, so a missing install fails at start; the SDK no longer
requires that order. Simulation-only configuration: `state_file`, `evidence_dir`, `fault_file`
(including `camera_offline` and `camera_online`). See
[research/lab-automation/03_micromanager](../../../research/lab-automation/03_micromanager/).
