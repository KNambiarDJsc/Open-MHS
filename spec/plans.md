# Plans (v0.1)

A plan runs several commands, possibly across devices, as one unit: in a fixed order,
under one lease, with a declared response when a step fails. Every step is an ordinary
command submitted through the single-command path ([lifecycle.md](lifecycle.md)), so it
gets the same scope and policy checks, validation, receipt and audit record as any other
command. A plan adds ordering, a lease and failure handling. It adds no way around a check.
Design: [ADR-0018](../docs/adr/0018-orchestration.md).

## 1. Shape

```json
{
  "description": "power the stage and move it to the work position",
  "lease_ttl_ms": 900000,
  "steps": [
    {
      "id": "output-on",
      "request": { "target": { "device": "sim.psu-01", "capability": "power.output.enabled" },
                   "operation": "write", "value": true },
      "on_failure": "compensate",
      "compensate": { "target": { "device": "sim.psu-01", "capability": "power.output.enabled" },
                      "operation": "write", "value": false }
    },
    {
      "id": "move",
      "after": ["output-on"],
      "preconditions": [
        { "device": "sim.axis-01", "capability": "axis.homed", "op": "eq", "value": true }
      ],
      "request": { "target": { "device": "sim.axis-01", "capability": "axis.move_absolute" },
                   "operation": "invoke",
                   "params": { "position": { "value": 50, "unit": "mm" },
                               "speed": { "value": 20, "unit": "mm/s" } } },
      "on_failure": "ask"
    }
  ]
}
```

| Field | Meaning |
|---|---|
| `steps[].id` | 1–64 letters, digits, `-`, `_`, `.`; unique in the plan |
| `steps[].request` | A write or invoke request ([schemas/command.schema.json](schemas/command.schema.json)) with no `idempotency_key`; the plan derives one per attempt |
| `steps[].after` | Steps that must have succeeded first |
| `steps[].preconditions` | Readings (`device`, `capability`, `op`, `value`, optional `max_age_ms`) that must hold just before the step is submitted. They are compared as descriptor preconditions are. A reading that is missing, stale or fails the comparison fails the step without sending anything |
| `steps[].on_failure` | `halt` (default), `compensate` or `ask` (§3) |
| `steps[].compensate` | A request that undoes the step, used when the plan compensates |
| `lease_ttl_ms` | The plan's lease runs this long from the start of each step (default 15 minutes, at most 24 hours) |

At most 100 steps. Steps run one at a time in dependency order; among steps that are
ready, the one declared first runs first. Parallel steps are not part of v0.1.

## 2. Running

1. **Checks before anything happens.** The structure, every request and compensation, and
   every precondition target are checked together. Any problem refuses the whole plan
   with all its `plan.*` errors, and nothing is leased or sent.
2. **Lease.** The plan leases every device its steps and compensations act on, with the
   claim rule of [topology.md](topology.md). If that fails (`lease.conflict`,
   `lease.not_authorized`, …) the plan is refused. The lease is renewed before each step.
   If it was lost, a new one is taken; if that fails, the plan waits for a decision.
3. **Steps.** Before each step its preconditions are read, as the plan's caller. The step
   is then submitted as the plan's caller, with idempotency key
   `plan:<plan id>:<step id>:<attempt>`. The step's command and receipt are ordinary, and
   the plan records the command id.
4. **End.** When every step has succeeded the plan is `SUCCEEDED`. When a plan ends, for
   any reason, its lease is released.

A step counts as succeeded when its command's outcome is `succeeded` or `unverified`
(lifecycle.md §6). It counts as failed when the outcome is `failed`, `denied` or
`not_executed`. An `uncertain` outcome, or one still pending, counts as **uncertain**.

`plans.submit` returns when the plan ends or waits, as `commands.submit` does.

## 3. Failure

| Step result | `halt` | `compensate` | `ask` |
|---|---|---|---|
| Failed definitely | `HALTED`: completed steps stay as they are | `COMPENSATING` | `AWAITING_DECISION` |
| Uncertain | `AWAITING_DECISION` | `AWAITING_DECISION` | `AWAITING_DECISION` |
| Held for approval | `WAITING_APPROVAL` | `WAITING_APPROVAL` | `WAITING_APPROVAL` |

