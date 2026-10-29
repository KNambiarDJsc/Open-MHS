# Command state machine

The authoritative state list and legal edges are in
[`crates/openmhs-types/src/lifecycle.rs`](../../crates/openmhs-types/src/lifecycle.rs),
with the wire meanings and recovery rules in [`spec/lifecycle.md`](../../spec/lifecycle.md).
The table is the implementation baseline; this note makes its failure interpretation easy
to find.

```text
CREATED -> VALIDATING -> DENIED | AWAITING_APPROVAL -> APPROVED
                                      |                   |
                                      +-> DENIED/EXPIRED  +-> DISPATCHED
                                                             |
                    +-------------------+--------------------+----------------+
                    v                   v                    v                v
                REJECTED            ACCEPTED             FAILED/TIMED_OUT   UNKNOWN_OUTCOME
                                        |                       |                |
                                      APPLIED                   +-> UNKNOWN     +-> RECONCILED_*
                                        |
                         VERIFIED / VERIFICATION_FAILED / UNVERIFIABLE
```

An acknowledgement is not completion. `ACCEPTED` and `APPLIED` remain pending. `VERIFIED`
requires qualifying readback. `UNVERIFIABLE` records a reported application without that
readback. Timeout, disconnect, malformed replies and restart windows may leave the physical
effect uncertain; no automatic resend follows an ambiguous dispatch.

## Persistence and identity

`commands` has a stable command id, principal, device, capability, optional idempotency key,
request hash, state, creation time and serialized receipt. A partial unique index enforces
idempotency keys per principal/device. Same-key/same-request returns the stored command;
same-key/different-request reports a conflict. Each transition updates the receipt/state and
appends audit data transactionally.

The current request ID is gateway-generated from the accepted command record. Native adapter
references are captured in the adapter result/receipt where supplied. Neither is a universal
hardware idempotency primitive: device-side deduplication depends on adapter/controller
support.

## Recovery boundary

On startup, in-flight records that might have reached the adapter become `UNKNOWN_OUTCOME`.
`APPLIED` can resume verification after the device is registered. Pre-dispatch work is
closed or left awaiting a human according to the lifecycle specification. Reconciliation
uses post-dispatch readback where available; otherwise an operator can resolve with a
recorded reason. Recovery never infers success from an absent record or missing response.

## Required future verification

The existing transition matrix test checks the finite legal-edge table. Additional model or
property tests should exercise generated event histories, crash points around journal/send
boundaries, storage exhaustion, and restart recovery. These tests are not implied by the
existing lifecycle unit test or the earlier simulator study.
