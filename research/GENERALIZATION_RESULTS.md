# Generalization: an unseen, independently described device

Phase 9. Does the importer, adapter and OpenMHS toolchain work on a device it was not built
around? **Yes for the mechanics, with the same two caveats as before: the description does not
carry the safety facts, and a reviewer can switch the safety check off.** Simulator only; no
physical hardware was involved.

Data: [`results/generalization.json`](results/generalization.json), produced by
[`generalization/run_generalization.py`](generalization/run_generalization.py). Re-run it with
`hotplate_server.py --port 5200` up.

## The device and why it counts

A stirring hotplate (`generalization/hotplate_td.json`, 73 lines), described in the style of the
W3C Thing Description 1.1 examples, and served by a 126-line stand-in (`hotplate_server.py`). It
differs from OpenFlexure on purpose:

| | OpenFlexure simulator | Hotplate |
|---|---|---|
| Server | LabThings-FastAPI | hand-written stand-in, not LabThings |
| Authentication | none | HTTP Basic (`lab` / `bench`) |
| Units | none declared | `Cel`, `min`, `rpm` declared |
| Action parameters | rarely typed | `temperature`, `minutes` with ranges |
| Safety flags | none declared | `safe` / `idempotent` declared on every action |
| Events | none | one (`overheat`) |
| Long-running work | LabThings invocations | plain synchronous POST |

**Honesty about independence.** I wrote this description and this server. They were written before
the importer was run on them and in the standard's style rather than the importer's, but a
description written by the same person who wrote the importer is weaker evidence than a vendor's.
It is a test of generality across *shapes*, not a survey of real devices.

## What happened, stage by stage

| Stage | Result |
|---|---|
| Import with no review | 4 properties exposed read-only; 1 action exposed (`selfTest`, declared `safe`); 3 actions held for review; 1 event unsupported; Basic auth recognised, credentials taken from the environment |
| Import with a 3-action review file | 4 properties, 4 actions exposed. `stirRate`'s unit was not declared (`unit_unknown`) and `targetTemperature` stayed unexposed for writing |
| Live read | `temperature` returned a `Quantity(67.0 Cel)`, source `simulator` |
| `selfTest` (TD-declared safe) | **UNVERIFIABLE**: executed, but nothing declares how to confirm it, so OpenMHS will not say "verified" |
| `setProgram` 40 Cel | **VERIFIED**: the target was read back and matched. Three seconds later the plate read 61.5 Cel (it was cooling toward 40) |
| `setProgram` 500 Cel | **DENIED by OpenMHS** before the device saw it: outside 20..300 Cel, a range that came from the TD |
| `stop` | **UNVERIFIABLE** (same reason as `selfTest`) |
| Wrong password | device reported **OFFLINE**, reason "the Thing rejected the credentials (HTTP 401)" |
| Credential leak check | the password was absent from the manifest, the gateway log and the audit trail; the manifest names the variables only |

### What the review file added

Three entries, each carrying a judgement the description could not: `side_effect`, a time bound and,
for `setProgram`, a readback (`target_temperature`, tolerance 0.01). Everything else (names, types,
units, ranges, idempotency, security scheme) was generated from the description.

## The stress test: the same device, described badly

The same description with units, ranges, titles and `safe`/`idempotent` stripped out:

| | Complete description | Incomplete description |
|---|---|---|
| Properties exposed | 4 | 4 |
| Actions exposed with no review | 1 | **0** |
| Actions exposed after the same review file | 4 | **0**: all four wait for a reviewer to supply idempotency |
| Units | declared | `unit_missing` on 3 properties |
| `setProgram` 500 Cel | **DENIED** by a TD-supplied range | see below |

With a *completed* review (idempotency restored, and the two numeric parameters declared
`unbounded_ok`), `setProgram` was exposed. A request for **500 Cel then went through**: OpenMHS
forwarded it, this stand-in's `setProgram` does not check its own range, the readback matched, and
the command ended `VERIFIED`. Nothing in the chain stopped it, and the audit trail says so
faithfully.

This is the important result. OpenMHS enforces the limits it is *given*. The importer refuses to
invent them and makes omitting them an explicit, named decision (`unbounded_ok`), but a reviewer who
makes that decision has turned the check off, and "VERIFIED" then means only that the device
agrees with what it was told. The defence is the review step being visible, not it being
infallible.

## Does it generalize?

| Claim | Evidence | Verdict |
|---|---|---|
| The mapping rules apply to a device with different auth, units, and server | the hotplate mapped and ran live, but it was also what exposed bugs 5-7 in WOT_IMPORTER_RESULTS.md (quantity wrapping, 401 reported healthy, silent events/security); this run is the re-run after those fixes | **Supported after fixes, n = 2 device families** |
| A description with real safety flags needs less review | 1 of 4 actions exposed unreviewed, vs 0 of 50 on OpenFlexure | **Supported, but small**: the flag allowed one read-only-in-practice action |
| The importer fails closed on a thin description | 0 actions exposed on the stripped description | **Supported** |
| The pipeline would work on arbitrary vendor descriptions | not tested | **Not shown** |
| Events | `overheat` reported unsupported, not mapped | **Gap** |

## Limits

- One additional device, authored by me. A real survey (public TD catalogues, other LabThings
  deployments) would very likely turn up shapes this does not cover: other security schemes,
  nested schemas, `forms` with multiple operations, non-JSON content types.
- Synchronous actions only on this device; asynchronous behaviour is covered in
  [ASYNC_ACTION_RELIABILITY.md](ASYNC_ACTION_RELIABILITY.md).
- Nothing here concerns a physical hotplate or any safety property of one.
