# Workcell plan

A plan for the virtual lab: power the stage and move it to the work position. It
leases the supply and the axis, then runs five steps in order:

| Step | Command | If it fails |
|---|---|---|
| `set-voltage` | supply setpoint 0.8 V | undo the steps done so far (`compensate`); this one undoes to 0 V |
| `output-on` | supply output on | `compensate`; undoes to off |
| `home` | home the axis | `compensate` |
| `move` | axis to 50 mm at 50 mm/s, only if `axis.homed` is true | `compensate`; undoes to 0 mm |
| `output-off` | supply output off | `ask` a person |

Every step is an ordinary command: the same grants, limits, preconditions and readback
verification as when it is sent alone ([spec/plans.md](../../spec/plans.md)).

## Run it

```sh
openmhs serve --sim                           # terminal 1; the rest as the operator:
for d in sim.psu-01 sim.axis-01; do
  openmhs grants add --id "cell-$d" --principal agent:cell --device "$d" --capability '*' \
    --ops read,write,invoke --effect allow
done
openmhs tokens create agent:cell --any-scope  # prints the agent's token
export OPENMHS_TOKEN=<agent token>
openmhs plans preflight examples/workcell/plan.json   # dry run against twins
openmhs plans submit examples/workcell/plan.json
openmhs plans show <plan id>
```

`plans preflight` runs the plan against twins of the devices, seeded with their current
readings. It changes nothing real, and its twins have no physics: `ok` means the plan
fits the contract and the current readings, not that it will succeed.

## Break it

Inject a simulator fault as the operator, then submit again:

| Fault | What happens |
|---|---|
| `openmhs sim fault sim.axis-01 reject` | `home` fails. The supply's steps are undone, newest first: output off, then 0 V. The plan ends `COMPENSATED` |
| `openmhs sim fault sim.psu-01 reject` | `set-voltage` fails. Nothing reaches the axis; nothing needed undoing |
| `openmhs sim fault sim.psu-01 apply-then-hang` | The setpoint is applied but never answered. The plan stops `AWAITING_DECISION` and sends nothing more, whatever `on_failure` says |

For the last one, the plan waits for facts and a person:

```sh
openmhs sim fault sim.psu-01 clear
openmhs commands reconcile <set-voltage command id>   # reads the device: RECONCILED_APPLIED
openmhs plans decide <plan id> --decision resume       # operator: carry on after it
```

Deciding `resume` before reconciling is refused (`plan.decision.uncertain`). These runs
are tested in [crates/openmhs-gateway/tests/workcell.rs](../../crates/openmhs-gateway/tests/workcell.rs).
