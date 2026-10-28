# ADR-0024: Conditional plan steps, and why parallel steps wait

- Status: Accepted
- Date: 2026-10-28
- Builds on: [ADR-0018](0018-orchestration.md)

## Context

Plans already have dependencies (`after`) and preconditions. A failed precondition fails
the step and so stops or compensates the plan. Many workflows need "do this only if": switch
the output on only if protection has not tripped, cool only if the chamber is above a
threshold. Writing that as a precondition makes a normal branch look like a failure.

The platform brief also asks for parallel steps.

## Decision

### Conditional steps (`when`)

A step may carry `when`: conditions in the same form as preconditions, read fresh just
before the step would start.

- **All hold:** the step runs, then its preconditions are checked as before.
- **One is definitely false** (a usable reading fails the comparison): the step is
  `SKIPPED` and nothing is sent. Every step whose `after` names a skipped step is skipped
  too, because what it depends on did not happen. The plan carries on with independent
  steps. `plan.step.skipped` is audited with the reason.
- **Undecidable** (the device does not answer, the reading is stale or of bad quality, or
  the values cannot be compared): the step **fails**. Not knowing is never "false":
  skipping on a missing reading would turn a fault into a silent change of plan.
- Skipped steps are not compensated, since they had no effect. A plan whose steps all
  succeeded or were skipped has succeeded.
- A dry run (`plans.preflight`) evaluates `when` against the twin's readings, reports
  `skipped`, and the plan can still be `ok`. Real readings decide again at run time.
- `check_plan` refuses a `when` that reads something not readable, as for preconditions.

### Parallel steps: not yet

The plan runner is a durable state machine with one current step:
- approvals hold that step;
- decisions (`resume`, `retry`, `compensate`, `abort`) apply to it;
- crash recovery settles it;
- compensation undoes completed steps newest first.

Running steps at the same time changes each of these at once: several held commands, a
decision that must say which branch, a partial halt with one branch still in flight,
recovery of several interrupted steps, and an undo order across branches. Getting any of
them wrong produces exactly the failures the lifecycle exists to prevent: a step sent after
the plan was told to stop, or an undo that runs while its branch is still moving.

So parallel execution is deferred to its own ADR. Its rules, to be designed then:
- only steps with no shared device or resource in the capability graph run together, under
  the plan's one lease;
- an unknown outcome on one branch stops new steps on every branch and waits for a person;
- compensation runs only after every branch has stopped.

Plans keep running ready steps one at a time, which is correct, only slower.

## Consequences

- `PlanStep.when`, `StepStatus::Skipped`, `StepVerdict::Skipped` and the
  `plan.step.skipped` audit kind are added. Clients that match on step status or verdict
  see a new value.
- A skipped step's `error` holds the reason (`plan.step.skipped`), so the record says why.

## Tests

`crates/openmhs-engine/tests/conditional_steps.rs`:
- a false condition skips a step and its dependent, an independent step still runs, the
  device receives only the two commands that ran, and the skipped change did not happen;
- a condition on a device that does not answer fails the step and sends nothing;
- a dry run reports both skips and stays `ok`;
- a condition on an unreadable capability is refused when the plan is checked.
