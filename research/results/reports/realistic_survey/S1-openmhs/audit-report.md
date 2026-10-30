# Experiment report

Generated 2026-10-30T18:20:29+0530.

> A view of the gateway's journal. It is only as trustworthy as the journal; the audit chain detects alteration and reordering of the records present, not removal of the newest ones (compare the anchor) and not a gateway that lied. It records the device's reports, not the physical truth.

## Audit chain

- records: **161**, chain valid: **True**
- anchor (keep this somewhere the gateway's writer cannot reach): seq 161, hash `392e8cc84edbee68…`

## Device contracts in force

- seq 2 `ofm.stage` adapter `openmhs.py.wot` descriptor sha256 `4bd63698dd0dbf67`

## Commands

### `7d3b2ede0cdf` ofm.stage / move_y → **VERIFIED**

- asked by `agent:survey` (credential `tok_7efdc67e6a9d8777`); invoke {"y": -1}
- authorization: allow by ['my']
- device's own reference: `labthings:04b5587b-5886-4dee-8deb-8a6d21233ecf`
- lifecycle: CREATED (12:48:55.811) → VALIDATING (12:48:55.816) → APPROVED (12:48:55.818) → DISPATCHED (12:48:55.822) → ACCEPTED (12:48:55.862) → APPLIED (12:48:56.001) → VERIFIED (12:48:56.157)
- verification: readback; expected -1, observed -1 at 2026-10-30T12:48:56.157Z (simulator), matched True

### `c99214aeffed` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_7efdc67e6a9d8777`); invoke {"x": -1}
- authorization: allow by ['mx']
- device's own reference: `labthings:097c323f-421e-466a-89d6-e9b246281a53`
- lifecycle: CREATED (12:49:01.071) → VALIDATING (12:49:01.075) → APPROVED (12:49:01.078) → DISPATCHED (12:49:01.082) → ACCEPTED (12:49:01.147) → APPLIED (12:49:01.297) → VERIFIED (12:49:01.425)
- verification: readback; expected -1, observed -1 at 2026-10-30T12:49:01.425Z (simulator), matched True

### `0104adf936df` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_7efdc67e6a9d8777`); invoke {"x": 0}
- authorization: allow by ['mx']
- device's own reference: `labthings:d9e0fbaf-a8bd-43d0-b5c5-c34302f3b4bb`
- lifecycle: CREATED (12:49:05.865) → VALIDATING (12:49:05.87Z) → APPROVED (12:49:05.873) → DISPATCHED (12:49:05.877) → ACCEPTED (12:49:05.914) → APPLIED (12:49:06.079) → VERIFIED (12:49:06.2Z)
- verification: readback; expected 0, observed 0 at 2026-10-30T12:49:06.2Z (simulator), matched True

### `e174bbb5ff06` ofm.stage / move_y → **VERIFIED**

- asked by `agent:survey` (credential `tok_7efdc67e6a9d8777`); invoke {"y": 0}
- authorization: allow by ['my']
- device's own reference: `labthings:9b2b22b0-749f-445c-abe7-6cd5fcd1c50b`
- lifecycle: CREATED (12:49:11.625) → VALIDATING (12:49:11.63Z) → APPROVED (12:49:11.633) → DISPATCHED (12:49:11.638) → ACCEPTED (12:49:11.688) → APPLIED (12:49:11.821) → VERIFIED (12:49:11.983)
- verification: readback; expected 0, observed 0 at 2026-10-30T12:49:11.983Z (simulator), matched True

### `a850043372b2` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_7efdc67e6a9d8777`); invoke {"x": 1}
- authorization: allow by ['mx']
- device's own reference: `labthings:9155d89b-c221-463f-b14b-619e4e1bb4e7`
- lifecycle: CREATED (12:49:16.612) → VALIDATING (12:49:16.616) → APPROVED (12:49:16.618) → DISPATCHED (12:49:16.62Z) → ACCEPTED (12:49:16.627) → APPLIED (12:49:16.75Z) → VERIFIED (12:49:16.9Z)
- verification: readback; expected 1, observed 1 at 2026-10-30T12:49:16.899Z (simulator), matched True

### `fde1ba8ac9a4` ofm.stage / move_y → **VERIFIED**

- asked by `agent:survey` (credential `tok_7efdc67e6a9d8777`); invoke {"y": 1}
- authorization: allow by ['my']
- device's own reference: `labthings:28cf45b1-5159-4a7d-a132-feda6e5fbeaf`
- lifecycle: CREATED (12:49:21.676) → VALIDATING (12:49:21.682) → APPROVED (12:49:21.684) → DISPATCHED (12:49:21.688) → ACCEPTED (12:49:21.734) → APPLIED (12:49:21.887) → VERIFIED (12:49:22.037)
- verification: readback; expected 1, observed 1 at 2026-10-30T12:49:22.036Z (simulator), matched True

### `e50a8752a61f` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_7efdc67e6a9d8777`); invoke {"x": 0}
- authorization: allow by ['mx']
- device's own reference: `labthings:1977a5b0-f197-42b9-ad17-6079a302929c`
- lifecycle: CREATED (12:49:28.577) → VALIDATING (12:49:28.583) → APPROVED (12:49:28.586) → DISPATCHED (12:49:28.59Z) → ACCEPTED (12:49:28.644) → APPLIED (12:49:28.79Z) → VERIFIED (12:49:28.946)
- verification: readback; expected 0, observed 0 at 2026-10-30T12:49:28.945Z (simulator), matched True

### `711b4a90d6e9` ofm.stage / move_y → **VERIFIED**

- asked by `agent:survey` (credential `tok_7efdc67e6a9d8777`); invoke {"y": 0}
- authorization: allow by ['my']
- device's own reference: `labthings:96ed76a6-a0ef-4b3f-b0ce-4f7adf2de614`
- lifecycle: CREATED (12:49:33.938) → VALIDATING (12:49:33.943) → APPROVED (12:49:33.946) → DISPATCHED (12:49:33.949) → ACCEPTED (12:49:33.992) → APPLIED (12:49:34.136) → VERIFIED (12:49:34.296)
- verification: readback; expected 0, observed 0 at 2026-10-30T12:49:34.295Z (simulator), matched True

### `1bc6d64a4062` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_7efdc67e6a9d8777`); invoke {"x": -1}
- authorization: allow by ['mx']
- device's own reference: `labthings:de1ad5d6-5718-45b9-8b76-fc1ba8920fa4`
- lifecycle: CREATED (12:49:38.927) → VALIDATING (12:49:38.932) → APPROVED (12:49:38.934) → DISPATCHED (12:49:38.937) → ACCEPTED (12:49:38.991) → APPLIED (12:49:39.137) → VERIFIED (12:49:39.283)
- verification: readback; expected -1, observed -1 at 2026-10-30T12:49:39.283Z (simulator), matched True

### `d75895168639` ofm.stage / move_y → **VERIFIED**

- asked by `agent:survey` (credential `tok_7efdc67e6a9d8777`); invoke {"y": -1}
- authorization: allow by ['my']
- device's own reference: `labthings:9e03381e-a90f-4f51-87a4-b83da98cb5a6`
- lifecycle: CREATED (12:49:45.934) → VALIDATING (12:49:45.94Z) → APPROVED (12:49:45.946) → DISPATCHED (12:49:45.951) → ACCEPTED (12:49:45.999) → APPLIED (12:49:46.145) → VERIFIED (12:49:46.297)
- verification: readback; expected -1, observed -1 at 2026-10-30T12:49:46.297Z (simulator), matched True

### `ae34c1cfe3ac` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_7efdc67e6a9d8777`); invoke {"x": 0}
- authorization: allow by ['mx']
- device's own reference: `labthings:c50cd6cf-150f-4513-9cb4-22f36bb4cbc8`
- lifecycle: CREATED (12:49:50.764) → VALIDATING (12:49:50.766) → APPROVED (12:49:50.768) → DISPATCHED (12:49:50.772) → ACCEPTED (12:49:50.824) → APPLIED (12:49:50.976) → VERIFIED (12:49:51.141)
- verification: readback; expected 0, observed 0 at 2026-10-30T12:49:51.14Z (simulator), matched True

### `c5653bd64deb` ofm.stage / move_y → **VERIFIED**

- asked by `agent:survey` (credential `tok_7efdc67e6a9d8777`); invoke {"y": 0}
- authorization: allow by ['my']
- device's own reference: `labthings:93b5966f-222d-448f-a86c-6b54500ca99b`
- lifecycle: CREATED (12:49:55.634) → VALIDATING (12:49:55.639) → APPROVED (12:49:55.641) → DISPATCHED (12:49:55.644) → ACCEPTED (12:49:55.654) → APPLIED (12:49:55.813) → VERIFIED (12:49:55.956)
- verification: readback; expected 0, observed 0 at 2026-10-30T12:49:55.956Z (simulator), matched True

### `74e7a3fa74c3` ofm.stage / move_z → **VERIFIED**

- asked by `agent:survey` (credential `tok_7efdc67e6a9d8777`); invoke {"z": -1}
- authorization: allow by ['mz']
- device's own reference: `labthings:7c52558a-ba80-44b9-979e-05fd3bf363b5`
- lifecycle: CREATED (12:50:00.974) → VALIDATING (12:50:00.978) → APPROVED (12:50:00.98Z) → DISPATCHED (12:50:00.983) → ACCEPTED (12:50:01.023) → APPLIED (12:50:01.165) → VERIFIED (12:50:01.323)
- verification: readback; expected -1, observed -1 at 2026-10-30T12:50:01.322Z (simulator), matched True

### `3604808bf7ee` ofm.stage / move_z → **VERIFIED**

- asked by `agent:survey` (credential `tok_7efdc67e6a9d8777`); invoke {"z": 0}
- authorization: allow by ['mz']
- device's own reference: `labthings:5f2d402c-cc1a-493f-a80e-464b6cbafe71`
- lifecycle: CREATED (12:50:06.514) → VALIDATING (12:50:06.522) → APPROVED (12:50:06.529) → DISPATCHED (12:50:06.538) → ACCEPTED (12:50:06.569) → APPLIED (12:50:06.717) → VERIFIED (12:50:06.867)
- verification: readback; expected 0, observed 0 at 2026-10-30T12:50:06.867Z (simulator), matched True

### `25967ba1edfe` ofm.stage / move_z → **VERIFIED**

- asked by `agent:survey` (credential `tok_7efdc67e6a9d8777`); invoke {"z": 1}
- authorization: allow by ['mz']
- device's own reference: `labthings:a18be442-2f28-4c39-940a-34686aeec4b2`
- lifecycle: CREATED (12:50:12.38Z) → VALIDATING (12:50:12.385) → APPROVED (12:50:12.388) → DISPATCHED (12:50:12.392) → ACCEPTED (12:50:12.442) → APPLIED (12:50:12.586) → VERIFIED (12:50:12.729)
- verification: readback; expected 1, observed 1 at 2026-10-30T12:50:12.728Z (simulator), matched True

### `e72a953dccfd` ofm.stage / move_z → **VERIFIED**

- asked by `agent:survey` (credential `tok_7efdc67e6a9d8777`); invoke {"z": 0}
- authorization: allow by ['mz']
- device's own reference: `labthings:79b324d8-7621-49cb-8ce5-9857766e8554`
- lifecycle: CREATED (12:50:17.956) → VALIDATING (12:50:17.96Z) → APPROVED (12:50:17.962) → DISPATCHED (12:50:17.965) → ACCEPTED (12:50:18Z) → APPLIED (12:50:18.135) → VERIFIED (12:50:18.283)
- verification: readback; expected 0, observed 0 at 2026-10-30T12:50:18.283Z (simulator), matched True
