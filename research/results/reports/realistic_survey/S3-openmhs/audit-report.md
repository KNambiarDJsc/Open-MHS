# Experiment report

Generated 2026-10-30T18:23:28+0530.

> A view of the gateway's journal. It is only as trustworthy as the journal; the audit chain detects alteration and reordering of the records present, not removal of the newest ones (compare the anchor) and not a gateway that lied. It records the device's reports, not the physical truth.

## Audit chain

- records: **170**, chain valid: **True**
- anchor (keep this somewhere the gateway's writer cannot reach): seq 170, hash `af149f5e8ef77cab…`

## Device contracts in force

- seq 2 `ofm.stage` adapter `openmhs.py.wot` descriptor sha256 `4bd63698dd0dbf67`

## Commands

### `96e508fc2bc1` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_c3784bb3ad570933`); invoke {"x": -1}
- authorization: allow by ['mx']
- device's own reference: `labthings:8f80ef25-eda0-478e-b12d-afe6f0191d85`
- lifecycle: CREATED (12:51:53.056) → VALIDATING (12:51:53.062) → APPROVED (12:51:53.065) → DISPATCHED (12:51:53.07Z) → ACCEPTED (12:51:53.09Z) → APPLIED (12:51:53.233) → VERIFIED (12:51:53.351)
- verification: readback; expected -1, observed -1 at 2026-10-30T12:51:53.35Z (simulator), matched True

### `0e1776d83053` ofm.stage / move_y → **VERIFIED**

- asked by `agent:survey` (credential `tok_c3784bb3ad570933`); invoke {"y": 1}
- authorization: allow by ['my']
- device's own reference: `labthings:5b585aef-afa8-473b-a47f-25a24a478225`
- lifecycle: CREATED (12:51:56.366) → VALIDATING (12:51:56.369) → APPROVED (12:51:56.372) → DISPATCHED (12:51:56.376) → ACCEPTED (12:51:56.394) → APPLIED (12:51:56.517) → VERIFIED (12:51:56.645)
- verification: readback; expected 1, observed 1 at 2026-10-30T12:51:56.644Z (simulator), matched True

### `ef38f58deb7d` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_c3784bb3ad570933`); invoke {"x": 0}
- authorization: allow by ['mx']
- device's own reference: `labthings:e60bb076-cc1e-48f5-828d-3d8c5b58f7e2`
- lifecycle: CREATED (12:52:02.332) → VALIDATING (12:52:02.333) → APPROVED (12:52:02.335) → DISPATCHED (12:52:02.337) → ACCEPTED (12:52:02.355) → APPLIED (12:52:02.482) → VERIFIED (12:52:02.634)
- verification: readback; expected 0, observed 0 at 2026-10-30T12:52:02.634Z (simulator), matched True

### `76684ea43d3e` ofm.stage / move_x → **RECONCILED_APPLIED**

- asked by `agent:survey` (credential `tok_c3784bb3ad570933`); invoke {"x": 1}
- authorization: allow by ['mx']
- device's own reference: `labthings:85b05837-8400-4908-a936-a618ce788135`
- lifecycle: CREATED (12:52:08.191) → VALIDATING (12:52:08.195) → APPROVED (12:52:08.198) → DISPATCHED (12:52:08.201) → ACCEPTED (12:52:08.232) → UNKNOWN_OUTCOME (12:52:08.364) → RECONCILED_APPLIED (12:52:09.748)
- verification: readback; expected 1, observed 1 at 2026-10-30T12:52:09.747Z (simulator), matched True
- device error: lost contact with invocation 85b05837-8400-4908-a936-a618ce788135 (http://127.0.0.1:58816/api/v3/action_invocations/85b05837-8400-4908-a936-a618ce788135) after it was accepted (Remote end closed connection without response); its outcome is unknown

### `92537b5bb843` ofm.stage / move_y → **VERIFIED**

- asked by `agent:survey` (credential `tok_c3784bb3ad570933`); invoke {"y": 0}
- authorization: allow by ['my']
- device's own reference: `labthings:2c8b6271-caed-4c91-bcc6-00cb9cee188d`
- lifecycle: CREATED (12:52:16.788) → VALIDATING (12:52:16.792) → APPROVED (12:52:16.794) → DISPATCHED (12:52:16.797) → ACCEPTED (12:52:16.827) → APPLIED (12:52:16.964) → VERIFIED (12:52:17.118)
- verification: readback; expected 0, observed 0 at 2026-10-30T12:52:17.118Z (simulator), matched True

### `98971a860953` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_c3784bb3ad570933`); invoke {"x": 0}
- authorization: allow by ['mx']
- device's own reference: `labthings:1efc2587-c42a-415d-9d14-f5f287be23b4`
- lifecycle: CREATED (12:52:22.036) → VALIDATING (12:52:22.038) → APPROVED (12:52:22.039) → DISPATCHED (12:52:22.042) → ACCEPTED (12:52:22.075) → APPLIED (12:52:22.198) → VERIFIED (12:52:22.316)
- verification: readback; expected 0, observed 0 at 2026-10-30T12:52:22.316Z (simulator), matched True

### `609582447d04` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_c3784bb3ad570933`); invoke {"x": -1}
- authorization: allow by ['mx']
- device's own reference: `labthings:4a6dedda-cf99-428c-805d-1568a8acdbb6`
- lifecycle: CREATED (12:52:27.074) → VALIDATING (12:52:27.079) → APPROVED (12:52:27.082) → DISPATCHED (12:52:27.084) → ACCEPTED (12:52:27.129) → APPLIED (12:52:27.247) → VERIFIED (12:52:27.367)
- verification: readback; expected -1, observed -1 at 2026-10-30T12:52:27.367Z (simulator), matched True

### `c5b9bcc5955f` ofm.stage / move_y → **VERIFIED**

- asked by `agent:survey` (credential `tok_c3784bb3ad570933`); invoke {"y": -1}
- authorization: allow by ['my']
- device's own reference: `labthings:9a934c3e-c7b7-4144-bc0c-20fe2477b5c4`
- lifecycle: CREATED (12:52:31.752) → VALIDATING (12:52:31.755) → APPROVED (12:52:31.757) → DISPATCHED (12:52:31.76Z) → ACCEPTED (12:52:31.81Z) → APPLIED (12:52:31.947) → VERIFIED (12:52:32.068)
- verification: readback; expected -1, observed -1 at 2026-10-30T12:52:32.068Z (simulator), matched True

### `9cf52ad08d7d` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_c3784bb3ad570933`); invoke {"x": 0}
- authorization: allow by ['mx']
- device's own reference: `labthings:c4a6bf25-4b37-48e3-90b1-401f01a08390`
- lifecycle: CREATED (12:52:37.293) → VALIDATING (12:52:37.295) → APPROVED (12:52:37.298) → DISPATCHED (12:52:37.301) → ACCEPTED (12:52:37.337) → APPLIED (12:52:37.495) → VERIFIED (12:52:37.648)
- verification: readback; expected 0, observed 0 at 2026-10-30T12:52:37.648Z (simulator), matched True

### `c5ef9c95b538` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_c3784bb3ad570933`); invoke {"x": 1}
- authorization: allow by ['mx']
- device's own reference: `labthings:8d628ea9-a392-45d6-861e-ba8d8dad59ec`
- lifecycle: CREATED (12:52:47.239) → VALIDATING (12:52:47.244) → APPROVED (12:52:47.247) → DISPATCHED (12:52:47.252) → ACCEPTED (12:52:47.282) → APPLIED (12:52:47.432) → VERIFIED (12:52:47.595)
- verification: readback; expected 1, observed 1 at 2026-10-30T12:52:47.594Z (simulator), matched True

### `c435b6a1aefd` ofm.stage / move_y → **VERIFIED**

- asked by `agent:survey` (credential `tok_c3784bb3ad570933`); invoke {"y": 0}
- authorization: allow by ['my']
- device's own reference: `labthings:e40860fb-dddd-450f-a6fa-62275c714ad9`
- lifecycle: CREATED (12:52:52.7Z) → VALIDATING (12:52:52.704) → APPROVED (12:52:52.707) → DISPATCHED (12:52:52.71Z) → ACCEPTED (12:52:52.757) → APPLIED (12:52:52.899) → VERIFIED (12:52:53.023)
- verification: readback; expected 0, observed 0 at 2026-10-30T12:52:53.023Z (simulator), matched True

### `39cbb0aa03ff` ofm.stage / move_z → **VERIFIED**

- asked by `agent:survey` (credential `tok_c3784bb3ad570933`); invoke {"z": -1}
- authorization: allow by ['mz']
- device's own reference: `labthings:4bd22163-d928-4b30-8f20-895754549e4f`
- lifecycle: CREATED (12:52:56.793) → VALIDATING (12:52:56.797) → APPROVED (12:52:56.8Z) → DISPATCHED (12:52:56.803) → ACCEPTED (12:52:56.813) → APPLIED (12:52:56.963) → VERIFIED (12:52:57.11Z)
- verification: readback; expected -1, observed -1 at 2026-10-30T12:52:57.11Z (simulator), matched True

### `d2ef86ca1864` ofm.stage / move_z → **VERIFIED**

- asked by `agent:survey` (credential `tok_c3784bb3ad570933`); invoke {"z": 0}
- authorization: allow by ['mz']
- device's own reference: `labthings:07957f5e-2b6f-4ab7-af8e-436cd76a1dda`
- lifecycle: CREATED (12:53:03.487) → VALIDATING (12:53:03.492) → APPROVED (12:53:03.493) → DISPATCHED (12:53:03.496) → ACCEPTED (12:53:03.52Z) → APPLIED (12:53:03.657) → VERIFIED (12:53:03.797)
- verification: readback; expected 0, observed 0 at 2026-10-30T12:53:03.797Z (simulator), matched True

### `7333fa2a8c44` ofm.stage / move_z → **VERIFIED**

- asked by `agent:survey` (credential `tok_c3784bb3ad570933`); invoke {"z": 1}
- authorization: allow by ['mz']
- device's own reference: `labthings:4756c115-0110-4ce4-aa51-3d5e90c86ac0`
- lifecycle: CREATED (12:53:10.047) → VALIDATING (12:53:10.049) → APPROVED (12:53:10.05Z) → DISPATCHED (12:53:10.053) → ACCEPTED (12:53:10.089) → APPLIED (12:53:10.238) → VERIFIED (12:53:10.377)
- verification: readback; expected 1, observed 1 at 2026-10-30T12:53:10.376Z (simulator), matched True

### `ceb0374e0819` ofm.stage / move_z → **VERIFIED**

- asked by `agent:survey` (credential `tok_c3784bb3ad570933`); invoke {"z": 0}
- authorization: allow by ['mz']
- device's own reference: `labthings:f97ea499-7bf1-4d16-9fb9-ca05ecccd3f5`
- lifecycle: CREATED (12:53:13.799) → VALIDATING (12:53:13.802) → APPROVED (12:53:13.805) → DISPATCHED (12:53:13.809) → ACCEPTED (12:53:13.82Z) → APPLIED (12:53:13.958) → VERIFIED (12:53:14.114)
- verification: readback; expected 0, observed 0 at 2026-10-30T12:53:14.113Z (simulator), matched True

### `3f76dbf2b012` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_c3784bb3ad570933`); invoke {"x": 0}
- authorization: allow by ['mx']
- device's own reference: `labthings:51d2c7a4-8a06-4e1b-809f-980efd59db23`
- lifecycle: CREATED (12:53:18.571) → VALIDATING (12:53:18.576) → APPROVED (12:53:18.577) → DISPATCHED (12:53:18.58Z) → ACCEPTED (12:53:18.626) → APPLIED (12:53:18.764) → VERIFIED (12:53:18.906)
- verification: readback; expected 0, observed 0 at 2026-10-30T12:53:18.905Z (simulator), matched True
