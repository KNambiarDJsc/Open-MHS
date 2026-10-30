# Experiment report

Generated 2026-10-30T09:55:09+0530.

> A view of the gateway's journal. It is only as trustworthy as the journal; the audit chain detects alteration and reordering of the records present, not removal of the newest ones (compare the anchor) and not a gateway that lied. It records the device's reports, not the physical truth.

## Audit chain

- records: **30**, chain valid: **True**
- anchor (keep this somewhere the gateway's writer cannot reach): seq 30, hash `1761e0d132825b45…`

## Device contracts in force

- seq 2 `ofm.stage` adapter `openmhs.py.wot` descriptor sha256 `6c9ca131e9cfce3c`
- seq 23 `ofm.stage` adapter `openmhs.py.wot` descriptor sha256 `6c9ca131e9cfce3c`
- seq 26 `ofm.stage` adapter `openmhs.py.wot` descriptor sha256 `6c9ca131e9cfce3c`

## Commands

### `dd42911bfc0f` ofm.stage / move_x → **VERIFIED**

- asked by `agent:demo` (credential `tok_5a27581fa2dc8e9d`); invoke {"x": 50}
- authorization: require_approval by ['demo-stage-approval'], approved by operator:local
- device's own reference: `labthings:e7661ee0-28d5-4d59-9ebe-1154d2c44ce0`
- lifecycle: CREATED (04:22:10.008) → VALIDATING (04:22:10.01Z) → AWAITING_APPROVAL (04:22:10.014) → APPROVED (04:22:10.029) → DISPATCHED (04:22:10.032) → ACCEPTED (04:22:10.082) → APPLIED (04:22:10.224) → VERIFIED (04:22:10.358)
- verification: readback; expected 50, observed 50 at 2026-10-30T04:22:10.357Z (simulator), matched True
