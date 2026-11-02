# Opentrons adapter (protocol simulation)

An out-of-process OpenMHS adapter that runs **reviewed** Opentrons protocols on the simulator
of the [opentrons](https://github.com/Opentrons/opentrons) package (10.1.0, Apache-2.0). It
never connects to a robot.

Opentrons keeps the protocol API, labware, pipettes and its simulator. The adapter adds one
rule: an agent picks a protocol by name and supplies numbers, never code. Each protocol is a
file pinned by its SHA-256 in `config.protocols`; the bytes that were hashed are the bytes
that run. A file edited after review is refused before dispatch (the
`protocol.<name>.reviewed` precondition) and again at run time.

| Capability | Kind | Notes |
|---|---|---|
| `protocol.run` | action | `run_id` (new per run), `protocol` (a reviewed name), `n_wells` 2..12, `transfer_ul` 10..100 uL, `diluent_ul` 20..100 uL, `mix_reps` 0..5. Irreversible, not idempotent. Precondition: the protocol file is the reviewed one. Verified by reading back `run.last_id`. Result: the protocol's SHA-256, the number of simulated commands and the SHA-256 of their log |
| `run.last_id`, `run.count`, `run.last_commands` | property | The simulator's run record |
| `protocol.<name>.reviewed`, `protocol.<name>.sha256` | property | The file's digest now, and whether it is the reviewed one |

**What a run proves.** The Opentrons simulator checks a protocol's steps against the robot,
labware and pipette. It does not track liquid: it accepts an overfilled well and a negative
volume. A `VERIFIED` run means the simulation completed and was recorded, nothing about
volumes or samples. The volume bounds are OpenMHS's, chosen so a well's 200 uL cannot be
exceeded (the descriptor cannot bound the sum of two parameters, so each is bounded alone).

## Run

```sh
uv venv --python 3.12 .venv && uv pip install --python .venv -r requirements.txt
OT_API_CONFIG_DIR=$PWD/.ot openmhs serve --adapter-manifest adapters/python/opentrons/manifest.json --allow-unsigned-adapters
```

Set `OT_API_CONFIG_DIR`, or opentrons writes its configuration under `~/.opentrons`.

opentrons 10.1.0 simulates the Flex only, and passes neither bundled data nor runtime-parameter
overrides at the API levels the Flex needs, so the adapter uses the documented interactive
context (`opentrons.simulate.get_protocol_api`) and calls the protocol's `dilute(protocol,
params)`. It imports opentrons at start, so that the seconds this takes do not come out of the
first run's timeout. The SDK no longer restricts when a native extension can be imported on Windows.

Simulation-only configuration: `state_file`, `evidence_dir` (each run's command log as JSON),
`fault_file`, `run_timeout_ms`. See
[research/lab-automation/02_opentrons](../../../research/lab-automation/02_opentrons/).
