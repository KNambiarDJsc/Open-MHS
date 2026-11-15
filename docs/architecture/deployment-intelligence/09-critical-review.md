# Critical review, product and business

## 1. Where this direction comes from, and what the evidence says so far

This is the repository's third framing in a month: a governed physical tool runtime (M0-M9), an independent outcome
"evidence plane" (RFC 0004, 2026-11-10), and now deployment intelligence. The evidence gathered so far is sobering
and relevant:

- No external user runs Open-MHS. Every verified result is from simulators, emulators or a replayed public recording.
- The outcome verifier matched a short script on clean data in both experiments; it only won when evidence was
  mislabelled or self-confirming (`research/outcome-verifier/ros2-evaluation/RESULTS*.md`).
- The same experiments did show something a release dashboard would get wrong: a ROS 2 action reporting `SUCCEEDED`
  40 times when the lift reached its target 17 times.

The proposal keeps that finding (ADR-0033) and does not discard the earlier work. It does not remove the need to find
users. Every phase gate below is about users, not features.

## 2. The ten questions

**1. Why would a team use Open-MHS instead of Foxglove?** They should not *instead*; Foxglove (and Roboto) are where
engineers look at recordings, and both now sell AI triage. The only reason to add Open-MHS is questions those tools do
not answer from a recording alone: which deployment manifest was running, what changed since the last success on this
robot, and whether release B is worse than A with a defensible denominator, with reported and verified outcomes kept
apart. If Foxglove ships release-aware outcome statistics, this reason weakens sharply.

**2. Why wouldn't this be built inside Formant or InOrbit?** It could be, and partly is (Formant analytics, InOrbit
incidents, now OpenRobOps under Apache-2.0). Their incident features serve fleet operators watching live robots.
Open-MHS's angle is engineering after the fact: release attribution, evidence packages, training handoff, working from
files without a fleet platform. A team already on InOrbit may prefer an OpenRobOps plugin; Open-MHS should be able to
export to it rather than compete.

**3. The smallest feature with independent value?** `openmhs ops compare` over imported recordings and deployment
manifests: "pick failures, release A vs B, with intervals, confounds and the incidents behind the numbers". The
timeline alone is weaker, because Foxglove and Roboto already give good timelines.

**4. What if no customer wants a new edge agent?** The design does not need one (ADR-0027). Import and SDK spool files
cover Stage 1. The collector is conditional.

**5. Can it work by importing existing MCAP files and deployment metadata?** Yes; that is the first path. Its limits:
task ids often do not exist in recordings (synthetic tasks by rule, marked as such), and deployment metadata must come
from CI or the robot. Without it, release intelligence is impossible.

**6. The differentiator beyond a dashboard?** Three properties, all checkable: (a) outcome provenance (reported,
verified, unknown, never merged); (b) statistics that refuse to overclaim (exposure denominators, intervals, strata,
`insufficient_evidence`); (c) deterministic, explained correlation with stable ids and rebuilds. Honestly, these are
*qualities*, not features, and qualities are hard to sell. The bet is that robotics teams burned by misleading status
and noisy dashboards value them.

**7. Abstractions likely to become obsolete?** `policy.chunk` (action representations are changing fast), the starter
failure taxonomy, any world-model record, and possibly the task/action split for end-to-end policies with no explicit
actions. Kept small or in `ext` for that reason. Likely to last: event identity, time with uncertainty, deployment
manifests, artifacts by reference, interventions, outcomes with provenance.

**8. Data impossible to infer without explicit instrumentation?** Task boundaries and task types (unless a topic
carries them), intent (what was supposed to happen), deployment manifests, operator identity and intervention reasons,
dataset episode references, and any independent verification of outcomes. Recordings show behaviour, not intent.

**9. What can the system never know with certainty?** The physical outcome, unless an independent measurement exists
and is labelled honestly; cause; complete ordering across unsynchronized clocks; whether missing events never happened
or were lost; whether a robot's reported manifest is what actually ran; anything a compromised robot chose to report.

**10. What would make us abandon or narrow this?** See §5. In short: no team shares data within 60 days; teams that
try it prefer Foxglove/Roboto's AI triage and see no value in release comparisons; or an incumbent ships release-aware
outcome statistics first.

## 3. Build / integrate / defer / reject

