# Experiment report

Generated 2026-10-30T10:48:09+0530.

> A view of the gateway's journal. It is only as trustworthy as the journal; the audit chain detects alteration and reordering of the records present, not removal of the newest ones (compare the anchor) and not a gateway that lied. It records the device's reports, not the physical truth.

## Audit chain

- records: **18**, chain valid: **True**
- anchor (keep this somewhere the gateway's writer cannot reach): seq 18, hash `0de116bdfaed14e3…`

## Device contracts in force

- seq 2 `ofm.stage` adapter `openmhs.py.wot` descriptor sha256 `9b41549bf05db9c5`

## Commands

### `ab9590bd2a6c` ofm.stage / move_x → **VERIFIED**

- asked by `agent:demo` (credential `tok_6c8460ca4578f6f3`); invoke {"x": 1}
- authorization: allow by ['demo-stage-approval']
- device's own reference: `labthings:da43fc3d-ca89-4f79-a826-d47a2443e0de`
- lifecycle: CREATED (05:18:05.432) → VALIDATING (05:18:05.437) → APPROVED (05:18:05.44Z) → DISPATCHED (05:18:05.442) → ACCEPTED (05:18:05.525) → APPLIED (05:18:05.659) → VERIFIED (05:18:05.815)
- verification: readback; expected 1, observed 1 at 2026-10-30T05:18:05.815Z (simulator), matched True