**An uncertain outcome is never guessed.** Whatever `on_failure` says, the plan stops,
keeps its lease, and submits nothing more until a person decides. Retrying or compensating
something that may or may not have happened is never automatic.

**Compensation** runs the `compensate` requests of the completed steps that have one,
newest first, each as an ordinary command (key
`plan:<plan id>:<step id>:undo:<attempt>`). Steps without a compensation stay as they are.
The step that failed is not compensated. When every compensation has succeeded the plan is
`COMPENSATED`. A compensation that fails, or whose outcome is uncertain, makes the plan
wait for a decision.

**Approval.** A step held by policy pauses the plan in `WAITING_APPROVAL`. When an approver
approves the command, the command runs and the plan carries on before the approval call
returns. If the approval is denied, withdrawn or expires, or the command is cancelled, the
step has failed and the plan follows its `on_failure`.

## 4. States

| State | Meaning |
|---|---|
| `RUNNING` | Submitting steps |
| `WAITING_APPROVAL` | A step (or compensation) is held for approval |
| `AWAITING_DECISION` | Stopped for a person: an uncertain step, `ask`, a failed compensation, a lost lease, or a restart |
| `COMPENSATING` | Undoing completed steps |
| `SUCCEEDED` | Every step succeeded (terminal) |
| `HALTED` | A step failed under `halt` (terminal) |
| `COMPENSATED` | Compensation finished (terminal) |
| `ABORTED` | An approver ended it (terminal) |
| `CANCELLED` | Its submitter or an approver cancelled it, or its credential was revoked or rotated (terminal) |

Step statuses: `PENDING`, `RUNNING`, `WAITING_APPROVAL`, `SUCCEEDED`, `FAILED`,
`UNCERTAIN`, `COMPENSATING`, `COMPENSATED`, `COMPENSATION_FAILED`,
`COMPENSATION_UNCERTAIN`. The plan's `current` names the step it is on or stopped at, and
`reason` says why it is waiting or how it ended.

## 5. Decisions

Only a human approver decides (`plan.decision.not_approver` otherwise, including for the
plan's own agent). Decisions apply to a plan in `AWAITING_DECISION`, except that `abort`
also applies in `WAITING_APPROVAL`.

| Decision | Allowed when | Effect |
|---|---|---|
| `resume` | The step in question succeeded, for example reconciled as applied, or was never sent | Carry on after it |
| `retry` | The step's command failed definitely (including reconciled as not applied) | Submit it again as a new attempt with a new key |
| `compensate` | The plan is not compensating already | Compensate the completed steps (a step reconciled as applied counts as completed) |
| `abort` | Always | End the plan (`ABORTED`); withdraw a command it is waiting on; touch nothing else |

While the step's outcome is still uncertain, `resume`, `retry` and `compensate` are refused
(`plan.decision.uncertain`). Reconcile the command first (`commands.reconcile`), then
decide. Every decision is recorded in the plan with who made it and when, and audited.

## 6. Restart

A plan that was `RUNNING` or `COMPENSATING` when the gateway stopped comes back in
`AWAITING_DECISION` with the reason. Its interrupted step is set from what the journal
holds. A command found under the step's key is linked and judged by its state after
command recovery (an in-flight command is `UNKNOWN_OUTCOME`, so the step is uncertain). If
no command was created, the step is still `PENDING`. A plan waiting for approval whose
command ended during the outage is treated the same way. Nothing is submitted
automatically after a restart. The plan keeps its lease until the lease's deadline.

## 7. Who sees and controls a plan

- The plan runs as the caller that submitted it: the same principal, credential and
  scopes, recorded with the plan. Revoking or rotating that credential cancels the plan.
- An agent sees only its own plans; other ids answer `plan.unknown`. Human and service
  principals see all.
- The submitter or an approver can cancel. A running plan stops at the next step boundary,
  after the step in progress finishes. Cancelling withdraws a held command and releases
  the lease.
