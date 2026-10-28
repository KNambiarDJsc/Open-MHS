# ADR-0025: Parallel plan steps, opt-in and settled before the plan reacts

- Status: Accepted
- Date: 2026-10-28
- Builds on: [ADR-0018](0018-orchestration.md), [ADR-0024](0024-conditional-steps.md)

## Context

ADR-0024 deferred parallel steps because the plan runner is a durable state machine with one
current step. Running several steps at once risks two failures:
- a step sent after the plan has decided to stop;
- an undo that runs while its branch is still moving.

This ADR adds parallel steps without changing how any single step is checked, sent,
recorded, approved, decided or recovered.

## Decision

A plan opts in with `"parallel": true`. Without it nothing changes.

### Which steps run together

The runner picks a batch:

1. It starts with the first ready step in declared order. A step is ready when it is
   pending and every step in its `after` has succeeded.
2. It adds every later ready step whose claim, computed from the capability graph exactly
   as leases do (the devices it acts on, everything they contain, and every device sharing
   one of their resources), does not overlap a claim already in the batch.
3. A step that policy would hold for approval never joins a batch. If the first ready step
   would be held, or fewer than two steps qualify, the plan runs one step as before.

### How a batch runs

1. The plan's lease is renewed once.
2. Each step's dependency-skip, `when` and precondition checks run one at a time. A check
   failure is an ordinary step failure; if it stops the plan, **nothing in the batch is
   sent**.
3. The remaining steps' commands are submitted at the same time, each an ordinary command
   with every check, keyed and journaled as any plan step.
4. **The runner waits for every one of them to settle** before reacting.
5. Each result is recorded as a forward result: succeeded, failed, uncertain or held. The
   plan's reaction is then decided once, in this order:
   - **any uncertain outcome:** the plan waits for a person (`AWAITING_DECISION`), and
     nothing is compensated by itself;
   - **else a held step:** the plan waits for its approval. Only one command is ever held
     per plan; a second is withdrawn (cancelled, which sends nothing) and its step goes
     back to pending;
   - **else a failure:** the first failed step in declared order applies its `on_failure`.
     Compensation then undoes every succeeded step, siblings included, newest first, after
     all of them have stopped;
   - **else:** the plan carries on.

### Recovery

After a restart, every step that was in flight is settled, not only the plan's current one.
The plan waits for a person, as before.

## Consequences

- A plan runs faster when its steps use separate devices, and never runs two steps on one
  device or resource at once.
- The order steps start in may differ from the declared order within a batch. Ordering
  that matters must be stated with `after`.
- A held command can be withdrawn when an uncertain or failed sibling decides the plan's
  reaction. That is always safe, since a held command has not been sent.

## Tests

`crates/openmhs-engine/tests/parallel_steps.rs`:
- in virtual time, two 2 s steps on separate devices finish together with `parallel` and
  in sequence without it;
- two steps on one device run in sequence even with `parallel`;
- a step that needs approval runs alone;
- when one branch fails, the other branch's command settles first and is then compensated;
- an unknown outcome on one branch stops the plan for a person without undoing the other.
