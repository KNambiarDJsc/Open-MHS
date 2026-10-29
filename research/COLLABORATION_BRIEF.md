# OpenMHS and LabThings: a simulator-only study, and two questions

*Draft for the OpenMHS maintainer to review. Not sent to anyone.*

## Why this exists

Dr Richard Bowman (University of Glasgow) expressed interest in OpenMHS's audit logging and in
natural-language interaction with hardware, and suggested testing against the OpenFlexure
simulator and W3C Web of Things descriptions. This note summarises what was done in response, what
was found, and what is **not** claimed. It is not a proposal for a collaboration and does not
assume one.

## What OpenMHS is

A gateway between software agents (including language-model agents) and hardware. Every command is
checked against a device's declared limits and a deny-by-default permission model, optionally held
for a human approval, sent through an adapter, verified by reading the device back where it can, and
recorded in a hash-chained audit trail. It reports "I do not know" rather than guess when contact
is lost. Pre-alpha; Apache-2.0.

## What was done

Everything over HTTP, against **simulators only**: the OpenFlexure server in its simulation
configuration, an independent LabThings example Thing, and a hand-written hotplate description.
No LabThings or OpenFlexure code was modified, copied or vendored. No hardware was used.

- A generic importer turns a W3C Thing Description into an OpenMHS device contract, failing closed:
  an action is not exposed until a person states its side effect, limits and timing.
- A set of failure scenarios and a 225-run fault-injection benchmark against LabThings asynchronous
  actions, with ground truth read from the server.
- A provenance study of the audit trail, and a two-device workflow with an injected fault.
- A natural-language experiment with a small local model, which was too weak to show anything
  about agents.

Everything is reproducible from the repository's `research/` directory.

## What was found that may be of interest to LabThings

1. **Action affordances never say whether they are safe or idempotent.** The Thing Description
   model has the fields; none of the 50 actions in the OpenFlexure simulator sets them. An importer
   can therefore expose none of them without a human.
2. **Property reads carry no measurement time.** A cached response is indistinguishable from a
   fresh one. In one scenario this led a verifying gateway to conclude "the move did not happen"
   about a move that had; the HTTP `Date` header helps, with a blind spot of about a second.
3. **Finished invocations are removed lazily**, only when another action is invoked: one was still
   served 330 s after finishing, with 300 s documented. Expired and unknown invocations both return
   `404`, and the message contains a literal `{id}` (a missing `f` in the string, three places in
   0.3.0).
4. **Cancelling an action that never checks for cancellation returns 200 "Cancel request sent"**
   while the action runs on. Documented as cooperative; it surprised the integrator.

All four were measured on `labthings-fastapi` 0.3.0 and have not been checked against newer code;
any may already be fixed. None is an OpenFlexure defect.

## What was found about OpenMHS

The study found and fixed gaps in OpenMHS itself: the device's own reference and a hash of the
contract in force were not in the chained audit record; an error after acceptance was once
misclassified as "nothing was sent"; and some of its tool descriptions use example identifiers a
small model copies. It also showed what the audit trail does not protect against: a rewrite that
recomputes the hashes, or removal of the newest records without an externally held anchor.

## What is not claimed

- No hardware validation, no compatibility claim for a physical OpenFlexure microscope, no safety
  or certification claim.
- No endorsement or involvement by Dr Bowman, the University of Glasgow, LabThings or OpenFlexure.
- No claim that OpenMHS makes language-model agents more accurate; in this study it did not.
- The benchmark is a trade, not a win: OpenMHS removed duplicate executions and out-of-limit
  dispatch, and a careful hand-written client matched it on duplicates.

## Two questions

1. **Is there a convention, or appetite, for declaring `safe`/`idempotent` on LabThings actions and
   for stamping property reads with a measurement time?** If one exists we would follow it; if
   not, we would like to know whether it is something the maintainers would rather see raised as a
   discussion than as a patch. We will not propose a change without that guidance.
2. **For audit logging and natural-language interaction, what would a minimally useful trial look
   like?** For example, a read-only session against the OpenFlexure simulator in which you decide
   which records must be capturable (who, what, when, under what authorization, what evidence)
   and tell us what is missing. Nothing beyond the simulator, and nothing that moves a stage
   without an explicit, bounded approval.

## What we could offer, without commitment

The fault-injection harness and the example Thing, the importer, and the benchmark, for anyone who
wants to test their own integration against faults.
