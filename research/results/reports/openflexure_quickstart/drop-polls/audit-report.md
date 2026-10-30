# Experiment report

Generated 2026-10-30T09:56:15+0530.

> A view of the gateway's journal. It is only as trustworthy as the journal; the audit chain detects alteration and reordering of the records present, not removal of the newest ones (compare the anchor) and not a gateway that lied. It records the device's reports, not the physical truth.

## Audit chain

- records: **46**, chain valid: **True**
- anchor (keep this somewhere the gateway's writer cannot reach): seq 46, hash `1c41ff1d56c3938e…`

## Device contracts in force

- seq 2 `ofm.stage` adapter `openmhs.py.wot` descriptor sha256 `6c9ca131e9cfce3c`
- seq 10 `ofm.stage` adapter `openmhs.py.wot` descriptor sha256 `6c9ca131e9cfce3c`
- seq 29 `ofm.stage` adapter `openmhs.py.wot` descriptor sha256 `6c9ca131e9cfce3c`

## Commands

### `4a47c169a391` ofm.stage / move_x → **RECONCILED_APPLIED**

- asked by `agent:demo` (credential `tok_22945e06bf7fbf6b`); invoke {"x": 50}
- authorization: require_approval by ['demo-stage-approval'], approved by operator:local
- device's own reference: `labthings:08f9b307-f046-4b25-ad9c-a3bdc6d2bba9`
- lifecycle: CREATED (04:23:07.442) → VALIDATING (04:23:07.444) → AWAITING_APPROVAL (04:23:07.446) → APPROVED (04:23:07.46Z) → DISPATCHED (04:23:07.463) → ACCEPTED (04:23:07.497) → UNKNOWN_OUTCOME (04:23:07.625) → RECONCILED_APPLIED (04:23:07.684)
- verification: readback; expected 50, observed 50 at 2026-10-30T04:23:07.681Z (simulator), matched True
- device error: lost contact with invocation 08f9b307-f046-4b25-ad9c-a3bdc6d2bba9 (http://127.0.0.1:61478/api/v3/action_invocations/08f9b307-f046-4b25-ad9c-a3bdc6d2bba9) after it was accepted (Remote end closed connection without response); its outcome is unknown

### `3a9e7bc119c8` ofm.stage / move_x → **RECONCILED_APPLIED**

- asked by `agent:demo` (credential `tok_cd9b613607c461b5`); invoke {"x": -50}
- authorization: require_approval by ['demo-stage-approval'], approved by operator:local
- device's own reference: `labthings:47049b3d-01c1-4095-8e35-1dc4fbcbc5ca`
- lifecycle: CREATED (04:26:15.091) → VALIDATING (04:26:15.093) → AWAITING_APPROVAL (04:26:15.096) → APPROVED (04:26:15.11Z) → DISPATCHED (04:26:15.113) → ACCEPTED (04:26:15.157) → UNKNOWN_OUTCOME (04:26:15.282) → RECONCILED_APPLIED (04:26:15.34Z)
- verification: readback; expected -50, observed -50 at 2026-10-30T04:26:15.338Z (simulator), matched True
- device error: lost contact with invocation 47049b3d-01c1-4095-8e35-1dc4fbcbc5ca (http://127.0.0.1:55022/api/v3/action_invocations/47049b3d-01c1-4095-8e35-1dc4fbcbc5ca) after it was accepted (Remote end closed connection without response); its outcome is unknown
