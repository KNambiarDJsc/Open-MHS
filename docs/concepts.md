# Core concepts

A short vocabulary. Each term links to the specification, which is the authority; nothing
here restates it in detail.

## Devices and capabilities

A **device** is one piece of equipment, described by a **descriptor**: a JSON document
that says what it is and what it can do ([spec/descriptor.md](../spec/descriptor.md)). What
it can do is a list of **capabilities**. A capability is a *property* (read, write,
subscribe) or an *action* (invoke), and carries its own contract:

- the value's **type and unit**, with a valid range ([units](../spec/units.md));
- **preconditions**, such as "only if the output is not tripped";
- **freshness**: how old a reading may be and still be trusted;
- **limits** on rate and slew, and a **side-effect class** (none, reversible, hazardous);
- how the result is **verified**, usually by reading a property back.

An agent can only ask for what a descriptor offers, in the units it declares.

## Adapters

An **adapter** turns the contract into the device's own protocol and back: Modbus
registers, SCPI strings, HTTP calls, a vendor library. It lives behind the gateway and
never decides policy. Adapters are written in Rust (compiled in), described by a JSON
**profile** (Modbus TCP, MQTT, SCPI, HTTP), or run as a supervised child process in any
language ([adapter guide](adapters/README.md), [adapter protocol](../spec/adapter-protocol.md)).
Whether an adapter tells the truth is checked by the [conformance suite](../spec/conformance.md).

## Commands

Every write or invocation is a **command** with a lifecycle: created, validated, approved,
dispatched, applied, verified ([spec/lifecycle.md](../spec/lifecycle.md)). Commands are
journaled before they are sent, are idempotent by key, and have a deadline. A command
that is refused is a **receipt**, not an error: it says which rule refused it and what
would be accepted.

## Authorization

Nothing is allowed by default. The operator grants a **principal** (an agent, a person)
the right to do an **operation** on a device's capability; some grants require **human
approval**. Agents hold **scoped credentials** that can never exceed their grants, and
cannot change policy or approve their own commands ([spec/authorization.md](../spec/authorization.md)).
The policy is deterministic code, not a prompt.

## Command outcomes

A command ends in one of a small set of states. The one to understand is
**`UNKNOWN_OUTCOME`**: the gateway sent a command and did not learn whether it took
effect (a timeout, a dropped link, a crashed adapter). It is never reported as failed and
never retried blindly. You **reconcile** it by reading the device back, and the result is
`RECONCILED_APPLIED` or `RECONCILED_NOT_APPLIED`. Success itself means `VERIFIED`: the
device was read back and matched ([lifecycle](../spec/lifecycle.md), [errors](../spec/errors.md)).

## Coordination

Devices can be related in a **capability graph** (containment, shared resources, frames),
held under **leases**, and driven by **plans**: ordered steps across devices, each an
ordinary command, with a declared response to failure (halt, compensate, ask a person)
([topology](../spec/topology.md), [plans](../spec/plans.md)).

## What OpenMHS is not

It is not a real-time controller and not a safety system: hardware interlocks and
emergency stops keep final authority. See [Limitations](limitations.md).
