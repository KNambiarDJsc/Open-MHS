# One workflow, two independent devices, one injected fault

Phase 7. Can OpenMHS run a short workflow across two devices that were never designed to work
together, enforce its steps and approvals, survive a fault in the middle, and leave an account
afterwards? **Yes in simulation, with the human decisions the design asks for. A plain script
written against the same two interfaces ends the same fault in a state nobody has a record of.**

Simulators only; the stage bound and the approver are my experiment choices. No physical device.

Data: [`results/multi_device.json`](results/multi_device.json). Code:
[`experiments/multi_device_experiment.py`](experiments/multi_device_experiment.py). Evidence report
for the fault run: [`results/reports/multi_device_R2_report.md`](results/reports/multi_device_R2_report.md).

## The devices and the workflow

| Device | What it is | Reached through |
|---|---|---|
| `ofm.stage` | the OpenFlexure server's simulated stage (LabThings-FastAPI) | a fault-injecting proxy |
| `lab.hotplate` | the stirring-hotplate stand-in (HTTP Basic, synchronous) | directly |

Both were imported from their Thing Descriptions with one reviewed override file each; neither
was changed. The workflow, a three-step OpenMHS plan under one lease:

1. **heat**: set the plate to 40 Cel, **verified** by reading the target back. If the plan
   compensates, `stop` undoes it.
2. **move**: stage to x = 50, **needs a human approval**, bounded to +/-100, verified by reading
   the position back. On failure: ask a person.
3. **stop**: stop heating.

The fault: once the stage move has been accepted, **every status poll is lost** (the connection
is reset). The move happens; OpenMHS cannot see it finish.

## What happened

### R1: no fault

The plan stopped at the approval (`WAITING_APPROVAL`, nothing moved); the approver (a script that
approves only a stage move within the bound) approved it; the plan then ran to `SUCCEEDED`. Three
commands: set-program `VERIFIED`, move `VERIFIED` (approved by `operator:local`, device reference
recorded), stop `UNVERIFIABLE`. Audit chain valid (50 records).

The final `stop` is `UNVERIFIABLE` because the Thing Description gives no way to confirm a stop
and the reviewer declared none. The plan still counts it as success, as the spec says an
unverified step does. **Nothing in OpenMHS confirmed the heater stopped**; the harness's own
look at the hotplate did.

### R2: fault, then reconcile and resume

| Moment | OpenMHS | Ground truth |
|---|---|---|
| after the move's status polls were lost | move command `UNKNOWN_OUTCOME`; plan `AWAITING_DECISION`, reason "it may or may not have taken effect; reconcile it, then decide"; **step `stop` not sent** | stage at x = 50 (the move happened); hotplate still **heating** |
| operator waits, then reconciles the move | `RECONCILED_APPLIED` (position read back as 50 after the move started) | unchanged |
| operator decides `resume` | plan runs the final step, `SUCCEEDED` | hotplate cooling |

Nothing was resent, and the heater kept its state until a person decided: the plan did not guess in
either direction. Audit chain valid (53 records); the export names the human decision (who, what,
when, reason).

### R3: fault, then compensate

Same fault; the operator reconciles, then decides `compensate` instead of `resume`. The plan ends
`COMPENSATED`: the heat step's `stop` ran (hotplate cooling) and the final step never ran. The
stage **stays at x = 50**: its move had no compensation declared, and the plan "touches nothing
else" by design. A person wanting the stage back would have to move it, with the usual approval.

## The same fault with a plain script

A script of the kind a lab writes first: start the plate, move the stage, wait for it, stop the
plate. No gateway. The same polls were lost.

| Script | What it did | Ground truth afterwards |
|---|---|---|
| D1 no fault | all three steps | stage 50, hotplate stopped |
| D2 fault, bare | started heating, moved the stage, **lost the status**, raised and stopped | stage **at 50** (the script thinks the move failed), hotplate **still heating** |
| D3 fault, with a cleanup (`finally`: stop the plate) | same, then stopped the plate | stage at 50 (script thinks it failed), plate stopped |

D2 leaves the heater running and a stage that moved without anyone knowing it. D3 is safer for
the heater but still has no answer to "did the move happen?"; its operator would have to look at
the stage. Neither has an approval gate, a bound on the target, or a record beyond the script's
own output. D3 is a fair picture of a careful script. It is also the point at which most scripts
stop being written, because every added handler is another place to be wrong.

## What this shows

- **Ordering, lease and approval held across two devices.** The move did not happen until a
  person approved it, and the approver's decisions are in the record.
- **An uncertain step stopped the workflow without making anything worse.** The plan neither
  carried on nor undid the heating.
- **The two decisions a person can make were both available and both recorded.** `resume` after a
  reconcile, or `compensate`.
- **A single exportable report** tells the story: contracts in force, the plan, the human
  decisions, each command with authorization, lifecycle, device reference and verification.

## What this does not show

- That this workflow is meaningful in a lab: it was made up to exercise two devices.
- Any behaviour with devices that fail differently (a stage that stalls, a plate that overheats).
  The only fault was a lost status.
- That OpenMHS prevented the harm D2 shows: the script and the plan are different designs, and a
  careful script author could write most of the behaviour, as the benchmark notes
  (RELIABILITY_BENCHMARK.md).
- A real approver. The "human" is a script running with an operator token.
- Anything about hardware.
