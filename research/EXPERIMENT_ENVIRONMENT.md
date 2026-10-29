# Experiment environment

Phase 2. Everything here ran on one Windows 11 machine, against **simulators only**. The two
servers are kept separate from OpenMHS and unmodified: no LabThings or OpenFlexure source was
changed.

## Versions

| Component | Version |
|---|---|
| OS | Windows 11 Pro (10.0.26300), x86_64, 16 logical CPUs |
| Python | 3.13.4 (CPython); `uv` 0.7.11 for environments |
| labthings-fastapi | 0.3.0 from PyPI, source at `761441e` (0.3.0 + 11 commits) |
| openflexure-microscope-server | 3.0.0b1, source at `01d0dd3` (v3 development branch), installed editable |
| fastapi / pydantic / numpy / opencv-python-headless | 0.135.4 / 2.14.1 / 2.4.6 / 4.13.0.92 |
| OpenMHS | this repository, branch `research/labthings-wot`, debug build |

## Layout and storage

All third-party code and environments live on drive D: (the system drive is almost full).

| Path | What | Size |
|---|---|---|
| `D:\openmhs-research\src\labthings-fastapi` | shallow clone (depth 50) | 2 MB |
| `D:\openmhs-research\src\openflexure-microscope-server` | shallow clone + the downloaded web app | 12 MB |
| `D:\openmhs-research\venv` | one Python 3.13 venv with both packages | 403 MB |
| `D:\openmhs-research\uv-cache` | package cache | 56 MB |
| `D:\openmhs-research\ofm-sim` | simulator working directory (config, settings, logs, data) | small |
| `D:\openmhs-research\labbench` | the independent LabThings example | small |
| `D:\openmhs-research\work` | generated manifests, gateway homes, per-scenario ledgers | 7 MB |

No rebuilds were needed. The system drive's free space did not change during the work.

## Installation commands (reproducible)

```sh
# source
git clone --depth 50 https://github.com/labthings/labthings-fastapi.git
git clone --depth 50 https://gitlab.com/openflexure/openflexure-microscope-server.git

# environment (on a drive with space); installs labthings-fastapi 0.3.0 as a dependency
uv venv venv --python 3.13
cd openflexure-microscope-server
uv pip install --python ../venv/Scripts/python.exe -e .

# the server refuses to start without its web application; fetch the CI build
uv pip install --python ../venv/Scripts/python.exe python-gitlab
../venv/Scripts/python.exe pull_webapp.py          # downloads the build artifact from GitLab CI
```

`pull_webapp.py` downloads a prebuilt artifact from OpenFlexure's GitLab CI. Its code was read
before it was run; it fetches the artifact and unzips it into `static/`.

## Running the OpenFlexure simulator

```sh
cd ofm-sim && cp ../openflexure-microscope-server/ofm_config_simulation.json .
openflexure-microscope-server -c ofm_config_simulation.json --host 127.0.0.1 --port 5000
```

- Started and answered requests about **2 seconds** after launch.
- The first launch failed with `FileNotFoundError: The static directory does not exist. You will
  need to pull or compile the web app.` until `pull_webapp.py` had run. This is documented behaviour
  but is the main setup obstacle.
- Writes `openflexure/{settings,logs,data}` under the working directory.
- `GET /` answers **307** (redirect to the web app). The API prefix is **`/api/v3`**.

### Endpoints used

| Endpoint | Purpose | Observed |
|---|---|---|
| `GET /api/v3/thing_descriptions/` | all Thing Descriptions (116 KB, 14 Things) | 200 |
| `GET /api/v3/{thing}/{property}` | read a property | 200, bare JSON value |
| `POST /api/v3/{thing}/{action}` | start an action | **201** + invocation resource |
| `GET /api/v3/action_invocations/{id}` | poll an invocation | 200; 404 for an unknown id |
| `GET /api/v3/action_invocations/{id}/output` | an invocation's output | 200 |
| `DELETE /api/v3/action_invocations/{id}` | request cancellation | 200 "Cancel request sent" |

Validation errors are **422** with FastAPI's `detail` list (e.g. `extra_forbidden` for an
unexpected input key), returned before any invocation exists.

### What the simulator exposes

14 Things: `camera` (16 properties, 17 actions), `stage` (7, 8), `autofocus` (0, 6), `gallery`
(3, 1), `camera_stage_mapping` (7, 4), `illumination` (0, 2), `system` (6, 3), `smart_scan` (6, 5),
`histo_scan_workflow` (12, 2), `snake_workflow` (11, 0), `raster_workflow` (11, 0),
`bg_color_channels_luv` (6, 0), `bg_channel_deviations_luv` (5, 0), `timelapse` (5, 2). Totals:
**95 properties, 50 actions, 0 events.** Every Thing is `security: no_security`.
Capture: `research/fixtures/openflexure-sim-thing-descriptions.json`.

### Simulation limits

- The stage (`DummyStage`) models motion as timed steps. It has **no mechanical limits and no
  optics**; it will move to any integer coordinate.
- The camera is synthetic. Nothing here says anything about image quality, focus, illumination, or
  a real microscope's behaviour.
- A `GET` of a property that does not exist on a Thing returns 404 (`camera/exposure_time` does not
  exist in the simulation: the camera exposes different names).
- A direct stage move done outside OpenMHS was visible to the next OpenMHS read, confirming the
  adapter reads live state and caches nothing.

## The independent LabThings example (`LabBench`)

Not part of OpenFlexure. A minimal `lt.Thing` in `labbench/labbench.py` with:

- properties `position`, `busy`, `jobs_started` (read-only);
- `move_to(target, seconds, fail_after)`: moves in small steps, **cancellable**, and can raise an
  error part-way (`fail_after`);
- `move_uncancellable(target, seconds)`: same, but ignores cancellation;
- `where()`.

`position` changes only as the work really happens and `jobs_started` counts every execution, so a
test can tell "the invocation says completed" from "the state really changed" and "the work ran
twice" from "it ran once". Run with `labthings-server -c config.json --port 5100`.

## Licences

LabThings-FastAPI is MIT; OpenFlexure's server is GPL-3.0; OpenMHS is Apache-2.0. The integration
talks to the servers over HTTP and copies none of their code. One file in this repository,
`research/fixtures/openflexure-sim-thing-descriptions.json`, is **output captured from running
OpenFlexure's server** (its Thing Descriptions). It is data describing their HTTP interface, kept so the
tests run offline. If the OpenFlexure maintainers would rather it were not redistributed, it can be
removed and regenerated locally from the command above.

## Startup and shutdown

Both servers started in about two seconds and answered immediately. A running simulator holds
no state that needs saving for these experiments. Shutdown behaviour is recorded in the notes at the
end of the work (below).
