# ADR-0032: Release comparisons use exposure denominators and intervals, and may answer "insufficient evidence"

- Status: Accepted (reviewed 2026-11-15/16)
- Date: 2026-11-15

## Decision

1. The unit is a **task attempt** of one task type, attributed to the deployment manifest active on that robot when
   the attempt **started**. The denominator is attempts started, not incidents, not robots.
2. Rates of events that are not per attempt (interventions, faults) use **exposure time** (robot-hours under the
   manifest) as the denominator.
3. Outcomes are counted in four bins: succeeded, failed, unknown, excluded (with a reason). The headline rate is
   failed / (attempts − unknown). Unknowns are **reported and bounded**, never dropped: the comparison also shows the
   rate with unknowns counted as successes and as failures.
4. **The unit of independence is the scenario, not the attempt.** When attempts carry a `scenario_id` and some scenario
   is attempted more than once, comparisons count scenarios (each judged by the majority of its attempts; a tie is
   unknown), say so in a warning, and apply the sample-size rules to scenarios. `--per-attempt` gives the other count
   and labels it. Without scenario ids the tool cannot tell replicates from independent trials and says nothing: this
   is a known limit.
5. **Paired comparison (`--paired`).** When both releases attempted the same scenarios, each scenario can be compared with
   itself: the scenarios are matched by `subject.scenario_id`, each is judged in each arm as in rule 4, and the table of
   (fails in both, only in a, only in b, in neither) gives McNemar's exact two-sided test over the scenarios that differ
   and Newcombe's paired interval (method 10: the two Wilson intervals combined through the correlation the pairs show)
   for the difference. When the same scenarios fail in both arms the interval is narrower than treating the arms as
   independent samples; when they do not overlap it is the same. Scenarios present in only one arm, or without a known
   outcome in one of them, are named and left out; a paired comparison is not stratified by site. The sample-size rules
   apply to matched pairs.
6. **Time is reported twice, on purpose.** *Duration* is the span between the task's own start and end events, in the
   events' own clock; in a simulation that is simulated time. *Wall time* (`cost.wall_s`, when the producer reports it)
   is what the attempt cost the executing system to run. A change that makes episodes twice as fast to compute and no
   different in simulated length shows only in wall time. Both are given as a median and a 90th percentile, interpolated
   between ranks (the median of an even count is the mean of the two middle values), per attempt. Wall time is the
   producer's claim and is comparable across arms only on the same hardware and load; the store does not check this.
7. Every rate carries a **Wilson 95 % interval**; a difference between two releases carries a **Newcombe interval**.
   A Beta-Binomial posterior probability of regression may be shown alongside, labelled as such.
8. A result is `insufficient_evidence` when either arm has fewer than 30 attempts or fewer than 5 expected failures,
   or the interval of the difference spans both "no change" and the configured regression margin.
9. Comparisons are **stratified** by site and task type by default (Cochran-Mantel-Haenszel pooled estimate when
   strata are compatible), and report the strata mix, because a rollout that reached the easy site first is the most
   common confounder.
10. A continuously refreshed comparison is labelled as such; automated "regression" flags use a pre-declared margin and
   minimum sample, and say that repeated looks inflate false alarms (always-valid methods are deferred).

## Alternatives considered

- **Failure counts per day**: wrong denominator when traffic changes. Rejected.
- **p-values only**: invite peeking and say nothing about size. Rejected as the primary output.

## Consequences

- Release intelligence needs task-attempt events with start times; deployment events alone are not enough.
- Small fleets will often see `insufficient_evidence`. That is the honest answer and the product must say it well.
