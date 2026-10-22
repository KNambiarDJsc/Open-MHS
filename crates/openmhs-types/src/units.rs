//! Quantity kinds and the UCUM unit table (spec/units.md, ADR-0004).
//!
//! Every unit maps to its quantity kind's canonical unit by an affine transform:
//! `canonical = value * scale + offset`. Conversions only happen between units of
//! the same quantity kind and only through this table. Nothing is inferred.

use std::f64::consts::PI;

/// A row of the unit table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitDef {
    /// UCUM case-sensitive code.
    pub code: &'static str,
    /// Quantity kind this unit measures.
    pub quantity: &'static str,
    /// Multiplier to the quantity's canonical unit.
    pub scale: f64,
    /// Offset added after scaling (non-zero only for affine units such as `Cel`).
    pub offset: f64,
}

const fn u(code: &'static str, quantity: &'static str, scale: f64) -> UnitDef {
    UnitDef {
        code,
        quantity,
        scale,
        offset: 0.0,
    }
}

const fn affine(code: &'static str, quantity: &'static str, scale: f64, offset: f64) -> UnitDef {
    UnitDef {
        code,
        quantity,
        scale,
        offset,
    }
}

/// Quantity kinds and their canonical unit, in table order.
pub const QUANTITIES: &[(&str, &str)] = &[
    ("dimensionless", "1"),
    ("length", "m"),
    ("mass", "kg"),
    ("time", "s"),
    ("frequency", "Hz"),
    ("temperature", "K"),
    ("electric_potential", "V"),
    ("electric_current", "A"),
    ("electric_resistance", "Ohm"),
    ("power", "W"),
    ("energy", "J"),
    ("pressure", "Pa"),
    ("angle", "rad"),
    ("velocity", "m/s"),
    ("angular_velocity", "rad/s"),
    ("acceleration", "m/s2"),
    ("force", "N"),
    ("torque", "N.m"),
    ("volume", "m3"),
    ("volumetric_flow", "m3/s"),
    ("illuminance", "lx"),
];

/// The complete v0.1 unit table. Adding a unit is a reviewed change with tests.
pub const UNITS: &[UnitDef] = &[
    u("1", "dimensionless", 1.0),
    u("%", "dimensionless", 0.01),
    u("m", "length", 1.0),
    u("km", "length", 1e3),
    u("cm", "length", 1e-2),
    u("mm", "length", 1e-3),
    u("um", "length", 1e-6),
    u("kg", "mass", 1.0),
    u("g", "mass", 1e-3),
    u("mg", "mass", 1e-6),
    u("s", "time", 1.0),
    u("ms", "time", 1e-3),
    u("us", "time", 1e-6),
    u("min", "time", 60.0),
    u("h", "time", 3600.0),
    u("Hz", "frequency", 1.0),
    u("kHz", "frequency", 1e3),
    u("MHz", "frequency", 1e6),
    u("K", "temperature", 1.0),
    affine("Cel", "temperature", 1.0, 273.15),
    affine("[degF]", "temperature", 5.0 / 9.0, 459.67 * 5.0 / 9.0),
    u("V", "electric_potential", 1.0),
    u("mV", "electric_potential", 1e-3),
    u("kV", "electric_potential", 1e3),
    u("A", "electric_current", 1.0),
    u("mA", "electric_current", 1e-3),
    u("uA", "electric_current", 1e-6),
    u("Ohm", "electric_resistance", 1.0),
    u("kOhm", "electric_resistance", 1e3),
    u("MOhm", "electric_resistance", 1e6),
    u("W", "power", 1.0),
    u("mW", "power", 1e-3),
    u("kW", "power", 1e3),
    u("J", "energy", 1.0),
    u("kJ", "energy", 1e3),
    u("W.h", "energy", 3600.0),
    u("kW.h", "energy", 3.6e6),
    u("Pa", "pressure", 1.0),
    u("kPa", "pressure", 1e3),
    u("bar", "pressure", 1e5),
    u("mbar", "pressure", 1e2),
    u("rad", "angle", 1.0),
    u("deg", "angle", PI / 180.0),
    u("m/s", "velocity", 1.0),
    u("mm/s", "velocity", 1e-3),
    u("rad/s", "angular_velocity", 1.0),
    u("deg/s", "angular_velocity", PI / 180.0),
    u("m/s2", "acceleration", 1.0),
    u("mm/s2", "acceleration", 1e-3),
    u("N", "force", 1.0),
    u("kN", "force", 1e3),
    u("N.m", "torque", 1.0),
    u("m3", "volume", 1.0),
    u("L", "volume", 1e-3),
    u("mL", "volume", 1e-6),
    u("uL", "volume", 1e-9),
    u("m3/s", "volumetric_flow", 1.0),
    u("L/min", "volumetric_flow", 1e-3 / 60.0),
    u("mL/min", "volumetric_flow", 1e-6 / 60.0),
    u("uL/min", "volumetric_flow", 1e-9 / 60.0),
    u("lx", "illuminance", 1.0),
];

