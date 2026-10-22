# ADR-0004: UCUM unit codes, exact match or explicit conversion, never inference

- Status: Accepted
- Date: 2026-10-23

## Context

A command of `5` to a voltage capability is ambiguous (V? mV? kV?). Unit mistakes
are a classic cause of physical incidents. Compile-time unit crates (e.g. `uom`)
don't help with values arriving as JSON at runtime.

## Decision

- Descriptors and commands carry units as **UCUM** case-sensitive codes
  (`V`, `mV`, `A`, `Cel`, `K`, `mm`, `deg`, `s`, `ms`, …).
- Each numeric capability declares a quantity kind (e.g. `electric_potential`) and a
  canonical unit.
- A command value must state its unit. The gateway accepts it if the unit equals the
  canonical unit, or if an **explicit, tested conversion** exists between the two
  within the same quantity kind (e.g. `mV → V`). Otherwise it is rejected.
- Unknown unit codes are rejected. Units are never inferred or defaulted.
- Affine conversions (e.g. `Cel ↔ K`) are handled explicitly in the table; no
  general UCUM expression parser in v0.1.

## Alternatives considered

- **Full UCUM parser.** Correct but large; most devices need a small set of units.
  Can be added later behind the same API.
- **Unitless numbers with documented units.** Exactly the ambiguity we want to remove.

## Consequences

- The conversion table is small, auditable and property-tested (round trips).
- Adding a unit is a reviewed code change, which is intended.
