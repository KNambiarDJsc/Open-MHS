//! Simulated single-channel programmable power supply.
//!
//! Model: the output drives a fixed resistive load. When the output is enabled and the
//! load current would exceed the current limit, protection trips and disables the
//! output, as a real supply's over-current protection would.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use openmhs_adapter::{
    AdapterCommand, AdapterError, AdapterErrorKind, ExecuteReport, HardwareAdapter, Health,
    Progress,
};
use openmhs_types::{Clock, Quantity, StateValue, Value};

use crate::fault::{Faults, SimControl, after_apply, before_apply};
use crate::rng::Rng;

const DESCRIPTOR: &str = include_str!("../../../spec/examples/valid/power-supply.json");
const ADAPTER_ID: &str = "openmhs.sim.psu";
const DEVICE_ID: &str = "sim.psu-01";
const LOAD_OHMS: f64 = 10.0;
const NOISE_V: f64 = 0.002;
const BASE_LATENCY_MS: u64 = 5;

/// Ground truth inside the simulated device, for tests to compare against.
#[derive(Debug, Clone, PartialEq)]
pub struct PsuTruth {
    pub setpoint_v: f64,
    pub enabled: bool,
    pub current_limit_a: f64,
    pub tripped: bool,
    /// How many times a command actually changed the device.
    pub applied_count: u64,
}

struct Inner {
    truth: PsuTruth,
    faults: Faults,
    rng: Rng,
}

/// Shared handle for tests and demos: inject faults and inspect ground truth.
#[derive(Clone)]
pub struct PsuHandle {
    inner: Arc<Mutex<Inner>>,
    executes: Arc<AtomicU64>,
    reads: Arc<AtomicU64>,
}

impl PsuHandle {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A poisoned lock only means a test thread panicked mid-update (e.g. the Crash
        // fault); the simulated state is still usable.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_faults(&self, faults: Faults) {
        self.lock().faults = faults;
    }

    pub fn update_faults(&self, f: impl FnOnce(&mut Faults)) {
        f(&mut self.lock().faults);
    }

    pub fn clear_faults(&self) {
        self.lock().faults = Faults::default();
    }

    pub fn truth(&self) -> PsuTruth {
        self.lock().truth.clone()
    }

    /// Force the device into a state, bypassing everything (test setup only).
    pub fn set_truth(&self, f: impl FnOnce(&mut PsuTruth)) {
        f(&mut self.lock().truth);
    }

    /// Calls to `execute`, including ones that failed.
    pub fn execute_calls(&self) -> u64 {
        self.executes.load(Ordering::SeqCst)
    }

    /// Calls to `read`.
    pub fn read_calls(&self) -> u64 {
        self.reads.load(Ordering::SeqCst)
    }
}

pub struct VirtualPsu {
    handle: PsuHandle,
    clock: Arc<dyn Clock>,
}

