# Upstream opportunities (for discussion, not for submission)

Phase 10. Which findings, if any, could be of use to LabThings-FastAPI or OpenFlexure, ranked by
how strong the evidence is and how small the change would be.

**Nothing here has been proposed to anyone.** No issue, pull request or message has been sent to
the LabThings or OpenFlexure projects or to any researcher, and none should be without the
maintainer's approval. Every item is written so it can be reviewed by the person who owns that
decision first. All evidence is from simulators on one machine; "LabThings" below means
`labthings-fastapi` 0.3.0 as installed for these experiments and "OpenFlexure" means the server's
`v3-dev` simulation configuration. Findings were not checked against newer upstream code, so any
of them may already be fixed.

## Ranking

| # | Candidate | Where | Evidence | Size | Fit |
|---|---|---|---|---|---|
| 1 | Let an action declare `safe` / `idempotent` so they appear in its Thing Description | LabThings | 0 of 50 OpenFlexure actions declare either; the importer can expose 0 of them without a human | small: two optional decorator arguments feeding fields the TD model already has | high: the fields exist in the W3C TD and in LabThings' own TD model, which never fills them |
| 2 | Fix the literal `{id}` in the 404 detail for an unknown invocation | LabThings | The message is `No action invocation found with ID {id}`: the `f` is missing, in 3 places in `actions.py` (lines ~561, ~604, ~654 of 0.3.0) | one character ×3 | a nit, but a clean one |
| 3 | Document that finished invocations are removed lazily and that 404 means "expired, restarted or never existed" | LabThings docs | An invocation was still served 330 s after finishing (retention is 300 s) and disappeared only after *another* action was invoked | docs | good: every integrator who polls late hits it |
| 4 | Document that property reads carry no measurement time, and what an intermediary can do to that | LabThings docs | S12/S12b (ASYNC_ACTION_RELIABILITY.md): a cached response is indistinguishable from a fresh one, and the only available mitigation is the HTTP `Date` header at 1 s resolution | docs | good, and a prerequisite for anyone doing verified writes |
| 5 | State in the docs that `DELETE` on an invocation is a request, not a stop | LabThings docs | S04b: `200 Cancel request sent`, carriage kept moving to its end | docs | the behaviour is documented in the code; the integrator-facing consequence is not obvious |
| 6 | Optional `unit` on properties | LabThings | 0 of 83 imported capabilities carry a unit from the description; 37 numeric ones fall back to dimensionless | medium: touches the property API | plausible; a larger design conversation |
| 7 | A reusable fault-injection proxy and example `LabBench` Thing for integrators | a separate repository, not upstream code | running the integration against real servers found 7 importer bugs and one misclassification in the adapter that reading the code did not (WOT_IMPORTER_RESULTS.md, ASYNC_ACTION_RELIABILITY.md) | medium | useful as an offer, not an ask |
| 8 | A `labthings` → OpenMHS adapter generated from a TD | OpenMHS's own repository | importer + adapter, 25 offline tests | already done | not an upstream contribution: it is OpenMHS code that talks HTTP to LabThings and copies none of it |

### The smallest contribution with the strongest evidence

**Candidate 2, then 1.** The `{id}` nit is a verified defect that can be shown in one `curl`, and
is correct without any argument about design. Candidate 1 has the strongest evidence of *value*
(it is the difference between an importer that exposes nothing and one that exposes the actions a
reviewer cares about), and the change is the smallest of the design-level items, but it is a design
decision about the LabThings API and belongs to its maintainers. The right first step for it is a
question, not a patch (see COLLABORATION_BRIEF.md).

## What each one rests on

**1. `safe` / `idempotent`.** LabThings' TD model has the fields (`thing_description/_model.py`)
and never sets them. In the OpenFlexure simulator's 14 Things none of the 50 actions declares
either; the importer therefore treats all of them as needing review. On a description that does
declare them (the hand-written hotplate) one of four actions could be exposed without review
(GENERALIZATION_RESULTS.md). Caution: a developer who marks an action `safe` has made a claim a
consumer will rely on. The importer deliberately maps `safe` to "reversible" and never to
"no effect", because a call that merely *reads* can still move a stage on a bad day.

**2. The `{id}` message.** Reproduce: `GET /action_invocations/00000000-0000-0000-0000-000000000000`
returns 404 with that literal text. Cosmetic. It also matters slightly for integrators: the same
404 is returned for "never existed" and "expired", and an adapter cannot tell them apart, so a
client of LabThings that loses contact can only answer "unknown" (S07 in ASYNC_ACTION_RELIABILITY.md).
A distinct status (for example 410 for expired) would be a feature request, not a bug, and is not
proposed here.

**3. Lazy expiry.** `expire_invocations` runs as a background task after each *new* action POST
(`actions.py`), not on a timer. Measured: a completed invocation was still `200` at 330 s with the
documented retention of 300 s, and `404` after one further action was invoked. The effect on an
integrator is that the window in which an invocation can be looked up depends on how busy the
server is. For OpenMHS the consequence is limited, because it reconciles by readback and records the
invocation id at acceptance (so a person can still search the server's own logs), but a client that
planned to ask LabThings "what happened to invocation X" an hour later would sometimes be able to
and sometimes not.

**4. Timestamp-less property reads.** See ASYNC_ACTION_RELIABILITY.md §6. This is the finding with
the most safety weight: a reading replayed by a cache made a verified-write system conclude
"not applied" and invite a resend. The mitigation (`Date` header) works with a stated blind spot of
about a second. What would help is a convention, even a documentation-level one, for where a
measurement time lives. That is a design question with trade-offs (every property response would
need to carry it) and should be put to the maintainers as a question.

**5. Cancellation.** The behaviour is correct and documented as cooperative. The consequence is
not obvious to an integrator: `200 "Cancel request sent"` followed by a carriage that keeps
moving. OpenMHS's response was to never cancel on its own and to say so in the receipt. A line in
the user guide would save the next integrator from having to find this out.

**6. Units.** A unit on `position`, `exposure_time` and similar would let an importer produce a
quantity capability rather than a dimensionless one. This is the largest of the "ask" items and
entangles with how LabThings models properties; it is listed for completeness, not recommended as
a first approach.

## What OpenMHS learned that should change OpenMHS (and has)

These do not need anyone's permission and are not upstream suggestions:

- The **device's own reference** for a command (here the LabThings invocation id) is now recorded
  at acceptance, in the hash-chained audit trail as well as the receipt. It survived a gateway kill
  and recovery in S10.
- The **hash of the capability contract in force**, and for an imported device the hash of the
  Thing Description it came from, is recorded when a device registers (AUDIT_AND_PROVENANCE.md).
- A device-side failure after acceptance is never reported as "not sent".
- Reads that have no measurement time can use the server's `Date` header, with a stated limit.

## What should not be proposed

- **Authentication, authorization, approval or audit inside LabThings.** Those are deployment
  concerns; LabThings is a device-side framework and the point of OpenMHS is to sit outside it.
  Proposing otherwise would duplicate OpenMHS inside a project that did not ask for it.
- **Idempotency keys in LabThings.** A key in an action POST would be a large change to a wide API
  for a benefit that a supervising layer already provides.
- **Anything in OpenFlexure.** The simulator's `DummyStage` has no limits and no optics (EXPERIMENT_ENVIRONMENT.md);
  those are properties of a simulator. Nothing found here is an OpenFlexure defect.
- **Anything that implies the projects endorse OpenMHS, or that OpenMHS is validated against real
  OpenFlexure hardware.** Neither is true.
