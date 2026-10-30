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
- A historical natural-language study with a small local model, which was too weak to show
  agent reliability. A later constrained GPT-6 Luna run read the OpenFlexure simulator stage,
  chose x=1, passed preflight and completed after operator approval; see the quickstart evidence.
- A live OpenFlexure simulator walkthrough in
  [OPENFLEXURE_SIMULATOR_QUICKSTART.md](OPENFLEXURE_SIMULATOR_QUICKSTART.md): x=50 finished
  `VERIFIED`; then an injected lost poll during a bounded x=-50 request was reconciled from
  readback as `RECONCILED_APPLIED`. Both audit chains verified. The provider was deterministic,
  so this establishes the OpenMHS control path, not language-model tool-use quality. Reports and
  traces are in `results/reports/openflexure_quickstart/`.

Everything is reproducible from the repository's `research/` directory.

## What was found that may be of interest to LabThings

The current LabThings-FastAPI source already models WoT `DataSchema.unit` and action
`safe`/`idempotent` fields. A runtime probe against LabThings-FastAPI 0.3.0 confirmed that
Pydantic `Annotated`/`Field(json_schema_extra={"unit": "mm"})` metadata reaches generated TDs
for data and functional properties and action inputs; OpenMHS imported `mm` as `length/mm`.
The useful upstream question is whether documenting that existing pattern would help authors.
See [WOT_UNITS_PROPOSAL.md](WOT_UNITS_PROPOSAL.md) and
[`experiments/labthings_units_probe.py`](experiments/labthings_units_probe.py).

The older reliability observations in this repository were measured against LabThings-FastAPI
0.3.0. They are historical and have not all been re-run against the current release; they are
not presented here as current defects.

## What was found about OpenMHS

The study found gaps in OpenMHS itself, and the current `main` code addresses the agent-facing
ones: tool descriptions no longer seed a concrete device id, `get_insights` reports an unknown
device as an error, and `get_allowed_operations` exposes the caller's current advisory access.
The device reference and contract hash are included in audit evidence, and post-acceptance
failures remain distinct from commands that were never sent. The audit work still documents its
limits: a rewriter who recomputes hashes, or truncation of the newest records without an
externally held anchor, cannot be detected by the local chain alone.

## What is not claimed

- No hardware validation, no compatibility claim for a physical OpenFlexure microscope, no safety
  or certification claim.
- No endorsement or involvement by Dr Bowman, the University of Glasgow, LabThings or OpenFlexure.
- No claim that OpenMHS makes language-model agents more accurate; the single successful
  simulator task does not compare agent accuracy or establish reliability.
- The benchmark is a trade, not a win: OpenMHS removed duplicate executions and out-of-limit
  dispatch, and a careful hand-written client matched it on duplicates.

## Two questions

1. **Which existing OpenFlexure user workflows would most benefit from agent control?**
2. **Would documenting the existing unit metadata path for LabThings' machine-readable
   descriptions be useful, and what documentation approach would fit the project?**

## What we could offer, without commitment

The fault-injection harness and example `LabBench` Thing, the importer, the benchmark, and the
OpenFlexure simulator walkthrough, for anyone who wants to inspect or reproduce the integration.
The in-memory units serialization probe passed; an HTTP endpoint-level check remains optional.
This brief remains a draft and has not been sent.
