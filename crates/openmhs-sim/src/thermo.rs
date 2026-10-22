//! Simulated ambient temperature sensor (read-only device).
//!
//! Model: a slow daily-style swing around 22 °C plus measurement noise. The `overheat`
//! device fault adds a large offset, as a sensor next to a failing heater would see.

use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use openmhs_adapter::{
    AdapterCommand, AdapterError, AdapterErrorKind, ExecuteReport, HardwareAdapter, Health,
    Progress,
};
use openmhs_types::{Clock, Quantity, StateValue, Value};

use crate::fault::{Faults, SimControl};
use crate::rng::Rng;

const DESCRIPTOR: &str = include_str!("../../../spec/examples/valid/temperature-sensor.json");
const ADAPTER_ID: &str = "openmhs.sim.thermo";
const DEVICE_ID: &str = "sim.thermo-01";
const BASE_C: f64 = 22.0;
const SWING_C: f64 = 1.5;
const PERIOD_MS: f64 = 600_000.0;
const NOISE_C: f64 = 0.05;
const OVERHEAT_C: f64 = 60.0;

struct Inner {
    faults: Faults,
    overheat: bool,
    rng: Rng,
}

#[derive(Clone)]
pub struct ThermoHandle {
    inner: Arc<Mutex<Inner>>,
}

impl ThermoHandle {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl SimControl for ThermoHandle {
    fn device_id(&self) -> &str {
        DEVICE_ID
    }

    fn faults(&self) -> Faults {
        self.lock().faults.clone()
    }

    fn set_faults(&self, faults: Faults) {
        self.lock().faults = faults;
    }

    fn device_faults(&self) -> &'static [&'static str] {
        &["overheat"]
    }

    fn inject_device_fault(&self, name: &str) -> Result<(), String> {
        match name {
            "overheat" => {
                self.lock().overheat = true;
                Ok(())
            }
            other => Err(format!("unknown fault '{other}'")),
        }
    }

    fn clear_device_faults(&self) {
        self.lock().overheat = false;
    }
}

pub struct VirtualThermo {
    handle: ThermoHandle,
    clock: Arc<dyn Clock>,
}

impl VirtualThermo {
    pub fn new(clock: Arc<dyn Clock>, seed: u64) -> (Self, ThermoHandle) {
        let handle = ThermoHandle {
            inner: Arc::new(Mutex::new(Inner {
                faults: Faults::default(),
                overheat: false,
                rng: Rng::new(seed),
            })),
        };
        (
            Self {
                handle: handle.clone(),
                clock,
            },
            handle,
        )
    }
}

#[async_trait]
impl HardwareAdapter for VirtualThermo {
    fn adapter_id(&self) -> &str {
        ADAPTER_ID
    }

    async fn describe(&self) -> Result<serde_json::Value, AdapterError> {
        serde_json::from_str(DESCRIPTOR)
            .map_err(|e| AdapterError::new(AdapterErrorKind::Internal, e.to_string()))
    }

    async fn connect(&self) -> Result<(), AdapterError> {
        self.handle.lock().faults.link()
    }

    async fn health(&self) -> Health {
        match self.handle.lock().faults.link() {
            Ok(()) => Health::Healthy,
            Err(e) => Health::Disconnected(e.message),
        }
    }

    async fn read(&self, capability: &str) -> Result<StateValue, AdapterError> {
        let mut inner = self.handle.lock();
        inner.faults.link()?;
        let value = match capability {
            "temperature.ambient" => {
                #[allow(clippy::cast_precision_loss)]
                let t = self.clock.now().millis() as f64;
                let swing = SWING_C * (std::f64::consts::TAU * t / PERIOD_MS).sin();
                let noise = inner.rng.signed_unit() * NOISE_C;
                let overheat = if inner.overheat { OVERHEAT_C } else { 0.0 };
                Value::Quantity(Quantity {
                    value: BASE_C + swing + noise + overheat,
                    unit: "Cel".into(),
                })
            }
            "sensor.status" => Value::Text(
                if inner.faults.bad_quality {
                    "degraded"
                } else {
                    "ok"
                }
                .into(),
            ),
            other => {
                return Err(AdapterError::new(
                    AdapterErrorKind::Unsupported,
                    format!("no readable capability '{other}'"),
                ));
            }
        };
        Ok(inner.faults.reading(self.clock.as_ref(), value))
    }

    async fn execute(
        &self,
        command: AdapterCommand,
        _progress: Progress,
    ) -> Result<ExecuteReport, AdapterError> {
        Err(AdapterError::new(
            AdapterErrorKind::Unsupported,
            format!("'{}' is read-only", command.capability),
        ))
    }

    async fn shutdown(&self) -> Result<(), AdapterError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmhs_adapter::TokioClock;
    use openmhs_types::Timestamp;

    fn celsius(v: &StateValue) -> f64 {
        match &v.value {
            Value::Quantity(q) => q.value,
            other => panic!("expected a quantity, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn descriptor_is_valid_and_matches_adapter() {
        let (t, _) = VirtualThermo::new(Arc::new(TokioClock::new()), 1);
        let d = openmhs_types::validate_descriptor(&t.describe().await.unwrap()).unwrap();
        assert_eq!(d.adapter.id, t.adapter_id());
        assert_eq!(d.device.id, DEVICE_ID);
    }

    #[tokio::test(start_paused = true)]
    async fn reads_plausible_temperature_and_overheat_raises_it() {
        let clock = Arc::new(TokioClock::starting_at(Timestamp::from_millis(0)));
        let (t, h) = VirtualThermo::new(clock, 3);
        let normal = celsius(&t.read("temperature.ambient").await.unwrap());
        assert!((20.0..24.0).contains(&normal), "{normal}");
        h.inject_device_fault("overheat").unwrap();
        let hot = celsius(&t.read("temperature.ambient").await.unwrap());
        assert!(hot > 70.0, "{hot}");
        h.clear_device_faults();
        assert!(celsius(&t.read("temperature.ambient").await.unwrap()) < 24.0);
    }

    #[tokio::test(start_paused = true)]
    async fn is_read_only() {
        let (t, _) = VirtualThermo::new(Arc::new(TokioClock::new()), 1);
        let (p, _rx) = Progress::channel();
        let cmd = AdapterCommand {
            command_id: "c1".into(),
            capability: "temperature.ambient".into(),
            operation: openmhs_types::descriptor::Operation::Write,
            value: None,
            params: std::collections::BTreeMap::new(),
            idempotent: true,
            deadline: Timestamp::from_millis(i64::MAX),
        };
        let err = t.execute(cmd, p).await.unwrap_err();
        assert_eq!(err.kind, AdapterErrorKind::Unsupported);
    }
}
