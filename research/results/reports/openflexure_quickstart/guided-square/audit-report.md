# Experiment report

Generated 2026-10-30T11:03:03+0530.

> A view of the gateway's journal. It is only as trustworthy as the journal; the audit chain detects alteration and reordering of the records present, not removal of the newest ones (compare the anchor) and not a gateway that lied. It records the device's reports, not the physical truth.

## Audit chain

- records: **52**, chain valid: **True**
- anchor (keep this somewhere the gateway's writer cannot reach): seq 52, hash `433e025441db5770…`

## Device contracts in force

- seq 2 `ofm.stage` adapter `openmhs.py.wot` descriptor sha256 `097a631e82122538`
- seq 51 `ofm.stage` adapter `openmhs.py.wot` descriptor sha256 `097a631e82122538`

## Commands

### `942715124d57` ofm.stage / move_x → **VERIFIED**

- asked by `agent:guided-square` (credential `[redacted]`); invoke {"x": 1}
- authorization: allow by ['guided-square-x']
- device's own reference: `labthings:1a89ead5-8b7a-4cf8-befd-d636584f127a`
- lifecycle: CREATED (05:31:17.316) → VALIDATING (05:31:17.318) → APPROVED (05:31:17.32Z) → DISPATCHED (05:31:17.323) → ACCEPTED (05:31:17.354) → APPLIED (05:31:17.495) → VERIFIED (05:31:17.626)
- verification: readback; expected 1, observed 1 at 2026-10-30T05:31:17.626Z (simulator), matched True

### `3dd18decd944` ofm.stage / move_y → **VERIFIED**

- asked by `agent:guided-square` (credential `[redacted]`); invoke {"y": 1}
- authorization: allow by ['guided-square-y']
- device's own reference: `labthings:d661dfc1-fffb-471d-80cb-2bc3a30ee5be`
- lifecycle: CREATED (05:31:24.148) → VALIDATING (05:31:24.153) → APPROVED (05:31:24.156) → DISPATCHED (05:31:24.161) → ACCEPTED (05:31:24.191) → APPLIED (05:31:24.322) → VERIFIED (05:31:24.466)
- verification: readback; expected 1, observed 1 at 2026-10-30T05:31:24.466Z (simulator), matched True

### `98d2906ecd23` ofm.stage / move_x → **VERIFIED**

- asked by `agent:guided-square` (credential `[redacted]`); invoke {"x": 0}
- authorization: allow by ['guided-square-x']
- device's own reference: `labthings:4c5b866c-abd4-4f7a-9980-9298ffe07085`
- lifecycle: CREATED (05:31:29.328) → VALIDATING (05:31:29.333) → APPROVED (05:31:29.337) → DISPATCHED (05:31:29.342) → ACCEPTED (05:31:29.353) → APPLIED (05:31:29.51Z) → VERIFIED (05:31:29.67Z)
- verification: readback; expected 0, observed 0 at 2026-10-30T05:31:29.669Z (simulator), matched True

### `4f5b21a85b45` ofm.stage / move_y → **VERIFIED**

- asked by `agent:guided-square` (credential `[redacted]`); invoke {"y": 0}
- authorization: allow by ['guided-square-y']
- device's own reference: `labthings:71dbcd94-f241-4980-8f2c-a22926c36123`
- lifecycle: CREATED (05:31:35.058) → VALIDATING (05:31:35.063) → APPROVED (05:31:35.064) → DISPATCHED (05:31:35.067) → ACCEPTED (05:31:35.103) → APPLIED (05:31:35.258) → VERIFIED (05:31:35.387)
- verification: readback; expected 0, observed 0 at 2026-10-30T05:31:35.387Z (simulator), matched True
