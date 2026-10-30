# Experiment report

Generated 2026-10-30T10:23:39+0530.

> A view of the gateway's journal. It is only as trustworthy as the journal; the audit chain detects alteration and reordering of the records present, not removal of the newest ones (compare the anchor) and not a gateway that lied. It records the device's reports, not the physical truth.

## Audit chain

- records: **21**, chain valid: **True**
- anchor (keep this somewhere the gateway's writer cannot reach): seq 21, hash `9832a072ad1deaff…`

## Device contracts in force

- seq 2 `ofm.stage` adapter `openmhs.py.wot` descriptor sha256 `6c9ca131e9cfce3c`

## Commands

### `d1eaa41a64a0` ofm.stage / move_x → **VERIFIED**

- asked by `agent:demo` (credential `tok_b78d147afa00a1ba`); invoke {"x": 1}
- authorization: require_approval by ['demo-stage-approval'], approved by operator:local
- device's own reference: `labthings:8b21cfc4-a3a7-4857-984d-c86f2ddc3559`
- lifecycle: CREATED (04:53:26.975) → VALIDATING (04:53:26.982) → AWAITING_APPROVAL (04:53:27.01Z) → APPROVED (04:53:38.699) → DISPATCHED (04:53:38.703) → ACCEPTED (04:53:38.732) → APPLIED (04:53:38.89Z) → VERIFIED (04:53:39.042)
- verification: readback; expected 1, observed 1 at 2026-10-30T04:53:39.042Z (simulator), matched True
