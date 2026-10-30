# Decision memo: OpenMHS × LabThings × OpenFlexure

For the OpenMHS maintainer. Everything below is from **simulators**; no physical hardware was
used and no hardware claim is made. Nothing has been proposed upstream or sent to anyone. The
detail behind each answer is in the numbered documents in this directory.

This memo began as a snapshot on the `research/labthings-wot` branch and contains historical
statements that were superseded after that snapshot. In particular, the three agent-facing issues
listed in question 1(d-f) and decision 2 below are now addressed on `main`; use the current
repository code and [COLLABORATION_BRIEF.md](COLLABORATION_BRIEF.md) for the present status. The
OpenFlexure quickstart ran successfully against the simulator, a LabThings 0.3.0 in-memory probe
verified unit metadata in generated descriptions, and GPT-6 Luna completed both an approval-gated
move and one autonomous-in-simulation move. None is physical validation.

## The ten questions

**1. What did OpenFlexure reveal about OpenMHS's weaknesses?**
It was the independent device that exposed them (CURRENT_CAPABILITIES.md and the phases below).
In OpenMHS: (a) the device's own reference for a command was not in the hash-chained audit, and
the contract in force was not identified (both fixed, tested); (b) the hash chain does not cover the
mutable receipt, a rewrite that recomputes hashes passes, and truncation of the newest records is
invisible without an off-gateway anchor (AUDIT_AND_PROVENANCE.md; the export now cross-checks
receipts and supports an anchor, the rest is open); (c) a stale-reading rule that depends on
measurement time breaks when the device gives none (mitigated, with a stated one-second blind
spot); (d) tool descriptions once contained a concrete example id (`sim.axis-01`); (e)
`get_insights` on an unknown device once returned an empty success; (f) there was no tool to inspect
effective access. These three agent-facing gaps are now addressed on `main`. (g) the value model
cannot express nullable or compound values (23 of 106 properties unsupported).

**2. Which OpenMHS features proved useful?**
Measured, not asserted: the unresolved-outcome hold plus reconcile-by-readback (15 of 15 correct
recoveries; no resend in six ways of losing contact); idempotency keys (one job instead of two);
limits enforced before dispatch (the out-of-limit target was refused 15 of 15 times; the bench
itself runs it); deny-by-default and approval (every scripted proposal refused or held, the stage
moved only after approval); the stale-reading rule (when its precondition held); the journal limit
(448 commands, 448 jobs, then refusal with nothing sent); plans stopping at an uncertain step;
and descriptor validation, which caught 3 of the importer's 7 bugs before any device saw them.

**3. What does LabThings solve better than OpenMHS?**
Describing a device from its own code: docstrings, types, schemas and affordances arrive in a
Thing Description with no hand-writing. Its asynchronous invocation model (`201`, a pollable
resource, status, log, output) is sound and OpenMHS builds on it rather than duplicating it. It
also has WebSocket events, which the importer does not yet use. OpenMHS reconciles by readback and
does not ask LabThings for the invocation's status; LabThings is the better authority on
whether *its own action* finished, for as long as it remembers (a few minutes, lazily expired).

**4. Can Thing Descriptions reduce integration work?**
Partly, and the part it cannot reduce is the part that matters. Transcription was generated: 83
capabilities and about 1,790 lines of descriptor JSON across the 14 OpenFlexure Things. Judgement
was not: none of the 50 actions declares `safe` or `idempotent`, no property carries a unit, so
all 50 actions stayed behind review (44 override lines for one bounded stage move). I did not
measure time and make no speed claim; what was counted is decisions (WOT_IMPORTER_RESULTS.md).

**5. Can natural-language agents work reliably with unfamiliar lab devices?**
**Still not shown generally.** The 32-run study used a 3B local model that could not use either
tool layer correctly and asserted state, freshness and permissions it had not observed. Later
constrained GPT-6 Luna runs both read x=0, chose x=1 and passed preflight; one completed after
operator approval, and one did so autonomously under a one-action simulator-only guard. They
demonstrate control of this simulator through MCP but do not establish reliable operation across
tasks or devices (NATURAL_LANGUAGE_EXPERIMENTS.md and OPENFLEXURE_SIMULATOR_QUICKSTART.md).

