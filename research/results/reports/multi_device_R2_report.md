# Experiment report

Generated 2026-10-29T19:22:01+0530.

> A view of the gateway's journal. It is only as trustworthy as the journal; the audit chain detects alteration and reordering of the records present, not removal of the newest ones (compare the anchor) and not a gateway that lied. It records the device's reports, not the physical truth.

## Audit chain

- records: **53**, chain valid: **True**
- anchor (keep this somewhere the gateway's writer cannot reach): seq 53, hash `d04988faf8fc33bd…`

## Device contracts in force

- seq 2 `lab.hotplate` adapter `openmhs.py.wot` descriptor sha256 `5b41aebe64facefe`
- seq 3 `ofm.stage` adapter `openmhs.py.wot` descriptor sha256 `77f761deb490481e`

## Plan `8b19c96cab6e` by `agent:lab`: **SUCCEEDED**

- steps: heat SUCCEEDED, move SUCCEEDED, stop SUCCEEDED
- decision: {"at": "2026-10-29T13:52:01.535Z", "by": "operator:local", "decision": "resume", "reason": "experiment operator"}

## Commands

### `dd408f2474fc` lab.hotplate / set_program → **VERIFIED**

- asked by `agent:lab` (credential `tok_d3d5d4a8773cd11a`); invoke {"minutes": {"unit": "min", "value": 1.0}, "temperature": {"unit": "Cel", "value": 40.0}}
- authorization: allow by ['heat']
- device reference: none recorded
- lifecycle: CREATED (13:51:57.312) → VALIDATING (13:51:57.313) → APPROVED (13:51:57.315) → DISPATCHED (13:51:57.317) → ACCEPTED (13:51:57.32Z) → APPLIED (13:51:57.321) → VERIFIED (13:51:57.441)
- verification: readback; expected {'unit': 'Cel', 'value': 40.0}, observed {'unit': 'Cel', 'value': 40.0} at 2026-10-29T13:51:57.44Z (simulator), matched True

### `b8a40311ec03` ofm.stage / move_x → **RECONCILED_APPLIED**

- asked by `agent:lab` (credential `tok_d3d5d4a8773cd11a`); invoke {"x": 50}
- authorization: require_approval by ['move'], approved by operator:local
- device's own reference: `labthings:1b386c2f-2f6c-49c3-ab18-cc62cf52d2ed`
- lifecycle: CREATED (13:51:57.448) → VALIDATING (13:51:57.449) → AWAITING_APPROVAL (13:51:57.451) → APPROVED (13:51:57.593) → DISPATCHED (13:51:57.595) → ACCEPTED (13:51:57.642) → UNKNOWN_OUTCOME (13:51:57.777) → RECONCILED_APPLIED (13:52:01.032)
- verification: readback; expected 50, observed 50 at 2026-10-29T13:52:01.032Z (simulator), matched True
- device error: lost contact with invocation 1b386c2f-2f6c-49c3-ab18-cc62cf52d2ed (http://127.0.0.1:56197/api/v3/action_invocations/1b386c2f-2f6c-49c3-ab18-cc62cf52d2ed) after it was accepted (Remote end closed connection without response); its outcome is unknown

### `fb9c35241d5c` lab.hotplate / stop → **UNVERIFIABLE**

- asked by `agent:lab` (credential `tok_d3d5d4a8773cd11a`); invoke {}
- authorization: allow by ['heat']
- device reference: none recorded
- lifecycle: CREATED (13:52:01.538) → VALIDATING (13:52:01.54Z) → APPROVED (13:52:01.541) → DISPATCHED (13:52:01.543) → ACCEPTED (13:52:01.559) → APPLIED (13:52:01.561) → UNVERIFIABLE (13:52:01.563)
- verification: none; expected None, observed None at None (None), matched None; no readback is declared in the Thing Description or the review
