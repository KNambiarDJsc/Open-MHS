# Current capabilities: OpenMHS, LabThings-FastAPI and OpenFlexure

Phase 1 of the LabThings / OpenFlexure interoperability study. Every row cites the file it was
read from. Nothing here is a claim about physical hardware: all behaviour was observed against
simulators (see [EXPERIMENT_ENVIRONMENT.md](EXPERIMENT_ENVIRONMENT.md)).

| Project | Version / commit inspected | Licence |
|---|---|---|
| OpenMHS | branch `research/labthings-wot` (v0.1.0 + unmerged work) | Apache-2.0 |
| LabThings-FastAPI | `761441e`, 0.3.0 + 11 commits (2026-10-28) | MIT (`LICENSE`) |
| OpenFlexure Microscope Server | `01d0dd3`, v3.0.0-beta1 + 154 commits (2026-10-28), "v3 development branch" | GPL-3.0 (`LICENSE`) |

The three are different layers. **LabThings** is a framework for writing a *Thing* (a device
server) and serving it over HTTP with a W3C Thing Description. **OpenFlexure** is a microscope
server built on it. **OpenMHS** is a supervisory gateway that sits between an agent and any device
server, and decides, journals and verifies. They overlap on action handling and logging, and
complement each other on authorization, uncertainty and audit.

## 1. What LabThings-FastAPI already does (do not re-invent)

| Capability | How it works | Source |
|---|---|---|
| W3C Thing Description | Generated per Thing from the class; served for all Things at `GET /thing_descriptions/` and per Thing. `@context`, `properties`, `actions`, `forms`, `securityDefinitions`, `base` | `server/__init__.py:479-499`, `thing.py:417-418`, `thing_description/_model.py` |
| Properties | Typed (pydantic), `readOnly`/`writeOnly`, `minimum`/`maximum` carried through. Observable over a WebSocket per Thing | `properties.py`, `thing.py:217-219`, `websockets.py` |
| **Asynchronous actions** | `POST /<thing>/<action>` returns **201** and an *invocation* resource (`id`, `status`, `href`, `timeRequested/Started/Completed`, `input`, `output`, `log`, `error`). Statuses `pending`, `running`, `completed`, `cancelled`, `error`. Poll `GET /action_invocations/{id}`; output at `/{id}/output` | `actions.py:945-1006`, `invocations.py:26-37,90-120` |
| Wait-before-reply | The `201` is held up to `response_timeout` (default 1 s) so fast actions often return already finished | `actions.py:770-811` |
| Retention | Finished invocations kept `retention_time` (default 300 s), then expired; the store is an in-memory dict | `actions.py:423,770-811` |
| **Cancellation** | `DELETE /action_invocations/{id}`: 200 "Cancel request sent" if pending/running, 404 unknown, 503 if already finished. **Cooperative:** the action must use `lt.cancellable_sleep` / a cancel hook; otherwise nothing stops | `actions.py:243-251,628-668`, `docs/source/actions.rst:104` |
| Error reporting | RFC 9457 Problem Details; an action that raises ends `error` with `error` populated; invalid input is 422 before any invocation exists | `problem_details.py`, `actions.py:379-407` |
| Per-invocation log | Log records from the action are captured with the invocation id and returned in the invocation | `logs.py`, `invocations.py:LogRecordModel` |
| Validation | Pydantic models for input and output; extra inputs refused unless the action takes `**kwargs` | `actions.py` (`input_model`), observed |

## 2. What LabThings and its Thing Descriptions do **not** provide

Each of these was checked in the source or observed, not assumed.

