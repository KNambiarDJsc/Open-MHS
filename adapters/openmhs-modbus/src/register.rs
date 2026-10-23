//! The register map: where each capability lives, and how its bits become a value.

use openmhs_types::descriptor::{Capability, CapabilityKind, Operation, ValueSpec, ValueType};
use serde::Deserialize;

use openmhs_profile::Mapping;

/// The four Modbus data tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Table {
    /// Read-write bits (function codes 1, 5, 15).
    Coil,
    /// Read-only bits (2).
    DiscreteInput,
    /// Read-only 16-bit words (4).
    Input,
    /// Read-write 16-bit words (3, 6, 16).
    Holding,
}

impl Table {
    pub fn is_bit(self) -> bool {
        matches!(self, Table::Coil | Table::DiscreteInput)
    }

    pub fn is_writable(self) -> bool {
        matches!(self, Table::Coil | Table::Holding)
    }
}

/// How the words of a register value are interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    U16,
    I16,
    U32,
    I32,
    F32,
}

/// For 32-bit values: which word comes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WordOrder {
    /// High word first (the Modbus convention).
    #[default]
    Big,
    /// Low word first, as some devices do.
    Little,
}

fn one() -> f64 {
    1.0
}

/// Where one capability lives. A property's value is `raw × scale + offset`. An action
/// writes one register or coil: an action parameter (`param`), or a fixed raw value
/// (`write`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Register {
    pub table: Table,
    pub address: u16,
    #[serde(default, rename = "type")]
    pub kind: Kind,
    #[serde(default)]
    pub word_order: WordOrder,
    #[serde(default = "one")]
    pub scale: f64,
    #[serde(default)]
    pub offset: f64,
    #[serde(default)]
    pub param: Option<String>,
    #[serde(default)]
    pub write: Option<serde_json::Value>,
}

impl Register {
    /// How many registers (or coils) the value occupies.
    pub fn width(&self) -> u16 {
        match (self.table.is_bit(), self.kind) {
            (true, _) | (false, Kind::U16 | Kind::I16) => 1,
            (false, _) => 2,
        }
    }

    /// The value spec this register carries for `cap`: the property's value, or the
    /// action parameter's.
    pub fn spec<'a>(&self, cap: &'a Capability) -> Option<&'a ValueSpec> {
        match &self.param {
            Some(p) => cap.params.as_ref()?.get(p),
            None => cap.value.as_ref(),
        }
    }

    /// Words as an engineering number: `raw × scale + offset`.
    pub fn decode(&self, words: &[u16]) -> Result<f64, String> {
        if words.len() != usize::from(self.width()) {
            return Err(format!(
                "expected {} register(s), got {}",
                self.width(),
                words.len()
            ));
        }
        let pair = || -> u32 {
            let (hi, lo) = match self.word_order {
                WordOrder::Big => (words[0], words[1]),
                WordOrder::Little => (words[1], words[0]),
            };
            (u32::from(hi) << 16) | u32::from(lo)
        };
        let raw = match self.kind {
            Kind::U16 => f64::from(words[0]),
            #[allow(clippy::cast_possible_wrap)]
            Kind::I16 => f64::from(words[0] as i16),
            Kind::U32 => f64::from(pair()),
            #[allow(clippy::cast_possible_wrap)]
            Kind::I32 => f64::from(pair() as i32),
            Kind::F32 => f64::from(f32::from_bits(pair())),
        };
        // Drop binary noise from the scale (0.1 × 215 = 21.500000000000004).
        Ok(((raw * self.scale + self.offset) * 1e9).round() / 1e9)
    }

    /// An engineering number as words. Refuses values the register cannot hold exactly
    /// enough: nothing is silently rounded or clipped.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn encode(&self, x: f64) -> Result<Vec<u16>, String> {
        let raw = (x - self.offset) / self.scale;
        let split = |v: u32| -> Vec<u16> {
            let (hi, lo) = ((v >> 16) as u16, v as u16);
            match self.word_order {
                WordOrder::Big => vec![hi, lo],
                WordOrder::Little => vec![lo, hi],
            }
        };
        let integer = |min: f64, max: f64| -> Result<i64, String> {
            let r = raw.round();
            if (raw - r).abs() > 1e-6 * raw.abs().max(1.0) {
                return Err(format!(
                    "{x} is not a multiple of the register's resolution {}",
                    self.scale
                ));
            }
            if r < min || r > max {
                return Err(format!(
                    "{x} does not fit the register (raw {r} outside {min}..{max})"
                ));
            }
            Ok(r as i64)
        };
        Ok(match self.kind {
            Kind::U16 => vec![integer(0.0, 65535.0)? as u16],
            Kind::I16 => vec![integer(-32768.0, 32767.0)? as i16 as u16],
            Kind::U32 => split(integer(0.0, 4_294_967_295.0)? as u32),
            Kind::I32 => split(integer(-2_147_483_648.0, 2_147_483_647.0)? as i32 as u32),
            Kind::F32 => {
                let f = raw as f32;
                if !f.is_finite() {
                    return Err(format!("{x} does not fit a 32-bit float"));
                }
                split(f.to_bits())
            }
        })
    }
}

