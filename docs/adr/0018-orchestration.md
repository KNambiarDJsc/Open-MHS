# ADR-0018: Capability graph, leases, stop lane and plans

- Status: Accepted
- Date: 2026-10-24
- Builds on: [ADR-0002](0002-command-lifecycle.md), [ADR-0010](0010-engine-execution-model.md),
  [ADR-0017](0017-identity-and-trust.md)

## Context

Up to M8 the gateway runs one command on one device at a time, safely. Coordinated work,
such as powering a stage, moving it, and waiting for a sensor, is left to the caller. A
caller that runs several commands has no way to keep others off the devices in between,
to stop a move in progress, to know in advance whether the whole sequence would be
allowed, or to know what to do when step three of five fails.

The constraint from M8 stands: no second execution path. Every step must be an ordinary
command through `Engine::submit`, as its caller, with its own receipt.

## Decision

### Graph

Relationships (`contains`, `depends_on`, `shares_resource`, `frame`) come from
descriptors (device facts) and from an operator topology file (installation facts), in
one object shape ([spec/descriptor.md](../../spec/descriptor.md) §7,
[spec/topology.md](../../spec/topology.md)). The graph is built from what is registered
when it is asked for, so it never goes stale. A bad topology stops the gateway; a
descriptor's dangling reference is only reported. The graph's one decision is what a
lease claims: containment and shared resources are followed, dependencies are not.

### Leases

A lease is a durable journal record: holder (principal and credential), the devices and
resources requested, the claim the graph computed, and a deadline (required, at most
24 hours). Acquiring is all-or-nothing in one transaction; an overlapping live lease
refuses it (`lease.conflict`). While a lease is live, a write or invoke on a claimed device
by anyone other than its holder is denied (`lease.held`), at the same point in evaluation
as scopes: after authentication, before the device is touched. Reads are not affected.

Leases end by release, by deadline, or by revoking the holder's credential. Expiry is
checked on use, so a gateway that crashed and restarted honours deadlines without a
sweeper. The claim is fixed when the lease is taken; a topology change takes effect for
new leases.

### Stop lane

An action may declare `"stop": true`. A stop is authorized and validated like any other
command, but it does not wait for the per-device lock and is not blocked by another
holder's lease: stopping must not wait for the motion it is meant to stop, and must not
depend on who holds the device. Stop capabilities may not declare preconditions, rate or
slew limits (descriptor validation). Adapters must accept a stop while another command is
executing. This replaces the M3 limitation recorded in ADR-0010.

### Plans

A plan is a list of steps, each a command request with an id, optional `after`
dependencies, optional plan-level preconditions (reads that must hold just before the step
is submitted), an `on_failure` of `halt`, `compensate` or `ask`, and an optional
compensating request.

- **Execution.** The plan leases the claim of every device its steps touch, then submits
  steps one at a time in a deterministic dependency order (ties by declaration order).
  Each step is an ordinary command, submitted as the plan's caller with a derived
  idempotency key, so the single-command policy path, its receipts and its audit apply
  unchanged. Parallel execution of independent steps is not part of v0.1.
- **States.** `RUNNING`, `WAITING_APPROVAL` (a step is held by policy), `AWAITING_DECISION`,
  `COMPENSATING`, and the terminal `SUCCEEDED`, `HALTED`, `COMPENSATED`, `ABORTED`,
  `CANCELLED`.
- **Failure.** A step that is denied, rejected, fails or expires follows its `on_failure`.
  `halt` ends the plan (`HALTED`) and leaves completed steps as they are. `compensate` runs
  the compensating requests of completed steps in reverse order, each as an ordinary
  command; if one of those fails, the plan waits for a decision. `ask` waits for a
  decision.
- **Uncertainty is never guessed.** A step in `UNKNOWN_OUTCOME` always moves the plan to
  `AWAITING_DECISION`, whatever its `on_failure` says. Neither retrying nor compensating
  something that may or may not have happened is automatic. The operator reconciles the
  command and then decides.
- **Decisions.** An approver decides: `resume` (only once the failed or uncertain step's
  command ended in success, for example after reconciliation), `retry` (submit the step
  again; refused while the step is uncertain), `compensate`, or `abort`.
- **Crash.** A plan that was `RUNNING` when the gateway stopped comes back as
  `AWAITING_DECISION`, with the reason. The interrupted step's command is recovered by the
  existing rules (`UNKNOWN_OUTCOME` if it was in flight).

### Twin preflight

`plans.preflight` dry-runs the whole plan in a separate, in-memory engine. Each device
in the plan is replaced by a virtual device built from its descriptor and seeded with
fresh readings of the real device. As built (M9 step 5), those are the engine's own reads,
the ones a real submission's live checks would make, rather than reads authorized as the
caller: descriptor preconditions are checked from engine reads in a real run, so seeding
through the caller's read grants would refuse plans that would in fact run. Step
preconditions are still read as the caller inside the twin, as in a real run. The twin
engine has the same policy, topology and rate and slew history, and the plan runs there
as the same caller. The result has one entry per step: whether it would be allowed, held for
approval, or refused and why, plus whether the lease could be taken now. A step held for
approval is reported and then assumed approved, so that later problems are found too.
Nothing reaches a real device apart from those reads.

The twin checks what the contract can check: policy, scopes, leases, units, ranges, rates,
slews, preconditions against readings, ordering, and readback against declared
verification. It does not model physics. A virtual device reads back what it was told, so
preflight cannot tell that a motor stalls or that a heater is slow.

## Alternatives considered

- **Plans as a client-side script.** Simple, but no lease, no crash recovery, no single
  record of what happened. Rejected.
- **A workflow engine or language.** Far more than v0.1 needs. Rejected.
- **Parallel step execution.** Useful, but it makes failure handling and replay much
  harder to reason about. Deferred; the plan format already carries dependencies.
- **Leases as advisory.** A lease that a non-holder's command can ignore protects nothing.
  Rejected.
- **Geometric conflict checks.** Out of scope; shared resources are declared by the
  operator.
- **Automatic retry of a failed step.** Contradicts ADR-0002's no-blind-retries rule for
  uncertain outcomes, and hides failures for definite ones. Rejected; `retry` is a decision.

## Consequences

- One more check in evaluation (`lease.held`) and one more descriptor field (`stop`).
- Plans, steps and leases are journal tables, created with `CREATE TABLE IF NOT EXISTS`
  like the rest. Old journals open unchanged.
- An L3 conformance requirement becomes testable: an adapter's device can be leased,
  driven by a plan, and stopped through the stop lane.
- Every plan step is a command with its own receipt, so CLI/MCP/SDK parity and the audit
  chain cover plans with no extra machinery.