impl VirtualPsu {
    pub fn new(clock: Arc<dyn Clock>, seed: u64) -> (Self, PsuHandle) {
        let handle = PsuHandle {
            inner: Arc::new(Mutex::new(Inner {
                truth: PsuTruth {
                    setpoint_v: 0.0,
                    enabled: false,
                    current_limit_a: 1.0,
                    tripped: false,
                    applied_count: 0,
                },
                faults: Faults::default(),
                rng: Rng::new(seed),
            })),
            executes: Arc::new(AtomicU64::new(0)),
            reads: Arc::new(AtomicU64::new(0)),
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

impl SimControl for PsuHandle {
    fn device_id(&self) -> &str {
        DEVICE_ID
    }

    fn faults(&self) -> Faults {
        self.lock().faults.clone()
    }

    fn set_faults(&self, faults: Faults) {
        PsuHandle::set_faults(self, faults);
    }

    fn device_faults(&self) -> &'static [&'static str] {
        &["trip"]
    }

    fn inject_device_fault(&self, name: &str) -> Result<(), String> {
        match name {
            "trip" => {
                let mut inner = self.lock();
                inner.truth.tripped = true;
                inner.truth.enabled = false;
                Ok(())
            }
            other => Err(format!("unknown fault '{other}'")),
        }
    }
}

fn volts(v: f64) -> Value {
    Value::Quantity(Quantity {
        value: v,
        unit: "V".into(),
    })
}

fn amps(a: f64) -> Value {
    Value::Quantity(Quantity {
        value: a,
        unit: "A".into(),
    })
}

fn number(v: Option<&Value>) -> Result<f64, AdapterError> {
    match v {
        Some(Value::Quantity(q)) => Ok(q.value),
        _ => Err(AdapterError::new(
            AdapterErrorKind::NotSent,
            "expected a quantity",
        )),
    }
}

fn boolean(v: Option<&Value>) -> Result<bool, AdapterError> {
    match v {
        Some(Value::Bool(b)) => Ok(*b),
        _ => Err(AdapterError::new(
            AdapterErrorKind::NotSent,
            "expected a boolean",
        )),
    }
}

/// Apply over-current protection.
fn protect(t: &mut PsuTruth) {
    if t.enabled && t.setpoint_v / LOAD_OHMS > t.current_limit_a {
        t.tripped = true;
        t.enabled = false;
    }
}

#[async_trait]
impl HardwareAdapter for VirtualPsu {
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
        if self.handle.lock().faults.disconnected {
            Health::Disconnected("simulated disconnect".into())
        } else {
            Health::Healthy
        }
    }

    async fn read(&self, capability: &str) -> Result<StateValue, AdapterError> {
        self.handle.reads.fetch_add(1, Ordering::SeqCst);
        let mut inner = self.handle.lock();
        inner.faults.link()?;
        let t = inner.truth.clone();
        let live = t.enabled && !t.tripped;
        let value = match capability {
            "power.output.voltage.setpoint" => volts(t.setpoint_v + inner.faults.readback_offset),
            "power.output.voltage.measured" => {
                let noise = inner.rng.signed_unit() * NOISE_V;
                volts(if live { t.setpoint_v + noise } else { 0.0 })
            }
            "power.output.current.limit" => amps(t.current_limit_a),
            "power.output.current.measured" => {
                amps(if live { t.setpoint_v / LOAD_OHMS } else { 0.0 })
            }
            "power.output.enabled" => Value::Bool(t.enabled),
            "protection.tripped" => Value::Bool(t.tripped),
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
        progress: Progress,
    ) -> Result<ExecuteReport, AdapterError> {
        self.handle.executes.fetch_add(1, Ordering::SeqCst);
        let (fault, latency) = {
            let inner = self.handle.lock();
            inner.faults.link()?;
            (
                inner.faults.execute,
                BASE_LATENCY_MS + inner.faults.execute_latency_ms,
            )
        };
        if let Some(report) = before_apply(fault, latency).await? {
            return Ok(report);
        }

        progress.accepted();
        {
            let mut inner = self.handle.lock();
            let t = &mut inner.truth;
            match command.capability.as_str() {
                "power.output.voltage.setpoint" => t.setpoint_v = number(command.value.as_ref())?,
                "power.output.current.limit" => t.current_limit_a = number(command.value.as_ref())?,
                "power.output.enabled" => {
                    let on = boolean(command.value.as_ref())?;
                    if on && t.tripped {
                        return Ok(ExecuteReport::Rejected {
                            reason: "protection tripped".into(),
                        });
                    }
                    t.enabled = on;
                }
                "protection.reset" => {
                    t.tripped = false;
                    t.enabled = false;
                }
                other => {
                    return Err(AdapterError::new(
                        AdapterErrorKind::Unsupported,
                        format!("cannot execute '{other}'"),
                    ));
                }
            }
            protect(t);
            t.applied_count += 1;
        }

        after_apply(fault, BTreeMap::new()).await
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
    use openmhs_types::descriptor::Operation;

    fn cmd(capability: &str, value: Value) -> AdapterCommand {
        AdapterCommand {
            command_id: "c1".into(),
            capability: capability.into(),
            operation: Operation::Write,
            value: Some(value),
            params: BTreeMap::new(),
            idempotent: true,
            deadline: Timestamp::from_millis(i64::MAX),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn descriptor_is_valid_and_matches_adapter() {
        let (psu, _) = VirtualPsu::new(Arc::new(TokioClock::new()), 1);
        let d = openmhs_types::validate_descriptor(&psu.describe().await.unwrap()).unwrap();
        assert_eq!(d.adapter.id, psu.adapter_id());
    }

    #[tokio::test(start_paused = true)]
    async fn overcurrent_trips_and_disables_output() {
        let (psu, h) = VirtualPsu::new(Arc::new(TokioClock::new()), 1);
        let (p, _rx) = Progress::channel();
        psu.execute(cmd("power.output.voltage.setpoint", volts(12.0)), p.clone())
            .await
            .unwrap();
        // 12 V into 10 Ohm = 1.2 A > 1.0 A limit.
        psu.execute(cmd("power.output.enabled", Value::Bool(true)), p)
            .await
            .unwrap();
        let t = h.truth();
        assert!(t.tripped && !t.enabled);
        assert_eq!(
            psu.read("power.output.voltage.measured")
                .await
                .unwrap()
                .value,
            volts(0.0)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn same_seed_same_noise() {
        let clock: Arc<dyn Clock> = Arc::new(TokioClock::starting_at(Timestamp::from_millis(0)));
        let run = |seed| {
            let clock = clock.clone();
            async move {
                let (psu, h) = VirtualPsu::new(clock, seed);
                h.set_truth(|t| {
                    t.setpoint_v = 5.0;
                    t.enabled = true;
                });
                let mut out = Vec::new();
                for _ in 0..5 {
                    out.push(
                        psu.read("power.output.voltage.measured")
                            .await
                            .unwrap()
                            .value,
                    );
                }
                out
            }
        };
        assert_eq!(run(7).await, run(7).await);
        assert_ne!(run(7).await, run(8).await);
    }
}