/// Look up a unit by its exact UCUM code.
pub fn unit(code: &str) -> Option<&'static UnitDef> {
    UNITS.iter().find(|d| d.code == code)
}

/// Whether `name` is a known quantity kind.
pub fn is_quantity(name: &str) -> bool {
    QUANTITIES.iter().any(|(q, _)| *q == name)
}

/// Why a conversion was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversionError {
    UnknownUnit(String),
    QuantityMismatch {
        from: &'static str,
        to: &'static str,
    },
}

impl std::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownUnit(code) => write!(f, "unknown unit '{code}'"),
            Self::QuantityMismatch { from, to } => {
                write!(f, "cannot convert {from} to {to}: different quantity kinds")
            }
        }
    }
}

impl std::error::Error for ConversionError {}

/// Convert `value` from unit `from` to unit `to`.
pub fn convert(value: f64, from: &str, to: &str) -> Result<f64, ConversionError> {
    let f = unit(from).ok_or_else(|| ConversionError::UnknownUnit(from.to_owned()))?;
    let t = unit(to).ok_or_else(|| ConversionError::UnknownUnit(to.to_owned()))?;
    if f.quantity != t.quantity {
        return Err(ConversionError::QuantityMismatch {
            from: f.quantity,
            to: t.quantity,
        });
    }
    if f.code == t.code {
        return Ok(value);
    }
    let canonical = value * f.scale + f.offset;
    Ok((canonical - t.offset) / t.scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn codes_are_unique_and_quantities_known() {
        for (i, a) in UNITS.iter().enumerate() {
            assert!(
                is_quantity(a.quantity),
                "{} has unknown quantity {}",
                a.code,
                a.quantity
            );
            assert!(
                UNITS[i + 1..].iter().all(|b| b.code != a.code),
                "duplicate unit {}",
                a.code
            );
        }
    }

    #[test]
    fn every_quantity_has_a_canonical_unit_with_unit_scale() {
        for (q, canonical) in QUANTITIES {
            let d = unit(canonical).expect("canonical unit in table");
            assert_eq!(d.quantity, *q);
            assert_eq!(
                (d.scale, d.offset),
                (1.0, 0.0),
                "canonical {canonical} must be identity"
            );
        }
    }

    #[test]
    fn known_conversions() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(close(convert(1500.0, "mV", "V").unwrap(), 1.5));
        assert!(close(convert(0.0, "Cel", "K").unwrap(), 273.15));
        assert!(close(convert(212.0, "[degF]", "Cel").unwrap(), 100.0));
        assert!(close(convert(180.0, "deg", "rad").unwrap(), PI));
        assert!(close(convert(1.0, "L/min", "mL/min").unwrap(), 1000.0));
    }

    #[test]
    fn refuses_unknown_and_mismatched() {
        assert_eq!(
            convert(1.0, "volts", "V"),
            Err(ConversionError::UnknownUnit("volts".into()))
        );
        assert!(matches!(
            convert(1.0, "V", "A"),
            Err(ConversionError::QuantityMismatch { .. })
        ));
        // UCUM codes are case-sensitive.
        assert!(convert(1.0, "v", "V").is_err());
    }

    /// Pairs of units that share a quantity kind.
    fn same_quantity_pair() -> impl Strategy<Value = (&'static UnitDef, &'static UnitDef)> {
        (0..UNITS.len()).prop_flat_map(|a| {
            let same: Vec<&'static UnitDef> = UNITS
                .iter()
                .filter(|b| b.quantity == UNITS[a].quantity)
                .collect();
            (Just(&UNITS[a]), proptest::sample::select(same))
        })
    }

    proptest! {
        #[test]
        fn round_trip_within_each_quantity((a, b) in same_quantity_pair(), v in -1e6f64..1e6) {
            let there = convert(v, a.code, b.code).unwrap();
            let back = convert(there, b.code, a.code).unwrap();
            prop_assert!((back - v).abs() <= 1e-6 * v.abs().max(1.0), "{} -> {} -> {}", v, there, back);
        }
    }
}
