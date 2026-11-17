# ADR-0036: A reported outcome may carry the values it was judged on (shape rules `shape-2`)

- Status: Accepted
- Date: 2026-11-17
- Builds on: [ADR-0033](0033-outcome-provenance.md), [ADR-0035](0035-failure-shape.md)

These rules were written and committed before the shape was computed on the policy demo's events. The episodes' peak
rewards were already known (they are in that demo's results page), so this is not a blind test of what the shape will
show; it fixes how the shape treats a measurement before the code that applies it exists.

## Context

The failure shape (ADR-0035) measures how far a goal missed only from a *verified* verdict's conditions. Some systems
judge their own outcome against a number and report it, with no independent check: a simulator's success flag is
"covered area over 95 %", a test bench reports "pressure within limits". In the PushT policy demo all 13 failed
episodes show as "task ended failed", although eleven peaked just under the success threshold and one barely moved the
block. The amount is in `ext`, which the shape never reads. Two failures that differ by a factor of ten in how far they
got have the same shape.

Calling such a measurement *verified* would be wrong: the system that executed is the one that judged (ADR-0033). It is
a reported outcome, and the values behind it are the reporter's.

## Decision

### 1. `outcome.reported.conditions` (RFC 0005 addition)

An event with `outcome.reported` may carry `outcome.reported.conditions`, in exactly the form of
`outcome.verified.conditions` (ADR-0035, section 2): `name`, `evidence`, `op`, bounds, `result`, `observed`. The
producer copies what the reporter judged on. The store checks the form as for verdicts and refuses a list that
contradicts the reported value in one direction only: a report of `succeeded` with a `false` condition. A report of
`failed`, `aborted` or `timeout` may have every condition `true` (a task can fail for a reason none of its conditions
covers), and nothing else is inferred from the list.

### 2. What the shape does with them (`shape-2`)

- A point that is a failed task or a failed action (ADR-0035, section 3) carries the misses of its *reported*
  conditions, measured by the same rule as a verdict's (the smallest distance outside the bound over the observed
  values; numeric comparisons only; one unit).
- Every miss names where it comes from: `reported` or `verified`. The summary of misses is kept per source and per
  condition, never pooled across the two.
- The headline says *reported* before such a miss. A verified miss reads as in `shape-1`.
- In the signature, a reported condition is keyed `reported:<evidence>` where a verified one is `<contract>:<evidence>`.
- Everything else is `shape-1` unchanged. On events that carry no reported conditions, `shape-2` gives the same shape
  as `shape-1` apart from the `rules` field and the source on each verified miss.

### 3. What it does not do

It does not make a reported outcome verified, does not count a reported miss in goal verdicts, spread or the
reported-against-verified disagreements, and does not feed a release statistic. The magnitude of a miss is not part of
the signature: binning a continuous distance would need a threshold this ADR has no basis for. Two failures with the
same signature and different distances are listed with their distances.

## Alternatives considered

- **Emit the simulator's judgement as `outcome.verified`.** Rejected: it is the executor judging itself (ADR-0033).
- **Read the amount from `ext`.** Rejected: `ext` is never interpreted (RFC 0005, ADR-0035).
- **Bin distances into "near miss" and "far miss" in the signature.** Rejected for now (section 3).

## Consequences

- A simulated or self-judged failure can say by how much it failed, labelled as the system's own report.
- Readers must keep the two sources apart; the output says which is which on every miss.
- Every stored shape changes its `rules` field on rebuild.

## First run, 2026-11-17 (recorded after the rules, not used to change them)

On the policy demo's 40 episodes (importer fills each episode's peak reward against 1, with the simulator's own success
flag as the result): the 13 failed episodes each carry one reported miss, below the bound, and no verdict. Eleven peaked
within 0.04 of a reward of 1 (0.0003 to 0.0386), `ddim-10` seed 1014 at 0.29 short, `ddpm-100` seed 1001 at 0.90
short. All 13 share the signature `pusht/task_failed/-/reported:reward` (8 under `ddim-10`, 5 under `ddpm-100`), as
section 3 says they would: the distance, not the signature, separates a near miss from no movement.

One formatting change was made after seeing the output, not a rule change: a miss of 0.0003 printed as "0.000", so a
distance under 0.001 now prints with enough decimals for its first significant digit and one more ("0.00030"). On the
CoreSense flights this is the only visible difference: one headline's first miss reads "0.00065 m" where it read
"0.001 m". Otherwise only the `rules` field and the source on each miss changed.
