//! The device behind every protocol simulator: state derived from the descriptor, with
//! conformance faults on demand.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use openmhs_conformance::{Fault, Fixture};
use openmhs_types::Descriptor;
use openmhs_types::descriptor::{
    Capability, CompareOp, PreconditionValue, ValueType, Verification,
};
use openmhs_types::value::{Quantity, Value};

use crate::codec;

#[derive(Debug)]
struct State {
    values: BTreeMap<String, Value>,
    link_down: bool,
    fault: Option<Fault>,
    /// State-changing requests received, applied or not.
    deliveries: u64,
}

/// What the device does with one state-changing request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Applied; answer normally.
    Applied,
    /// Refused and not applied; answer with the reason.
    Refused(String),
    /// Send no answer. The request may or may not have been applied.
    Silent,
    /// Applied; answer with something unreadable.
    Garbled,
}

/// A device that behaves as its descriptor says: properties hold values, writes are
/// checked against range, declared values and preconditions, and an action takes
/// effect by making its declared readback true (`axis.home` sets `axis.homed`).
///
/// Simulators translate protocol requests into [`read`](Self::read) and
/// [`command`](Self::command) calls. Implements [`Fixture`] for the conformance suite.
#[derive(Debug)]
pub struct VirtualDevice {
    descriptor: Descriptor,
    state: Mutex<State>,
}

impl VirtualDevice {
    /// A device in its initial state: `initial` values, else defaults from the
    /// descriptor.
    pub fn new(descriptor: Descriptor, initial: &BTreeMap<String, Value>) -> Arc<Self> {
        let mut values = BTreeMap::new();
        for cap in &descriptor.capabilities {
            if let Some(spec) = &cap.value {
                let v = initial
                    .get(&cap.name)
                    .cloned()
                    .unwrap_or_else(|| default(spec));
                values.insert(cap.name.clone(), v);
            }
        }
        Arc::new(Self {
            descriptor,
            state: Mutex::new(State {
                values,
                link_down: false,
                fault: None,
                deliveries: 0,
            }),
        })
    }

    pub fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether the link is down: the simulator must not answer at all.
    pub fn link_down(&self) -> bool {
        self.lock().link_down
    }

    /// The current value of a property.
    pub fn read(&self, capability: &str) -> Option<Value> {
        self.lock().values.get(capability).cloned()
    }

    /// Change a value directly, as the physical world would (a measured value drifting).
    /// Not a request: no faults, no checks beyond the type.
    pub fn set(&self, capability: &str, value: Value) {
        self.lock().values.insert(capability.to_owned(), value);
    }

    /// Handle one state-changing request: a write (`value`) or an action (`params`).
    /// Counts the delivery and applies the active fault.
    pub fn command(
        &self,
        capability: &str,
        value: Option<Value>,
        params: &BTreeMap<String, Value>,
    ) -> Outcome {
        let mut st = self.lock();
        st.deliveries += 1;
        let fault = st.fault;
        match fault {
            Some(Fault::Refuse) => {
                return Outcome::Refused("refused by the device (fault injection)".into());
            }
            Some(Fault::NoResponse) => return Outcome::Silent,
            _ => {}
        }
        if let Err(reason) = self.apply(&mut st, capability, value, params) {
            return Outcome::Refused(reason);
        }
        match fault {
            Some(Fault::LostAck) => Outcome::Silent,
            Some(Fault::Garbled) => Outcome::Garbled,
            _ => Outcome::Applied,
        }
    }

    fn apply(
        &self,
        st: &mut State,
        capability: &str,
        value: Option<Value>,
        params: &BTreeMap<String, Value>,
    ) -> Result<(), String> {
        let cap = self
            .descriptor
            .capability(capability)
            .ok_or_else(|| format!("no capability '{capability}'"))?;
        for p in &cap.preconditions {
            let current = st.values.get(&p.capability);
            if !current.is_some_and(|c| holds(c, p.op, &p.value)) {
                return Err(format!(
                    "precondition '{}' does not hold{}",
                    p.id,
                    p.description
                        .as_deref()
                        .map(|d| format!(": {d}"))
                        .unwrap_or_default()
                ));
            }
        }
        match value {
            Some(v) => {
                let spec = cap.value.as_ref().ok_or("not a property with a value")?;
                codec::check(spec, &v)?;
                st.values.insert(cap.name.clone(), v);
            }
            None => {
                for (name, spec) in cap.params.iter().flatten() {
                    let v = params
                        .get(name)
                        .ok_or_else(|| format!("missing parameter '{name}'"))?;
                    codec::check(spec, v).map_err(|e| format!("{name}: {e}"))?;
                }
                if let Some((target, v)) = effect(cap, params) {
                    st.values.insert(target, v);
                }
            }
        }
        Ok(())
    }
}

/// What an action does to the device's state: its declared readback becomes true.
fn effect(cap: &Capability, params: &BTreeMap<String, Value>) -> Option<(String, Value)> {
    match &cap.verification {
        Some(Verification::Readback {
            capability: Some(target),
            expect,
            expect_param,
            ..
        }) => {
            let v = match (expect, expect_param) {
                (Some(v), _) => v.clone(),
                (None, Some(p)) => params.get(p)?.clone(),
                _ => return None,
            };
            Some((target.clone(), v))
        }
        _ => None,
    }
}