| Capability | Decision | Reason |
|---|---|---|
| Event envelope and manifests (RFC 0005) | **Build** | The core; small; nothing equivalent is open and vendor-neutral |
| Import from MCAP + manifests | **Build** | First path to value; reuses research code |
| Deterministic correlation, timelines | **Build** | Needed for release attribution and handoffs |
| Release statistics with intervals | **Build** | The candidate differentiator |
| Handoff packages, LeRobot selections | **Build** | Cheap; links ops to learning |
| Python SDK spool | **Build** (P4) | Task ids and intent need instrumentation |
| Outcome verification (RFC 0004) as `outcome.verified` | **Build** (exists) | Distinguishes reported from verified |
| Visualization of recordings | **Integrate** (Foxglove, Rerun, Roboto) | Mature tools exist |
| AI triage / summaries | **Integrate** (Roboto, Foxglove agents) or later optional, labelled `generated` | Others lead; risk of overclaiming |
| ROS fault management | **Integrate** (`ros2_medkit`, `/diagnostics`) | Exists |
| Fleet dashboards, teleoperation, remote ops | **Integrate** (Formant, InOrbit/OpenRobOps, Viam) | Exists; not the wedge |
| OpenTelemetry | **Integrate** (carry ids; OTLP ingest later) | Standard for services |
| Edge collector | **Defer** until a partner needs live ingestion | ADR-0027 |
| Hosted multi-tenant cloud | **Defer** to Stage 2 triggers | No users yet |
| Column store, queue | **Defer** to Stage 3 triggers | ADR-0029 |
| Embedding similarity, causal inference | **Defer** / **Reject** for the core | Overclaiming risk (ADR-0030) |
| World-model first-class types | **Defer** | Representations unstable |
| Robot control inside the observation layer | **Reject** | ADR-0026. Governed execution stays the gateway's opt-in extension, never a dependency |
| Training framework, dataset storage | **Reject** | LeRobot and others own it |
| A universal robotics ontology | **Reject** | ADR-0028 |

## 4. Product and business

### Users and buyer

| | Who | Daily workflow touched |
|---|---|---|
| Initial user | Autonomy, deployment and robotics-infrastructure engineers at companies with roughly 10-300 robots in customer sites (mobile manipulation, warehouse AMRs, lab automation) | After a field failure: find the recording, find what was deployed, reconstruct the timeline, decide whose bug it is, hand it to the owning team or the ML team |
| Second user | ML engineers on VLA/imitation policies | Pick failure and intervention episodes for evaluation or fine-tuning; check whether a new checkpoint did better in the field |
| Buyer | Head of robotics infrastructure / VP engineering | Release confidence and engineering time spent on incidents |

### Deployment friction, and how the design lowers it

Security review of anything on the robot (none needed for import); data leaving the site (local-first, artifacts by
reference); integration effort (mapping files instead of code); one more tool to look at (export into Foxglove,
Roboto, InOrbit and ticket systems).

### Competitors and alternatives

Foxglove, Roboto, Sift (data and triage); Formant, InOrbit/OpenRobOps, Viam (fleet operations); `ros2_medkit`
(diagnostics); in-house Grafana + Sentry + spreadsheets (the most common, and the real competitor).

### Why add Open-MHS

Measured by: time from incident report to complete evidence package (target: from hours to minutes, to be measured
with partners); share of incidents with full deployment context (target > 90 % once manifests flow); time to a
defensible release verdict, and how often a "regression" verdict survives a second look.

### Open source and paid boundaries (hypotheses)

| Open source (Apache-2.0) | Possible paid offering |
|---|---|
| Event schema, SDKs, importers, local store, CLI, correlation, statistics, handoff, local web page, MCP tools | Hosted or supported self-hosted multi-site service: SSO, retention policies, cross-site comparisons at scale, alerting on release regressions, integrations maintenance, support |

Rule: a team of ten robots must get the full investigation and release-comparison experience free and offline.

Pricing hypotheses to test, not to announce: hosted per robot per month in the $20-60 range (fleet tools are commonly
priced per robot; unverified for this category); self-hosted annual licence per site; paid pilots for design partners
to test willingness to pay.

### Potential design partners (types)

Robotics startups 6-24 months into customer deployments; open hardware robot projects with active simulation stacks;
university labs running shared robot fleets; lab-automation groups already near Open-MHS's gateway work. Named
organizations stay out of this document until they agree.

### Validation criteria

| By | Criterion |
|---|---|
| Day 30 | 10 problem interviews; at least 5 describe a recent incident where deployment context or release attribution was missing and cost hours |
| Day 60 | 2 teams share real data; 1 imported and reviewed with them |
| Day 90 | 1 team uses it on its own data in two separate weeks or reports a measured time saving; at least 1 team says what it would pay |

## 5. Kill or narrow criteria

- **Narrow to a library** (schema + importer + statistics, no service) if teams want the analysis inside Foxglove,
  Roboto or their own notebooks rather than a new UI.
- **Narrow to ML handoff** if engineers value intervention/episode selection but not incident timelines.
- **Stop** if by day 90 no team has shared data, or two teams that tried it say their current tools already answer the
  release question.