impl Mapping for Register {
    fn check(&self, cap: &Capability) -> Result<(), String> {
        if self.scale == 0.0 || !self.scale.is_finite() || !self.offset.is_finite() {
            return Err("'scale' must be a non-zero number".into());
        }
        if self.address.checked_add(self.width() - 1).is_none() {
            return Err("the value runs past address 65535".into());
        }
        if self.table.is_bit()
            && (self.kind != Kind::U16 || self.scale != 1.0 || self.offset != 0.0)
        {
            return Err(
                "coils and discrete inputs are single bits: no 'type', 'scale' or 'offset'".into(),
            );
        }
        let writes = cap.operations.contains(&Operation::Write)
            || cap.operations.contains(&Operation::Invoke);
        if writes && !self.table.is_writable() {
            return Err("only coils and holding registers can be written".into());
        }
        match cap.kind {
            CapabilityKind::Action => {
                match (&self.param, &self.write) {
                    (Some(p), None) => {
                        if !cap.params.as_ref().is_some_and(|ps| ps.contains_key(p)) {
                            return Err(format!(
                                "'param' names '{p}', which is not a parameter of the action"
                            ));
                        }
                    }
                    (None, Some(w)) => {
                        let ok = if self.table.is_bit() {
                            w.is_boolean()
                        } else {
                            w.as_f64().is_some()
                        };
                        if !ok {
                            return Err("'write' must be true/false for a coil, or a raw number for a register".into());
                        }
                    }
                    _ => {
                        return Err(
                            "an action writes exactly one thing: set 'param' or 'write'".into()
                        );
                    }
                }
                let others = cap
                    .params
                    .iter()
                    .flatten()
                    .filter(|(n, _)| Some(*n) != self.param.as_ref())
                    .count();
                if others > 0 {
                    return Err(
                        "an action can carry at most one parameter, the one in 'param'".into(),
                    );
                }
            }
            _ => {
                if self.param.is_some() || self.write.is_some() {
                    return Err("'param' and 'write' are for actions".into());
                }
            }
        }
        if let Some(spec) = self.spec(cap) {
            match (self.table.is_bit(), spec.ty) {
                (true, ValueType::Boolean)
                | (
                    false,
                    ValueType::Number | ValueType::Integer | ValueType::Enum | ValueType::Boolean,
                ) => {}
                (true, _) => return Err("a coil or discrete input carries a boolean".into()),
                (false, ValueType::String) => return Err("registers cannot carry strings".into()),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(json: serde_json::Value) -> Register {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn values_round_trip_through_every_register_type() {
        let cases = [
            (
                serde_json::json!({"table": "holding", "address": 0, "type": "i16", "scale": 0.1}),
                -12.5,
            ),
            (
                serde_json::json!({"table": "holding", "address": 0, "type": "u32", "scale": 0.01}),
                70000.25,
            ),
            (
                serde_json::json!({"table": "holding", "address": 0, "type": "i32", "word_order": "little"}),
                -100000.0,
            ),
            (
                serde_json::json!({"table": "input", "address": 0, "type": "f32"}),
                3.5,
            ),
            (
                serde_json::json!({"table": "holding", "address": 0, "scale": 2, "offset": -10}),
                30.0,
            ),
        ];
        for (json, x) in cases {
            let r = reg(json.clone());
            let words = r.encode(x).unwrap();
            assert_eq!(r.decode(&words).unwrap(), x, "{json}");
        }
    }

    #[test]
    fn values_a_register_cannot_hold_are_refused() {
        let r = reg(serde_json::json!({"table": "holding", "address": 0, "scale": 0.1}));
        assert!(r.encode(1.25).unwrap_err().contains("resolution"));
        assert!(r.encode(-1.0).unwrap_err().contains("does not fit"));
        assert!(r.encode(7000.0).unwrap_err().contains("does not fit"));
        assert_eq!(r.decode(&[215]).unwrap(), 21.5);
        let big = reg(serde_json::json!({"table": "holding", "address": 0, "type": "u32"}));
        assert_eq!(big.encode(65537.0).unwrap(), [1, 1]);
        assert!(big.decode(&[1]).is_err());
    }
}
