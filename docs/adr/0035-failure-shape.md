# ADR-0035: The shape of a failure is computed from typed fields, never from text

- Status: Accepted
- Date: 2026-11-17
- Builds on: [ADR-0030](0030-correlation-not-causation.md), [ADR-0033](0033-outcome-provenance.md),
  [RFC 0004](../rfcs/0004-outcome-contracts-and-evidence.md), [RFC 0005](../rfcs/0005-deployment-events.md)

These rules were written and committed before they were run on any recorded data, so that what they show on the
CoreSense flights could not shape them.

## Context

An incident says *that* something went wrong: its triggers (`reported_failed`, `reported_verified_disagree`,
`intervention`, ...) and a timeline of member events. It does not say *how*: when in the task, in which part, by how
much, once or every time, after what. A reader has to work that out from the timeline by hand.

Three ways to produce the "how":

1. **A language model reads the timeline and writes an explanation.** Fluent, but not reproducible, not testable, and
   it will offer a cause the data does not contain.
2. **Patterns over log text.** Brittle, vendor-specific, and the text is in `ext`, which core rules never interpret.
3. **Compute it from typed fields the events already carry**, plus one new optional field: the measured values behind
   a verdict.

## Decision

The *shape* of an incident is computed by deterministic, versioned rules (`shape-1`) from typed fields only, rebuilt
with the incidents, and stored beside them. Same events and rule version, same shape, byte for byte.

### 1. Inputs

The incident's member events (ADR-0030): their `type`, time, clock domain and uncertainty, `attributes.component`,
`attributes.category`, `attributes.code`, goal and action ids, `outcome.reported.value`, `outcome.verified.value`, and
`outcome.verified.conditions` (section 2). Never `ext`, never free text. A window-joined member keeps its basis wherever
it appears in the shape.

### 2. The measured values behind a verdict (RFC 0005 addition)

An `outcome.verified` event may carry `outcome.verified.conditions`: one entry per contract condition, with the
condition's `name`, `evidence`, `op`, its bounds (`min`, `max`, `value`, `values`, exactly as in the RFC 0004
contract), the `result` (`true`, `false`, `undetermined`) and the `observed` values the verifier used. The producer
copies them from the verifier's report; the store checks their form and refuses a malformed list.

### 3. What the shape contains

- **Points.** Every member that marks something not going as reported, in time order: a verdict `violated`, an action
  that ended `failed`, `aborted`, `timeout` or `canceled`, a `fault.raised`, an `intervention.started`, a task that
  ended `failed`, `aborted` or `timeout`. Each with its offset from the task's start in seconds and as a fraction of the
  task's duration (when both ends are known), its component, ids and code.
- **The first point, and whether its place is known.** The first point *precedes* the second only when their times
  differ by more than both uncertainties on one clock (unknown uncertainty on one clock compares directly, as in
  RFC 0004). On two clocks without stated uncertainties the order is unknown, and the shape says so instead of
  picking one.
- **Goals.** Verdicts per component: satisfied, violated, unknown. The spread: `one`, `some`, or `all` of the verified
  goals violated. The position of the first violated goal among the verified goals in time order (k of n).
- **Misses.** For each failing condition (by contract and condition name), how many goals failed it, on which side of
  the bound (`above` or `below`), and by how much: for each goal, the smallest distance outside the bound over the
  records the verifier used (a condition is false only when every record fails it, so the goal missed by *at least*
  that much), summarized as min, median and max over goals, with the unit. Only numeric comparisons have a distance;
  `eq`, `ne` and `in` are counted. Values with different units are not combined.
- **Reported against verified, per goal.** Goals reported `succeeded` and verified `violated`; goals reported failed
  and verified `satisfied`.
- **A signature.** `task type / first point kind / component / failing condition names`, a facet key explainable field
  by field (ADR-0030, rule 4). Recurrence is counted by signature at read time: where it was first seen, and how many
  incidents carry it per release.
- **A headline.** One sentence assembled from a fixed template over the fields above. Its vocabulary is temporal and
  structural (*first*, *before*, *after*, *during*, *of*, *outside by*). It contains no causal word (*because*,
  *caused*, *due to*, *led to*, *root cause*); a test enforces it.

### 4. What the shape does not say

Why. A cause is an annotation by a person, or a hypothesis with `provenance: generated` that cites the event ids it
used (ADR-0030, rule 5). A narrator that turns the shape into prose may come later; it reads the shape, not the raw
events, and what it writes is labelled generated and never feeds a statistic.

Domain words (*overshoot*, *stall*, *slip*) are not computed by the store, which knows no domains. A producer that knows
them says so in the condition's name ("distance past the waypoint, along the path"), and the shape repeats the name.

### 5. Absence is stated

No conditions on a verdict: the shape says the verdict carries no measured values. No goals: the shape has task-level
points only. No points at all (an incident opened by an unknown report alone): the shape says nothing went wrong that
the events can place.

## Alternatives considered

- **LLM-first explanation.** Rejected for the core for the reasons in the context; allowed later as a labelled narrator
  over the shape (section 4).
- **Patterns over log text.** Rejected: `ext` is verbatim and uninterpreted by design (RFC 0005).
- **Domain-specific shapes in the store** ("overshoot along the path"). Rejected: needs a model of each robot's task.
  The producer can name it; the store reports it.
- **Computing the shape at read time only.** Rejected: listing and grouping incidents by shape would recompute every
  shape on every read. It is derived and stored like the incidents, and rebuilt with them.

## Consequences

- An incident can read "first point at 41 % of the task: goto goal 3 of 11 verified violated, y above the bound by at
  least 0.31 m; 9 of 11 goto goals violated; a fault 2.1 s before", and every number traces to an event id. (An
  illustration of the form, not a result.)
- Producers that want misses measured must carry the verifier's conditions on the verdict. Events grow by the observed
  values (bounded by the 64 KiB event limit).
- The shape is only as precise as the clocks: the "first" point is withheld when the clocks cannot order it.