fn default(spec: &openmhs_types::descriptor::ValueSpec) -> Value {
    let mid = spec.range.map_or(0.0, |r| match (r.min, r.max) {
        (Some(a), Some(b)) => (a + b) / 2.0,
        (Some(a), None) => a,
        (None, Some(b)) => b.min(0.0),
        (None, None) => 0.0,
    });
    match spec.ty {
        ValueType::Number => Value::Quantity(Quantity {
            value: mid,
            unit: spec.unit.clone().unwrap_or_default(),
        }),
        #[allow(clippy::cast_possible_truncation)]
        ValueType::Integer => Value::Integer(mid.ceil() as i64),
        ValueType::Boolean => Value::Bool(false),
        ValueType::Enum => Value::Text(
            spec.values
                .iter()
                .flatten()
                .next()
                .cloned()
                .unwrap_or_default(),
        ),
        ValueType::String => Value::Text(String::new()),
    }
}

/// Whether `current` satisfies a precondition. Quantities in other units are converted.
fn holds(current: &Value, op: CompareOp, want: &PreconditionValue) -> bool {
    let cmp = |w: &Value| -> Option<std::cmp::Ordering> {
        match (current, w) {
            (Value::Quantity(a), Value::Quantity(b)) => {
                let b = openmhs_types::units::convert(b.value, &b.unit, &a.unit).ok()?;
                a.value.partial_cmp(&b)
            }
            (a, b) => match (codec::number(a), codec::number(b)) {
                (Some(x), Some(y)) if !matches!(a, Value::Bool(_)) => x.partial_cmp(&y),
                _ => (a == b).then_some(std::cmp::Ordering::Equal),
            },
        }
    };
    use std::cmp::Ordering::{Equal, Greater, Less};
    match (op, want) {
        (CompareOp::In, PreconditionValue::Many(vs)) => vs.iter().any(|w| cmp(w) == Some(Equal)),
        (_, PreconditionValue::Many(_)) | (CompareOp::In, _) => false,
        (op, PreconditionValue::One(w)) => {
            let c = cmp(w);
            match op {
                CompareOp::Eq => c == Some(Equal),
                CompareOp::Ne => c != Some(Equal),
                CompareOp::Lt => c == Some(Less),
                CompareOp::Le => matches!(c, Some(Less | Equal)),
                CompareOp::Gt => c == Some(Greater),
                CompareOp::Ge => matches!(c, Some(Greater | Equal)),
                CompareOp::In => false,
            }
        }
    }
}

#[async_trait]
impl Fixture for VirtualDevice {
    fn supports(&self, _fault: Fault) -> bool {
        true
    }

    async fn inject(&self, fault: Fault) -> Result<(), String> {
        let mut st = self.lock();
        match fault {
            Fault::LinkDown => st.link_down = true,
            other => st.fault = Some(other),
        }
        Ok(())
    }

    async fn clear(&self) -> Result<(), String> {
        let mut st = self.lock();
        st.link_down = false;
        st.fault = None;
        Ok(())
    }

    fn deliveries(&self) -> Option<u64> {
        Some(self.lock().deliveries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axis() -> Arc<VirtualDevice> {
        let json: serde_json::Value = serde_json::from_str(include_str!(
            "../../../spec/examples/valid/linear-actuator.json"
        ))
        .unwrap();
        VirtualDevice::new(
            openmhs_types::validate_descriptor(&json).unwrap(),
            &BTreeMap::new(),
        )
    }

    fn mm(v: f64) -> Value {
        Value::Quantity(Quantity {
            value: v,
            unit: "mm".into(),
        })
    }

    #[test]
    fn actions_check_preconditions_and_take_effect_through_their_readback() {
        let d = axis();
        let params: BTreeMap<String, Value> = [
            ("position".to_owned(), mm(10.0)),
            (
                "speed".to_owned(),
                Value::Quantity(Quantity {
                    value: 5.0,
                    unit: "mm/s".into(),
                }),
            ),
        ]
        .into();
        assert!(
            matches!(d.command("axis.move_absolute", None, &params), Outcome::Refused(r) if r.contains("homed"))
        );
        assert_eq!(
            d.command("axis.home", None, &BTreeMap::new()),
            Outcome::Applied
        );
        assert_eq!(d.read("axis.homed"), Some(Value::Bool(true)));
        assert_eq!(
            d.command("axis.move_absolute", None, &params),
            Outcome::Applied
        );
        assert_eq!(d.read("axis.position"), Some(mm(10.0)));
    }

    #[tokio::test]
    async fn faults_decide_the_outcome_and_deliveries_are_counted() {
        let d = axis();
        let none = BTreeMap::new();
        d.inject(Fault::NoResponse).await.unwrap();
        assert_eq!(d.command("axis.home", None, &none), Outcome::Silent);
        assert_eq!(
            d.read("axis.homed"),
            Some(Value::Bool(false)),
            "a lost command is not applied"
        );
        d.inject(Fault::LostAck).await.unwrap();
        assert_eq!(d.command("axis.home", None, &none), Outcome::Silent);
        assert_eq!(
            d.read("axis.homed"),
            Some(Value::Bool(true)),
            "a lost answer is"
        );
        d.inject(Fault::Garbled).await.unwrap();
        assert_eq!(d.command("axis.home", None, &none), Outcome::Garbled);
        d.inject(Fault::Refuse).await.unwrap();
        assert!(matches!(
            d.command("axis.home", None, &none),
            Outcome::Refused(_)
        ));
        assert_eq!(d.deliveries(), Some(4));
        d.clear().await.unwrap();
        assert!(!d.link_down());
    }
}