| Gap | Evidence |
|---|---|
| **No authentication or authorization.** Every Thing is `security: no_security` (`nosec`) | `thing.py:417-418`; the OpenFlexure TDs all show it |
| **No idempotency or de-duplication.** Two identical POSTs run twice | `grep idempotent` finds it only in the TD model, never set or read by the framework; observed in [ASYNC_ACTION_RELIABILITY.md](ASYNC_ACTION_RELIABILITY.md) S11 (two jobs) |
| **`safe` and `idempotent` are never populated** although the TD schema has the fields | `thing_description/_model.py:282-283`; none of OpenFlexure's 50 actions sets them |
| **No units** in practice: the TD `unit` field exists, but none of OpenFlexure's 95 properties declares one | counted from the captured descriptions (`research/fixtures/`) |
| **No physical-effect or hazard metadata** (what an action does to the world, limits other than JSON-schema min/max) | absent from TD output |
| **No measurement timestamp on a property read** (a read returns a bare value) | observed: `GET /stage/position` → `{"x":0,"y":0,"z":0}` |
| **No tamper-evident record.** Logs are in-process (per-invocation, expiring) and, for OpenFlexure, a rotating file | `invocations.py`; `openflexure_microscope_server/logging.py:30-65` |
| **Actions can take undeclared keyword arguments** the TD does not list | OpenFlexure `stage.move_absolute`: TD lists `block_cancellation`, `backlash_compensation`; the axes `x`, `y`, `z` are `**kwargs` (`additionalProperties: true`) |
| **Not every value is representable** in a flat contract: nullable (`oneOf` with `null`), arrays, free objects | 12 nullable and 15 compound of OpenFlexure's 95 properties (27 unsupported in all; see WOT_IMPORTER_RESULTS.md) |
| **Events**: the TD can describe them; none of OpenFlexure's 14 Things declares one | counted: 0 events |

## 3. What OpenFlexure's server adds

- 14 Things in simulation mode (`ofm_config_simulation.json`): camera, stage, autofocus, gallery, camera/stage mapping, illumination, system, smart scan and three workflows, two background-detect Things, timelapse. Totals: **95 properties, 50 actions, 0 events**.
- A simulated camera (`things/camera/simulation.py`) and a dummy stage (`things/stage/dummy.py`). The dummy stage models motion as timed steps with `time.sleep` (lines ~130-148). **It models no optics, no focus physics, no mechanical limits** beyond what the code encodes.
- A rotating-file log (`logging.py`) and settings stored per Thing.
- A web application, served from `static/`, which must be downloaded separately (`pull_webapp.py`) before the server starts. The server refuses to start without it.
- Tests: `tests/unit_tests`, `tests/integration_tests`, `tests/lifecycle_test`, `tests/hardware_specific_tests`.

## 4. What OpenMHS already does

| Capability | Where |
|---|---|
| Command lifecycle with `ACCEPTED`/`APPLIED`/`VERIFIED` kept distinct, and `UNKNOWN_OUTCOME` after a timeout, disconnect or restart; never retried | `spec/lifecycle.md`, `crates/openmhs-types/src/lifecycle.rs`, `crates/openmhs-engine/src/engine.rs` (`run`, `dispatch`, `verify`, `reconcile`) |
| New non-idempotent writes to a capability refused while an earlier one is unresolved | `engine.rs` (unresolved hold), `crates/openmhs-engine/tests/unresolved.rs` |
| Readback verification; a reading must postdate the command (ADR-0020) | `engine.rs`, `crates/openmhs-engine/tests/stale_readback.rs`, `docs/adr/0020-readback-must-follow-dispatch.md` |
| Idempotency keys (same key, same body → same receipt) | `crates/openmhs-engine/src/journal.rs` (`insert_command`) |
| Deny-by-default grants, scoped credentials, single-use approvals | `crates/openmhs-policy/src/{authz,auth}.rs`, `crates/openmhs-engine/src/engine/approvals.rs` |
| Hash-chained audit and command journal (SQLite) | `crates/openmhs-engine/src/journal.rs` (`append_audit`, `verify_audit`) |
| Device availability and recovery | `crates/openmhs-engine/src/engine/availability.rs`, ADR-0019 |
| Plans (sequential, conditional, opt-in parallel) with leases and compensation | `crates/openmhs-engine/src/engine/plans.rs`, ADR-0018/0024/0025 |
| MCP server with structured refusals | `crates/openmhs-mcp/src/lib.rs` |
| Out-of-process Python adapter SDK and host | `python/openmhs/adapter.py`, `crates/openmhs-host` |
| Conformance levels with negative controls | `crates/openmhs-conformance`, `spec/conformance.md` |

