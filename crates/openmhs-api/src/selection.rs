//! Capability-based hardware selection: which registered devices can do what is needed.
//!
//! A pure function over descriptors and availability, so an agent can ask "what can
//! measure voltage and stream it" or "what can be set to 5 V" instead of reading every
//! descriptor. It selects candidates; it grants nothing. Whether a command is allowed is
//! still decided by the gateway when it is checked or sent (`check_command`).

use openmhs_types::descriptor::{CapabilityKind, Operation, SideEffect};
use openmhs_types::{Availability, Descriptor, DeviceStatus};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::insights::standard_formats;

/// What is needed. Every field that is set must hold; unset fields match anything.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilityQuery {
    /// Capability name, `*` patterns allowed, e.g. "*.voltage.*".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The physical quantity the descriptor declares, e.g. "voltage", "temperature".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<String>,
    /// The unit the descriptor declares, e.g. "V", "Cel".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// An operation it must support: "read", "write", "subscribe" or "invoke".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub operation: Option<Operation>,
    /// A value it must be able to take, in its declared unit: inside the declared range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// The highest side effect acceptable: "none", "reversible", "irreversible" or
    /// "hazardous" (default: any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub max_side_effect: Option<SideEffect>,
    /// Only devices that are not OFFLINE.
    #[serde(default)]
    pub available_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityMatch {
    pub device: String,
    pub capability: String,
    #[schemars(with = "String")]
    pub kind: CapabilityKind,
    #[schemars(with = "Vec<String>")]
    pub operations: Vec<Operation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[schemars(with = "String")]
    pub side_effect: SideEffect,
    /// `ONLINE`, `DEGRADED` or `OFFLINE` when the search ran.
    #[schemars(with = "String")]
    pub availability: Availability,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = standard_formats)]
pub struct Selection {
    pub matches: Vec<CapabilityMatch>,
    /// Devices examined.
    pub devices: usize,
}

/// Matches, available devices first, then by device and capability name.
pub fn select(
    descriptors: &[Descriptor],
    statuses: &[DeviceStatus],
    q: &CapabilityQuery,
) -> Selection {
    let mut matches = Vec::new();
    for d in descriptors {
        let availability = statuses
            .iter()
            .find(|s| s.device == d.device.id)
            .map_or(Availability::Online, |s| s.availability);
        if q.available_only && availability == Availability::Offline {
            continue;
        }
        for c in &d.capabilities {
            let spec = c.value.as_ref();
            let range = spec.and_then(|v| v.range.as_ref());
            let ok = q
                .name
                .as_deref()
                .is_none_or(|p| openmhs_policy::glob_matches(p, &c.name))
                && q.quantity
                    .as_deref()
                    .is_none_or(|want| spec.and_then(|v| v.quantity.as_deref()) == Some(want))
                && q.unit
                    .as_deref()
                    .is_none_or(|want| spec.and_then(|v| v.unit.as_deref()) == Some(want))
                && q.operation.is_none_or(|op| c.supports(op))
                && q.max_side_effect.is_none_or(|m| c.side_effect <= m)
                && q.value.is_none_or(|x| {
                    // A value needs a declared range to be judged; none means "cannot say".
                    range.is_some_and(|r| {
                        r.min.is_none_or(|lo| x >= lo) && r.max.is_none_or(|hi| x <= hi)
                    })
                });
            if ok {
                matches.push(CapabilityMatch {
                    device: d.device.id.clone(),
                    capability: c.name.clone(),
                    kind: c.kind,
                    operations: c.operations.clone(),
                    quantity: spec.and_then(|v| v.quantity.clone()),
                    unit: spec.and_then(|v| v.unit.clone()),
                    min: range.and_then(|r| r.min),
                    max: range.and_then(|r| r.max),
                    side_effect: c.side_effect,
                    availability,
                });
            }
        }
    }
    matches.sort_by(|a, b| {
        (a.availability == Availability::Offline)
            .cmp(&(b.availability == Availability::Offline))
            .then_with(|| a.device.cmp(&b.device))
            .then_with(|| a.capability.cmp(&b.capability))
    });
    Selection {
        matches,
        devices: descriptors.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lab() -> Vec<Descriptor> {
        [
            include_str!("../../../spec/examples/valid/power-supply.json"),
            include_str!("../../../spec/examples/valid/temperature-sensor.json"),
            include_str!("../../../spec/examples/valid/linear-actuator.json"),
        ]
        .iter()
        .map(|s| openmhs_types::validate_descriptor(&serde_json::from_str(s).unwrap()).unwrap())
        .collect()
    }

    fn names(s: &Selection) -> Vec<String> {
        s.matches
            .iter()
            .map(|m| format!("{}/{}", m.device, m.capability))
            .collect()
    }

    #[test]
    fn finds_what_can_be_streamed_in_a_unit() {
        let s = select(
            &lab(),
            &[],
            &CapabilityQuery {
                unit: Some("V".into()),
                operation: Some(Operation::Subscribe),
                ..CapabilityQuery::default()
            },
        );
        assert_eq!(names(&s), ["sim.psu-01/power.output.voltage.measured"]);
        assert_eq!(s.devices, 3);
    }

    #[test]
    fn a_value_must_fit_the_declared_range() {
        let q = |v| CapabilityQuery {
            unit: Some("V".into()),
            operation: Some(Operation::Write),
            value: Some(v),
            ..CapabilityQuery::default()
        };
        assert_eq!(
            names(&select(&lab(), &[], &q(5.0))),
            ["sim.psu-01/power.output.voltage.setpoint"]
        );
        assert!(select(&lab(), &[], &q(48.0)).matches.is_empty());
    }

    #[test]
    fn side_effects_and_availability_narrow_the_choice() {
        let writable = CapabilityQuery {
            operation: Some(Operation::Write),
            max_side_effect: Some(SideEffect::None),
            ..CapabilityQuery::default()
        };
        assert!(
            select(&lab(), &[], &writable)
                .matches
                .iter()
                .all(|m| m.side_effect == SideEffect::None)
        );

        let offline = DeviceStatus {
            device: "sim.psu-01".into(),
            availability: Availability::Offline,
            reason: None,
            since: openmhs_types::Timestamp::from_millis(0),
            checked_at: openmhs_types::Timestamp::from_millis(0),
            busy: false,
            lease: None,
        };
        let any_volts = CapabilityQuery {
            unit: Some("V".into()),
            ..CapabilityQuery::default()
        };
        let all = select(&lab(), std::slice::from_ref(&offline), &any_volts);
        assert!(
            all.matches
                .iter()
                .all(|m| m.availability == Availability::Offline)
        );
        let up = select(
            &lab(),
            &[offline],
            &CapabilityQuery {
                available_only: true,
                ..any_volts
            },
        );
        assert!(up.matches.is_empty());
    }
}