**6. Does OpenMHS add reliability, or duplicate what LabThings has?**
It adds a different thing. It does not duplicate invocation tracking. It adds duplicate
suppression across retries, enforced limits, authorization and approval, honest "unknown"
outcomes, and a record. It did **not** make naive clients finish more work: they finished fewer
runs (5 and 10 of 45, against 35 for plain HTTP) because they stop where they cannot be sure; a
client that heeds the receipts and reconciles finished 20 of 45 with no duplicates. A careful
hand-written client matched OpenMHS on duplicates (RELIABILITY_BENCHMARK.md). Reliability here is
a trade of completion for certainty.

**7. What is the evidence?**

| Claim | Evidence | Strength |
|---|---|---|
| Acceptance ≠ completion ≠ verified; no resend after any loss of contact | ASYNC_ACTION_RELIABILITY.md, 6 loss-of-contact scenarios, ground truth from the server | Moderate (one server, simulated) |
| Naive retry duplicates work; gateway removes it | benchmark: 15 of 45 vs 0 (and 15 again for a client that changes keys) | Moderate; n = 5 per cell |
| Timestamp-less reads can make a verifying system infer the wrong thing; Date header helps to ~1 s | S12 family, F6/F6b | Moderate |
| TDs omit safety facts | 0 of 50 actions, 0 units on OpenFlexure's descriptions | Strong for this server set |
| The audit chain detects edits and deletions in the middle only | P2-P5 | Strong for the cases run |
| Agents are more accurate through OpenMHS | **no** | Not tested by these simulator smoke runs |
| Anything on hardware | none | None |

**8. What reusable integration would help LabThings?**
Not the importer, which is OpenMHS code. What is reusable: the fault-injection proxy and example
`LabBench` Thing for integrators to test their own clients; the benchmark harness; and four
documentation-level observations (UPSTREAM_OPPORTUNITIES.md), the smallest being a missing `f` in
three error strings. None has been proposed.

**9. What is the strongest research opportunity?**
*What do device descriptions leave out, and what does it cost to supervise a device safely
anyway?* It rests on the measurement that is already solid (what a real LabThings description omits),
the staleness finding, and a benchmark others can run; it needs more third-party descriptions,
one physical device in someone else's hands, and a literature survey to be a paper
(PUBLICATION_ASSESSMENT.md). The natural-language question is the more interesting one and the least
supported today.

**10. What should be shared with Dr Bowman first?**
Only with your approval, and nothing yet has been. If you decide to: the two-page
COLLABORATION_BRIEF.md and the example evidence report
(`results/reports/multi_device_R2_report.md`), because audit logging was his stated interest. Hold
back the natural-language results (weak) and the benchmark (a trade, which needs context). The
brief asks two questions and claims no collaboration, endorsement or hardware validation.

## Decisions for you

1. **Repository state.** The active branch is `main`; research artifacts and the latest model
   smoke run are being added directly here, consistent with the maintainer's workflow.
2. **Agent-facing correctness fixes.** Placeholder-free tool descriptions, an unknown-device
   error, and an effective-access query are implemented on `main`.
3. **Contact anyone?** The brief is a draft. Whether, when and by whom it goes out is yours.
4. **The OpenFlexure fixture** (116 KB of captured simulator output; GPL-3.0 software's output):
   ship, regenerate in CI, or drop.
5. **A broader capable-model study** remains open; the one-shot OpenFlexure run does not answer
   the reliability question.

## State of the work

Branch `main`. Core: `external_ref` on acceptance, the
reference chained into the audit, the contract hash at registration; engine tests pass. Importer
and adapter: `adapters/python/wot/` (25 offline tests). Harness: `research/experiments/`. Results:
`research/results/`. The OpenFlexure simulator and dedicated local Ollama process used for early
model attempts are stopped; no physical device was accessed.
