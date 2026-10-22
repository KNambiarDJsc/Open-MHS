# Quantities and units

Decision record: [ADR-0004](../docs/adr/0004-units.md).

## Rules

1. Units are [UCUM](https://ucum.org/) case-sensitive codes. `V` is a volt; `v` is not
   a unit.
2. Every numeric value on the wire carries its unit: `{"value": 5, "unit": "V"}`.
3. A unit not in the table below is rejected. Units are never inferred or defaulted.
4. A value may be given in any unit of the same quantity kind as the target; the
   gateway converts it with the table. Conversions across quantity kinds are refused.
5. Each unit converts to its kind's canonical unit as
   `canonical = value × scale + offset`. Only temperature units have a non-zero offset.
6. Adding a unit is a reviewed spec change with tests (round-trip property tests run
   over every pair of units of the same kind).

## Table (v0.1)

The reference implementation is `crates/openmhs-types/src/units.rs`; tests check that
it matches this table's canonical units.

| Quantity kind | Canonical | Other units |
|---|---|---|
| `dimensionless` | `1` | `%` |
| `length` | `m` | `km`, `cm`, `mm`, `um` |
| `mass` | `kg` | `g`, `mg` |
| `time` | `s` | `ms`, `us`, `min`, `h` |
| `frequency` | `Hz` | `kHz`, `MHz` |
| `temperature` | `K` | `Cel` (offset 273.15), `[degF]` |
| `electric_potential` | `V` | `mV`, `kV` |
| `electric_current` | `A` | `mA`, `uA` |
| `electric_resistance` | `Ohm` | `kOhm`, `MOhm` |
| `power` | `W` | `mW`, `kW` |
| `energy` | `J` | `kJ`, `W.h`, `kW.h` |
| `pressure` | `Pa` | `kPa`, `bar`, `mbar` |
| `angle` | `rad` | `deg` |
| `velocity` | `m/s` | `mm/s` |
| `angular_velocity` | `rad/s` | `deg/s` |
| `acceleration` | `m/s2` | `mm/s2` |
| `force` | `N` | `kN` |
| `torque` | `N.m` | — |
| `volume` | `m3` | `L`, `mL`, `uL` |
| `volumetric_flow` | `m3/s` | `L/min`, `mL/min`, `uL/min` |
| `illuminance` | `lx` | — |

## Known limitations

- No general UCUM expression parser: `mV/ms` is not accepted unless added to the table.
- Temperature *differences* (e.g. a slew of 2 `Cel` per second) are expressed in the
  capability's declared unit and are not offset-converted. Slew and tolerance values
  are always in the capability's own unit for this reason.
