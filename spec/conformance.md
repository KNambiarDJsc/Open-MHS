# Conformance levels

Conformance describes how an **adapter + descriptor** behaves through the OpenMHS
contract under test. It is cumulative: L2 includes L1 and L0. The executable suite is
built in M5; this document fixes what each level means.

**A conformance level is not a safety certification.** It says nothing about whether a
device is safe to operate, only that its integration behaves as specified.

| Level | Name | Requirements |
|---|---|---|
| L0 | Discoverable | Valid descriptor; adapter connects, reports health, describes capabilities matching the descriptor; clean shutdown |
| L1 | Observable | Reads of every readable property return typed values in the declared unit, with timestamp, source and quality; subscriptions deliver updates; disconnects surface as `BAD` quality or `adapter.disconnected`, never as stale-but-`GOOD` values |
| L2 | Controllable | Every write/invoke is honoured within declared timing; readback verification behaves as declared; device rejections map to `REJECTED`; timeouts map to `UNKNOWN_OUTCOME`; the adapter never retries a non-idempotent command on its own; malformed responses map to `adapter.malformed_response` |
| L3 | Coordinated | Participates in multi-device plans with resource leases (defined in M9) |

Fault behaviour (disconnects, timeouts, malformed data, crashes) is part of every level
above L0 and is tested by fault injection, not assumed.
