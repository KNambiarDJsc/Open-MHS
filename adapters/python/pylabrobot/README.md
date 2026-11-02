# PyLabRobot adapter (simulated liquid handler)

An out-of-process OpenMHS adapter for one [PyLabRobot](https://github.com/PyLabRobot/pylabrobot)
`LiquidHandler` on PyLabRobot's device-free `LiquidHandlerChatterboxBackend`. It refuses
every other backend and never drives hardware.

PyLabRobot keeps its liquid-handling abstractions (deck, labware, tip and volume tracking,
the operations). The adapter translates; OpenMHS keeps authorization, the command lifecycle,
idempotency, recovery and the audit trail.

**What its state means.** The chatterbox backend models no physics. Every volume and tip
this adapter reports is PyLabRobot's *software tracker* state. A `VERIFIED` transfer means
the trackers recorded that transfer. It says nothing about liquid.

## Capabilities

| Capability | Kind | Notes |
|---|---|---|
| `liquid.transfer` | action | Fresh tip, aspirate, dispense, discard tip. Params: `transfer_id` (new for every transfer), `source` (`A1`..`A3`), `dest` (`B1`..`B3`), `volume` (1 to 200 uL). Irreversible, not idempotent. Precondition: a tip is available. Verified by reading back `transfer.last_id` |
| `tips.available` | property | Tips left in the rack |
| `source.a1.volume` .. `source.a3.volume`, `dest.b1.volume` .. `dest.b3.volume` | property | Tracked volume in uL |
| `transfer.last_id`, `transfer.count` | property | The last recorded transfer, and how many the simulator completed |

A refusal by PyLabRobot (no liquid, no room, no tip) restores the handler's state from a
snapshot taken just before the transfer, and is reported as `REJECTED`: nothing was applied.
A `transfer_id` already used is refused, so a reused id can never verify an earlier transfer.

## Run

```sh
python -m venv .venv && .venv/bin/pip install -r requirements.txt   # pylabrobot==0.2.2
openmhs serve --adapter-manifest adapters/python/pylabrobot/manifest.json --allow-unsigned-adapters
```

The manifest's `runtime.command` runs `python`: it must be the interpreter with PyLabRobot
installed. The adapter is not signed, hence `--allow-unsigned-adapters` (development only).

## Simulation-only configuration

Both are off unless `config` names them, and both must lie under the manifest's
`filesystem.write`:

- `state_file`: the deck's tracker state, saved after every completed transfer and loaded at
  start, so the simulated deck survives an adapter or gateway restart. Without it, a restart
  starts a fresh deck, and reconciliation of an earlier transfer would read that fresh deck.
- `fault_file`: the experiment harness writes `{"next_transfer": "apply_then_hang"}` (applied,
  answer never sent) or `"hang_before_apply"` to make the next transfer misbehave once.

The experiment that exercises all of this is
[research/lab-automation/01_pylabrobot](../../../research/lab-automation/01_pylabrobot/).
