# Experiment report

Generated 2026-10-29T19:36:18+0530.

> A view of the gateway's journal. It is only as trustworthy as the journal; the audit chain detects alteration and reordering of the records present, not removal of the newest ones (compare the anchor) and not a gateway that lied. It records the device's reports, not the physical truth.

## Audit chain

- records: **22**, chain valid: **True**
- anchor (keep this somewhere the gateway's writer cannot reach): seq 22, hash `bc2338131e988ca9…`

## Device contracts in force

- seq 2 `lab.bench` adapter `openmhs.py.wot` descriptor sha256 `0a37ec49badaabc4`

## Commands

### `5abff1f5af13` lab.bench / move_to → **VERIFIED**

- asked by `agent:exp` (credential `tok_baf34232b27d2763`); invoke {"fail_after": {"unit": "s", "value": -1.0}, "seconds": {"unit": "s", "value": 0.2}, "target": 10}
- authorization: allow by ['w']
- device's own reference: `labthings:8a6d2e63-d687-464e-afad-fd1def7190a4`
- lifecycle: CREATED (14:06:17.753) → VALIDATING (14:06:17.755) → APPROVED (14:06:17.757) → DISPATCHED (14:06:17.759) → ACCEPTED (14:06:17.767) → APPLIED (14:06:18.149) → VERIFIED (14:06:18.28Z)
- verification: readback; expected 10, observed 10 at 2026-10-29T14:06:18.279Z (simulator), matched True

### `37cb9aded253` lab.bench / move_to → **VERIFIED**

- asked by `agent:exp` (credential `tok_baf34232b27d2763`); invoke {"fail_after": {"unit": "s", "value": -1.0}, "seconds": {"unit": "s", "value": 0.2}, "target": 20}
- authorization: allow by ['w']
- device's own reference: `labthings:5eb5e7d1-e2d4-4519-a965-760095361e6f`
- lifecycle: CREATED (14:06:18.284) → VALIDATING (14:06:18.285) → APPROVED (14:06:18.287) → DISPATCHED (14:06:18.288) → ACCEPTED (14:06:18.297) → APPLIED (14:06:18.648) → VERIFIED (14:06:18.781)
- verification: readback; expected 20, observed 20 at 2026-10-29T14:06:18.781Z (simulator), matched True
