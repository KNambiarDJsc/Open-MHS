# Experiment report

Generated 2026-10-30T18:31:53+0530.

> A view of the gateway's journal. It is only as trustworthy as the journal; the audit chain detects alteration and reordering of the records present, not removal of the newest ones (compare the anchor) and not a gateway that lied. It records the device's reports, not the physical truth.

## Audit chain

- records: **376**, chain valid: **True**
- anchor (keep this somewhere the gateway's writer cannot reach): seq 376, hash `e45f1e486fc3ca60…`

## Device contracts in force

- seq 2 `ofm.stage` adapter `openmhs.py.wot` descriptor sha256 `f2a5202382c461c4`

## Commands

### `b60ca20a0ec2` ofm.stage / move_y → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"y": -1000}
- authorization: allow by ['survey-y']
- device's own reference: `labthings:a082b7ea-a6f3-4cda-b8cc-caf1c80d4ad9`
- lifecycle: CREATED (13:00:58.647) → VALIDATING (13:00:58.649) → APPROVED (13:00:58.65Z) → DISPATCHED (13:00:58.653) → ACCEPTED (13:00:58.693) → APPLIED (13:00:59.807) → VERIFIED (13:00:59.958)
- verification: readback; expected -1000, observed -1000 at 2026-10-30T13:00:59.958Z (simulator), matched True

### `82e2c51c683e` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": -1000}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:0eb705a9-1eaa-4126-a0b9-64cc4fd3bc41`
- lifecycle: CREATED (13:00:59.977) → VALIDATING (13:00:59.979) → APPROVED (13:00:59.98Z) → DISPATCHED (13:00:59.983) → ACCEPTED (13:01:00.013) → APPLIED (13:01:01.027) → VERIFIED (13:01:01.174)
- verification: readback; expected -1000, observed -1000 at 2026-10-30T13:01:01.174Z (simulator), matched True

### `7f342670b218` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": -500}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:904b542f-a0bf-4baf-a195-dec943741556`
- lifecycle: CREATED (13:01:01.674) → VALIDATING (13:01:01.677) → APPROVED (13:01:01.679) → DISPATCHED (13:01:01.683) → ACCEPTED (13:01:01.714) → APPLIED (13:01:02.279) → VERIFIED (13:01:02.399)
- verification: readback; expected -500, observed -500 at 2026-10-30T13:01:02.399Z (simulator), matched True

### `fca16f1a82d9` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": 0}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:c7bda5e7-18f0-44c5-bee9-247300b853d9`
- lifecycle: CREATED (13:01:02.843) → VALIDATING (13:01:02.848) → APPROVED (13:01:02.85Z) → DISPATCHED (13:01:02.854) → ACCEPTED (13:01:02.877) → APPLIED (13:01:03.512) → VERIFIED (13:01:03.658)
- verification: readback; expected 0, observed 0 at 2026-10-30T13:01:03.657Z (simulator), matched True

### `afd92ceb2db6` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": 500}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:9f532465-71b9-41f0-be3b-da542259600a`
- lifecycle: CREATED (13:01:04.075) → VALIDATING (13:01:04.08Z) → APPROVED (13:01:04.082) → DISPATCHED (13:01:04.085) → ACCEPTED (13:01:04.142) → APPLIED (13:01:04.803) → VERIFIED (13:01:04.945)
- verification: readback; expected 500, observed 500 at 2026-10-30T13:01:04.945Z (simulator), matched True

### `ed559ec20444` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": 1000}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:f8a23e78-2277-454c-9647-e2a00599714f`
- lifecycle: CREATED (13:01:05.365) → VALIDATING (13:01:05.369) → APPROVED (13:01:05.371) → DISPATCHED (13:01:05.373) → ACCEPTED (13:01:05.381) → APPLIED (13:01:05.944) → VERIFIED (13:01:06.096)
- verification: readback; expected 1000, observed 1000 at 2026-10-30T13:01:06.096Z (simulator), matched True

### `ed3454073086` ofm.stage / move_y → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"y": -500}
- authorization: allow by ['survey-y']
- device's own reference: `labthings:c715a7af-ace2-4254-b8e5-f9977c7af33d`
- lifecycle: CREATED (13:01:06.522) → VALIDATING (13:01:06.527) → APPROVED (13:01:06.529) → DISPATCHED (13:01:06.532) → ACCEPTED (13:01:06.582) → APPLIED (13:01:07.147) → VERIFIED (13:01:07.299)
- verification: readback; expected -500, observed -500 at 2026-10-30T13:01:07.299Z (simulator), matched True

### `a0ccc25f7d83` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": 500}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:dcf66349-0606-4c18-bbae-58a1ce3a7d70`
- lifecycle: CREATED (13:01:07.74Z) → VALIDATING (13:01:07.744) → APPROVED (13:01:07.746) → DISPATCHED (13:01:07.749) → ACCEPTED (13:01:07.8Z) → APPLIED (13:01:08.332) → VERIFIED (13:01:08.441)
- verification: readback; expected 500, observed 500 at 2026-10-30T13:01:08.441Z (simulator), matched True

### `87aa09b0b05d` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": 0}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:60abc742-b4dc-4056-95d5-0ed74857d0f4`
- lifecycle: CREATED (13:01:08.875) → VALIDATING (13:01:08.879) → APPROVED (13:01:08.88Z) → DISPATCHED (13:01:08.883) → ACCEPTED (13:01:08.912) → APPLIED (13:01:09.465) → VERIFIED (13:01:09.59Z)
- verification: readback; expected 0, observed 0 at 2026-10-30T13:01:09.589Z (simulator), matched True

### `963495891bb6` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": -500}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:7d533730-7ee5-4ad1-a0be-32205e62e0f2`
- lifecycle: CREATED (13:01:10.047) → VALIDATING (13:01:10.051) → APPROVED (13:01:10.053) → DISPATCHED (13:01:10.057) → ACCEPTED (13:01:10.105) → APPLIED (13:01:10.683) → VERIFIED (13:01:10.837)
- verification: readback; expected -500, observed -500 at 2026-10-30T13:01:10.836Z (simulator), matched True

### `f528e9486c8d` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": -1000}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:e2c4e5c8-7679-4052-86c4-fd305fed6bd2`
- lifecycle: CREATED (13:01:11.314) → VALIDATING (13:01:11.319) → APPROVED (13:01:11.321) → DISPATCHED (13:01:11.324) → ACCEPTED (13:01:11.36Z) → APPLIED (13:01:11.89Z) → VERIFIED (13:01:12.023)
- verification: readback; expected -1000, observed -1000 at 2026-10-30T13:01:12.022Z (simulator), matched True

### `5ab372ffc159` ofm.stage / move_y → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"y": 0}
- authorization: allow by ['survey-y']
- device's own reference: `labthings:abd8f23b-6305-40c0-874b-509ea75fd10d`
- lifecycle: CREATED (13:01:12.457) → VALIDATING (13:01:12.459) → APPROVED (13:01:12.461) → DISPATCHED (13:01:12.464) → ACCEPTED (13:01:12.471) → APPLIED (13:01:13.022) → VERIFIED (13:01:13.185)
- verification: readback; expected 0, observed 0 at 2026-10-30T13:01:13.185Z (simulator), matched True

### `1196ca652488` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": -500}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:d1ebac5e-355e-4e3a-aab0-d73afaba06a0`
- lifecycle: CREATED (13:01:13.58Z) → VALIDATING (13:01:13.583) → APPROVED (13:01:13.585) → DISPATCHED (13:01:13.588) → ACCEPTED (13:01:13.597) → APPLIED (13:01:14.218) → VERIFIED (13:01:14.355)
- verification: readback; expected -500, observed -500 at 2026-10-30T13:01:14.355Z (simulator), matched True

### `ee362664e133` ofm.stage / move_x → **RECONCILED_APPLIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": 0}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:149e46a8-f2c9-4517-9c69-4f8da1f2875f`
- lifecycle: CREATED (13:01:14.799) → VALIDATING (13:01:14.805) → APPROVED (13:01:14.811) → DISPATCHED (13:01:14.82Z) → ACCEPTED (13:01:14.869) → UNKNOWN_OUTCOME (13:01:15.006) → RECONCILED_APPLIED (13:01:18.091)
- verification: readback; expected 0, observed 0 at 2026-10-30T13:01:18.089Z (simulator), matched True
- device error: lost contact with invocation 149e46a8-f2c9-4517-9c69-4f8da1f2875f (http://127.0.0.1:51706/api/v3/action_invocations/149e46a8-f2c9-4517-9c69-4f8da1f2875f) after it was accepted (Remote end closed connection without response); its outcome is unknown

### `55f1fc84b030` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": 500}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:fd7d5692-0ff1-457d-9e42-94d1f29ba382`
- lifecycle: CREATED (13:01:18.479) → VALIDATING (13:01:18.481) → APPROVED (13:01:18.483) → DISPATCHED (13:01:18.487) → ACCEPTED (13:01:18.494) → APPLIED (13:01:19.017) → VERIFIED (13:01:19.165)
- verification: readback; expected 500, observed 500 at 2026-10-30T13:01:19.165Z (simulator), matched True

### `ff4f0fbdf06c` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": 1000}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:cd1654a3-4287-426e-9164-2d66b44f725c`
- lifecycle: CREATED (13:01:19.602) → VALIDATING (13:01:19.604) → APPROVED (13:01:19.606) → DISPATCHED (13:01:19.61Z) → ACCEPTED (13:01:19.642) → APPLIED (13:01:20.204) → VERIFIED (13:01:20.35Z)
- verification: readback; expected 1000, observed 1000 at 2026-10-30T13:01:20.349Z (simulator), matched True

### `670f77f81a43` ofm.stage / move_y → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"y": 500}
- authorization: allow by ['survey-y']
- device's own reference: `labthings:48f8a49d-e214-4c77-95cd-6478d89c5d2a`
- lifecycle: CREATED (13:01:20.784) → VALIDATING (13:01:20.785) → APPROVED (13:01:20.787) → DISPATCHED (13:01:20.79Z) → ACCEPTED (13:01:20.824) → APPLIED (13:01:21.408) → VERIFIED (13:01:21.557)
- verification: readback; expected 500, observed 500 at 2026-10-30T13:01:21.557Z (simulator), matched True

### `703cfb752e95` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": 500}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:2dc14109-b949-40e7-acb4-ee5b2590d14d`
- lifecycle: CREATED (13:01:22.032) → VALIDATING (13:01:22.037) → APPROVED (13:01:22.04Z) → DISPATCHED (13:01:22.044) → ACCEPTED (13:01:22.07Z) → APPLIED (13:01:22.609) → VERIFIED (13:01:22.763)
- verification: readback; expected 500, observed 500 at 2026-10-30T13:01:22.762Z (simulator), matched True

### `b497d1f57871` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": 0}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:db5fcbde-d79d-4ac3-8cb1-af7bd8f789fc`
- lifecycle: CREATED (13:01:23.288) → VALIDATING (13:01:23.293) → APPROVED (13:01:23.296) → DISPATCHED (13:01:23.3Z) → ACCEPTED (13:01:23.326) → APPLIED (13:01:23.897) → VERIFIED (13:01:24.054)
- verification: readback; expected 0, observed 0 at 2026-10-30T13:01:24.053Z (simulator), matched True

### `242dbe4bc2b4` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": -500}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:e4a2ca49-5b44-46d5-8792-af77b1385b3a`
- lifecycle: CREATED (13:01:24.503) → VALIDATING (13:01:24.507) → APPROVED (13:01:24.509) → DISPATCHED (13:01:24.512) → ACCEPTED (13:01:24.545) → APPLIED (13:01:25.057) → VERIFIED (13:01:25.188)
- verification: readback; expected -500, observed -500 at 2026-10-30T13:01:25.188Z (simulator), matched True

### `d3c44b528568` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": -1000}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:ef841240-454f-410a-af65-6d80a7a67c95`
- lifecycle: CREATED (13:01:25.631) → VALIDATING (13:01:25.746) → APPROVED (13:01:25.749) → DISPATCHED (13:01:25.753) → ACCEPTED (13:01:25.801) → APPLIED (13:01:26.343) → VERIFIED (13:01:26.503)
- verification: readback; expected -1000, observed -1000 at 2026-10-30T13:01:26.502Z (simulator), matched True

### `3a9687b21eca` ofm.stage / move_y → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"y": 1000}
- authorization: allow by ['survey-y']
- device's own reference: `labthings:901265ae-b65e-432c-8f13-3f30885012ed`
- lifecycle: CREATED (13:01:26.951) → VALIDATING (13:01:26.959) → APPROVED (13:01:26.974) → DISPATCHED (13:01:26.984) → ACCEPTED (13:01:27.015) → APPLIED (13:01:27.551) → VERIFIED (13:01:27.697)
- verification: readback; expected 1000, observed 1000 at 2026-10-30T13:01:27.696Z (simulator), matched True

### `ccf6979d4d9f` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": -500}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:a6a02b71-87d9-4a58-b192-1ee98a2068b5`
- lifecycle: CREATED (13:01:28.171) → VALIDATING (13:01:28.188) → APPROVED (13:01:28.194) → DISPATCHED (13:01:28.202) → ACCEPTED (13:01:28.253) → APPLIED (13:01:28.775) → VERIFIED (13:01:28.943)
- verification: readback; expected -500, observed -500 at 2026-10-30T13:01:28.942Z (simulator), matched True

### `17699dfc876c` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": 0}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:2159ab80-2bf6-4051-8e43-370f1bdc4208`
- lifecycle: CREATED (13:01:29.373) → VALIDATING (13:01:29.395) → APPROVED (13:01:29.397) → DISPATCHED (13:01:29.4Z) → ACCEPTED (13:01:29.408) → APPLIED (13:01:29.966) → VERIFIED (13:01:30.119)
- verification: readback; expected 0, observed 0 at 2026-10-30T13:01:30.118Z (simulator), matched True

### `f0ce1eed7c36` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": 500}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:9cc75f81-5040-471e-b398-6ee673ab48ea`
- lifecycle: CREATED (13:01:30.546) → VALIDATING (13:01:30.561) → APPROVED (13:01:30.571) → DISPATCHED (13:01:30.577) → ACCEPTED (13:01:30.615) → APPLIED (13:01:31.145) → VERIFIED (13:01:31.278)
- verification: readback; expected 500, observed 500 at 2026-10-30T13:01:31.277Z (simulator), matched True

### `0f638c263bc1` ofm.stage / move_x → **VERIFIED**

- asked by `agent:survey` (credential `tok_85c2cb2ecee6c3e8`); invoke {"x": 1000}
- authorization: allow by ['survey-x']
- device's own reference: `labthings:cd65e910-1ff7-4f9b-b5dd-ef8ac745d0e3`
- lifecycle: CREATED (13:01:31.775) → VALIDATING (13:01:31.785) → APPROVED (13:01:31.794) → DISPATCHED (13:01:31.803) → ACCEPTED (13:01:31.846) → APPLIED (13:01:32.385) → VERIFIED (13:01:32.546)
- verification: readback; expected 1000, observed 1000 at 2026-10-30T13:01:32.546Z (simulator), matched True

### `2087d7151719` ofm.stage / move_y → **VERIFIED**

- asked by `agent:adaptive` (credential `tok_b1f0f2c44fc31c00`); invoke {"y": 0}
- authorization: require_approval by ['adaptive-y'], approved by operator:local
- device's own reference: `labthings:ac372877-b5bf-4b05-9572-9f0f5e6d5fce`
- lifecycle: CREATED (13:01:36.366) → VALIDATING (13:01:36.368) → AWAITING_APPROVAL (13:01:36.374) → APPROVED (13:01:36.38Z) → DISPATCHED (13:01:36.383) → ACCEPTED (13:01:36.43Z) → APPLIED (13:01:37.504) → VERIFIED (13:01:37.652)
- verification: readback; expected 0, observed 0 at 2026-10-30T13:01:37.651Z (simulator), matched True

### `9e557cf68de6` ofm.stage / move_x → **VERIFIED**

- asked by `agent:adaptive` (credential `tok_b1f0f2c44fc31c00`); invoke {"x": 0}
- authorization: require_approval by ['adaptive-x'], approved by operator:local
- device's own reference: `labthings:a51b3d04-a9d8-41dd-a7f0-efa9ebb4e595`
- lifecycle: CREATED (13:01:37.696) → VALIDATING (13:01:37.698) → AWAITING_APPROVAL (13:01:37.702) → APPROVED (13:01:37.707) → DISPATCHED (13:01:37.712) → ACCEPTED (13:01:37.747) → APPLIED (13:01:38.851) → VERIFIED (13:01:39.001)
- verification: readback; expected 0, observed 0 at 2026-10-30T13:01:39.001Z (simulator), matched True

### `80540b010b8f` ofm.stage / move_z → **VERIFIED**

- asked by `agent:adaptive` (credential `tok_b1f0f2c44fc31c00`); invoke {"z": -300}
- authorization: require_approval by ['adaptive-z'], approved by operator:local
- device's own reference: `labthings:fc416d1a-70bc-48cf-8d4c-69959e37787f`
- lifecycle: CREATED (13:01:39.029) → VALIDATING (13:01:39.031) → AWAITING_APPROVAL (13:01:39.034) → APPROVED (13:01:39.041) → DISPATCHED (13:01:39.045) → ACCEPTED (13:01:39.086) → APPLIED (13:01:39.497) → VERIFIED (13:01:39.66Z)
- verification: readback; expected -300, observed -300 at 2026-10-30T13:01:39.66Z (simulator), matched True

### `8beb1cd0b9c4` ofm.stage / move_z → **VERIFIED**

- asked by `agent:adaptive` (credential `tok_b1f0f2c44fc31c00`); invoke {"z": -150}
- authorization: require_approval by ['adaptive-z'], approved by operator:local
- device's own reference: `labthings:9e1cb4ca-0606-4523-ae63-2c23ccff0e24`
- lifecycle: CREATED (13:01:40.143) → VALIDATING (13:01:40.148) → AWAITING_APPROVAL (13:01:40.154) → APPROVED (13:01:40.171) → DISPATCHED (13:01:40.178) → ACCEPTED (13:01:40.2Z) → APPLIED (13:01:40.481) → VERIFIED (13:01:40.63Z)
- verification: readback; expected -150, observed -150 at 2026-10-30T13:01:40.63Z (simulator), matched True

### `56e337775983` ofm.stage / move_z → **VERIFIED**

- asked by `agent:adaptive` (credential `tok_b1f0f2c44fc31c00`); invoke {"z": 0}
- authorization: require_approval by ['adaptive-z'], approved by operator:local
- device's own reference: `labthings:4c3d5bad-f294-41b3-a15c-e73b06272b4c`
- lifecycle: CREATED (13:01:41.11Z) → VALIDATING (13:01:41.114) → AWAITING_APPROVAL (13:01:41.12Z) → APPROVED (13:01:41.125) → DISPATCHED (13:01:41.131) → ACCEPTED (13:01:41.166) → APPLIED (13:01:41.436) → VERIFIED (13:01:41.571)
- verification: readback; expected 0, observed 0 at 2026-10-30T13:01:41.57Z (simulator), matched True

### `9ac320d3375f` ofm.stage / move_z → **VERIFIED**

- asked by `agent:adaptive` (credential `tok_b1f0f2c44fc31c00`); invoke {"z": 150}
- authorization: require_approval by ['adaptive-z'], approved by operator:local
- device's own reference: `labthings:b35e72a0-6231-4ee8-a1bb-d24f9e5c9f54`
- lifecycle: CREATED (13:01:42.032) → VALIDATING (13:01:42.036) → AWAITING_APPROVAL (13:01:42.04Z) → APPROVED (13:01:42.045) → DISPATCHED (13:01:42.049) → ACCEPTED (13:01:42.086) → APPLIED (13:01:42.35Z) → VERIFIED (13:01:42.49Z)
- verification: readback; expected 150, observed 150 at 2026-10-30T13:01:42.489Z (simulator), matched True

### `2749d6850931` ofm.stage / move_z → **VERIFIED**

- asked by `agent:adaptive` (credential `tok_b1f0f2c44fc31c00`); invoke {"z": 300}
- authorization: require_approval by ['adaptive-z'], approved by operator:local
- device's own reference: `labthings:aece1357-2e5e-4fea-b958-2976c1129a19`
- lifecycle: CREATED (13:01:42.995) → VALIDATING (13:01:43Z) → AWAITING_APPROVAL (13:01:43.004) → APPROVED (13:01:43.009) → DISPATCHED (13:01:43.012) → ACCEPTED (13:01:43.05Z) → APPLIED (13:01:43.336) → VERIFIED (13:01:43.481)
- verification: readback; expected 300, observed 300 at 2026-10-30T13:01:43.48Z (simulator), matched True

### `c7b12b0a4800` ofm.stage / move_z → **VERIFIED**

- asked by `agent:adaptive` (credential `tok_b1f0f2c44fc31c00`); invoke {"z": 0}
- authorization: require_approval by ['adaptive-z'], approved by operator:local
- device's own reference: `labthings:6d3c622b-db8d-4fc8-ae9c-9ab47e2075a5`
- lifecycle: CREATED (13:01:44.507) → VALIDATING (13:01:44.511) → AWAITING_APPROVAL (13:01:44.514) → APPROVED (13:01:44.52Z) → DISPATCHED (13:01:44.524) → ACCEPTED (13:01:44.537) → APPLIED (13:01:44.932) → VERIFIED (13:01:45.052)
- verification: readback; expected 0, observed 0 at 2026-10-30T13:01:45.052Z (simulator), matched True

### `1506d5b91b1a` ofm.stage / move_y → **VERIFIED**

- asked by `agent:adaptive` (credential `tok_b1f0f2c44fc31c00`); invoke {"y": 250}
- authorization: require_approval by ['adaptive-y'], approved by operator:local
- device's own reference: `labthings:2432ce0a-9114-4231-9e59-f30309db1da9`
- lifecycle: CREATED (13:01:45.079) → VALIDATING (13:01:45.08Z) → AWAITING_APPROVAL (13:01:45.084) → APPROVED (13:01:45.089) → DISPATCHED (13:01:45.092) → ACCEPTED (13:01:45.102) → APPLIED (13:01:45.504) → VERIFIED (13:01:45.637)
- verification: readback; expected 250, observed 250 at 2026-10-30T13:01:45.637Z (simulator), matched True

### `6aed836f8b6f` ofm.stage / move_y → **VERIFIED**

- asked by `agent:adaptive` (credential `tok_b1f0f2c44fc31c00`); invoke {"y": 0}
- authorization: require_approval by ['adaptive-y'], approved by operator:local
- device's own reference: `labthings:8d900a5b-0f60-4773-8e1a-0c31903294bb`
- lifecycle: CREATED (13:01:46.079) → VALIDATING (13:01:46.081) → AWAITING_APPROVAL (13:01:46.083) → APPROVED (13:01:46.088) → DISPATCHED (13:01:46.091) → ACCEPTED (13:01:46.119) → APPLIED (13:01:46.401) → VERIFIED (13:01:46.561)
- verification: readback; expected 0, observed 0 at 2026-10-30T13:01:46.56Z (simulator), matched True

### `a5294f177054` ofm.stage / move_x → **VERIFIED**

- asked by `agent:adaptive` (credential `tok_b1f0f2c44fc31c00`); invoke {"x": 250}
- authorization: require_approval by ['adaptive-x'], approved by operator:local
- device's own reference: `labthings:ce02dd74-853a-4b16-96a5-80ebdb1b6bf1`
- lifecycle: CREATED (13:01:46.571) → VALIDATING (13:01:46.574) → AWAITING_APPROVAL (13:01:46.581) → APPROVED (13:01:46.595) → DISPATCHED (13:01:46.605) → ACCEPTED (13:01:46.64Z) → APPLIED (13:01:46.918) → VERIFIED (13:01:47.064)
- verification: readback; expected 250, observed 250 at 2026-10-30T13:01:47.064Z (simulator), matched True

### `914d5621f204` ofm.stage / move_x → **VERIFIED**

- asked by `agent:adaptive` (credential `tok_b1f0f2c44fc31c00`); invoke {"x": -250}
- authorization: require_approval by ['adaptive-x'], approved by operator:local
- device's own reference: `labthings:10019c1e-b5f3-4ffa-ba48-b40081a285a7`
- lifecycle: CREATED (13:01:47.548) → VALIDATING (13:01:47.552) → AWAITING_APPROVAL (13:01:47.56Z) → APPROVED (13:01:47.575) → DISPATCHED (13:01:47.585) → ACCEPTED (13:01:47.627) → APPLIED (13:01:48.208) → VERIFIED (13:01:48.354)
- verification: readback; expected -250, observed -250 at 2026-10-30T13:01:48.354Z (simulator), matched True

### `a741959c9bb0` ofm.stage / move_y → **VERIFIED**

- asked by `agent:adaptive` (credential `tok_b1f0f2c44fc31c00`); invoke {"y": -250}
- authorization: require_approval by ['adaptive-y'], approved by operator:local
- device's own reference: `labthings:9a4c4564-eaff-4967-afb6-1c4d34a10df1`
- lifecycle: CREATED (13:01:48.763) → VALIDATING (13:01:48.766) → AWAITING_APPROVAL (13:01:48.771) → APPROVED (13:01:48.782) → DISPATCHED (13:01:48.789) → ACCEPTED (13:01:48.837) → APPLIED (13:01:49.12Z) → VERIFIED (13:01:49.271)
- verification: readback; expected -250, observed -250 at 2026-10-30T13:01:49.27Z (simulator), matched True

### `09a0b6d02ee3` ofm.stage / move_x → **VERIFIED**

- asked by `agent:adaptive` (credential `tok_b1f0f2c44fc31c00`); invoke {"x": 0}
- authorization: require_approval by ['adaptive-x'], approved by operator:local
- device's own reference: `labthings:db4199b3-1d8e-4eb0-b3ef-de114315a5c0`
- lifecycle: CREATED (13:01:49.316) → VALIDATING (13:01:49.32Z) → AWAITING_APPROVAL (13:01:49.329) → APPROVED (13:01:49.339) → DISPATCHED (13:01:49.344) → ACCEPTED (13:01:49.359) → APPLIED (13:01:49.629) → VERIFIED (13:01:49.77Z)
- verification: readback; expected 0, observed 0 at 2026-10-30T13:01:49.77Z (simulator), matched True

### `28eef99842fb` ofm.stage / move_y → **VERIFIED**

- asked by `agent:adaptive` (credential `tok_b1f0f2c44fc31c00`); invoke {"y": 0}
- authorization: require_approval by ['adaptive-y'], approved by operator:local
- device's own reference: `labthings:5f16b5b9-eb94-4f42-b9f9-70bbe26c4ef2`
- lifecycle: CREATED (13:01:50.785) → VALIDATING (13:01:50.789) → AWAITING_APPROVAL (13:01:50.791) → APPROVED (13:01:50.795) → DISPATCHED (13:01:50.798) → ACCEPTED (13:01:50.836) → APPLIED (13:01:51.102) → VERIFIED (13:01:51.224)
- verification: readback; expected 0, observed 0 at 2026-10-30T13:01:51.223Z (simulator), matched True