### OpenMHS before this study: what it could not do with a LabThings server

- **Its HTTP profile adapter is declarative and has no notion of an asynchronous job.** A `write`/`action` is one request; it cannot follow an invocation resource to completion (`docs/adapters/http.md`, "No polling or push").
- **No way to remember what the device called the work.** Acceptance was a bare event with no identifier (`crates/openmhs-adapter/src/lib.rs`, `ProgressEvent::Accepted`), so a LabThings invocation id could not be kept in the journal. *Changed by this study:* an optional `external_ref` (see ASYNC_ACTION_RELIABILITY.md).
- **Readback freshness assumes the adapter supplies a measurement time.** LabThings reads carry none (see ASYNC_ACTION_RELIABILITY.md, S12).
- **No description import.** Devices were described by hand-written profiles or descriptors.
- **Single-valued verification.** A readback names one capability, so a three-axis position cannot be verified by one `move_absolute`.

## 5. Interoperability matrix

"Direct" is a client talking to LabThings with no OpenMHS. "Via OpenMHS" is the same Thing behind the generated adapter built in this study. ✔ present, ◐ partial, ✘ absent.

| Concern | Direct LabThings | Via OpenMHS | Notes |
|---|---|---|---|
| Machine-readable interface | ✔ TD 1.1 | ✔ descriptor (imported from the TD) | OpenMHS descriptors carry units, limits, side effects, verification; the TD usually does not |
| Discover what exists | ✔ `/thing_descriptions/` | ✔ `list_devices`, `find_capabilities` | |
| Read a value | ✔ | ✔ audited, with freshness metadata | LabThings gives no measurement time (Section 2) |
| Observe changes | ✔ WebSocket | ✔ `watch_state`/`poll_events` (sampled) | Different mechanisms; OpenMHS samples via reads |
| Start an action asynchronously | ✔ 201 + invocation | ✔ same underlying call; OpenMHS tracks the command | |
| Know completion vs acceptance | ✔ `status` | ✔ `ACCEPTED` → `APPLIED` → `VERIFIED` | |
| Cancel | ◐ cooperative request | ✘ deliberately not offered | A cancel request is not a stop; see S04b |
| Authentication / authorization | ✘ | ✔ | LabThings relies on the deployment (network, proxy) |
| Deny unknown actions | ✘ | ✔ deny by default; unreviewed actions not exposed | |
| Idempotent resubmission | ✘ | ✔ idempotency keys | |
| Resubmission after uncertainty | caller's problem | ✔ blocked until reconciled | |
| Readback verification | caller's problem | ✔ declared in the descriptor | |
| Stale-reading protection | ✘ | ◐ depends on a trustworthy reading time | Mitigation tested: HTTP `Date` header |
| Per-invocation log | ✔ | ✘ not copied into the journal | A possible addition; see PROVENANCE doc |
| Tamper-evident audit | ✘ | ✔ hash chain (with limits; see PROVENANCE doc) | |
| Scientific provenance model | ✘ | ✘ | Neither is one; see AUDIT_AND_PROVENANCE.md |

## 6. Where the two are complementary

- **LabThings is the better source of truth for what a device offers**: its descriptions are generated from the code that runs, so they cannot drift. OpenMHS should consume them, not duplicate them.
- **LabThings already solves asynchronous action handling well.** The invocation resource, statuses, retention and cooperative cancellation are sound. OpenMHS's contribution is *around* it: who may call it, what happens when the caller loses contact, and what counts as proof of an effect.
- **OpenMHS adds what LabThings deliberately leaves to the deployment**: authorization, a durable decision record, and conservative handling of uncertainty.
- **Neither verifies physical state by itself.** Only a measurement independent of the command can.

## 7. Limits of this audit

- Only the HTTP binding was inspected. LabThings' WebSocket stream was read, not exercised.
- OpenFlexure was run in **simulation** only. Its camera and stage hardware classes were not read in depth.
- The comparison reflects commits of 2026-10-28; OpenFlexure v3 is a development branch and will change.
