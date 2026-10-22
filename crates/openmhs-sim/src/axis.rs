//! Simulated single-axis linear stage.
//!
//! Model: motion takes real (clock) time. Position is computed from the clock, so a move
//! keeps going even if the adapter call that started it is abandoned — as on a real
//! stage whose acknowledgement was lost. Absolute moves need homing first. The `jam`
//! device fault stops the carriage where it is and latches `axis.fault`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use openmhs_adapter::{
    AdapterCommand, AdapterError, AdapterErrorKind, ExecuteReport, HardwareAdapter, Health,
    Progress,
};
use openmhs_types::{Clock, Quantity, StateValue, Timestamp, Value};

use crate::fault::{Faults, SimControl, after_apply, before_apply};

const DESCRIPTOR: &str = include_str!("../../../spec/examples/valid/linear-actuator.json");
const ADAPTER_ID: &str = "openmhs.sim.axis";
const DEVICE_ID: &str = "sim.axis-01";
const BASE_LATENCY_MS: u64 = 5;
const HOMING_MS: u64 = 1_000;
const POLL_MS: u64 = 20;
/// Where the carriage happens to be at power-on (unknown to the controller until homed).
const POWER_ON_POSITION_MM: f64 = 137.4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionKind {
    Move,
    Home,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    pub kind: MotionKind,
    pub from_mm: f64,
    pub to_mm: f64,
    pub start: Timestamp,
    pub duration_ms: u64,
}

impl Motion {
    fn end(&self) -> Timestamp {
        self.start.plus_ms(self.duration_ms)
    }

    fn position_at(&self, now: Timestamp) -> f64 {
        if self.duration_ms == 0 {
            return self.to_mm;
        }
        #[allow(clippy::cast_precision_loss)]
        let frac = (self.start.ms_until(now) as f64 / self.duration_ms as f64).clamp(0.0, 1.0);
        self.from_mm + (self.to_mm - self.from_mm) * frac
    }
}

/// Ground truth inside the simulated stage.
#[derive(Debug, Clone, PartialEq)]
pub struct AxisTruth {
    pub position_mm: f64,
    pub homed: bool,
    pub fault: bool,
    pub motion: Option<Motion>,
    /// How many commands actually changed the device.
    pub applied_count: u64,
}

impl AxisTruth {
    /// Bring the stored state up to `now`, completing a finished motion.
    fn settle(&mut self, now: Timestamp) {
        if let Some(m) = self.motion
            && now >= m.end()
        {
            self.position_mm = m.to_mm;
            if m.kind == MotionKind::Home {
                self.homed = true;
            }
            self.motion = None;
        }
    }

    fn position(&self, now: Timestamp) -> f64 {
        self.motion.map_or(self.position_mm, |m| m.position_at(now))
    }

    /// Stop immediately at the current position.
    fn halt(&mut self, now: Timestamp) {
        self.position_mm = self.position(now);
        self.motion = None;
    }

    fn state_name(&self) -> &'static str {
        match (self.fault, self.motion.map(|m| m.kind)) {
            (true, _) => "fault",
            (false, Some(MotionKind::Home)) => "homing",
            (false, Some(MotionKind::Move)) => "moving",
            (false, None) => "idle",
        }
    }
}

struct Inner {
    truth: AxisTruth,
    faults: Faults,
}

#[derive(Clone)]
pub struct AxisHandle {
    inner: Arc<Mutex<Inner>>,
    clock: Arc<dyn Clock>,
    executes: Arc<AtomicU64>,
}

impl AxisHandle {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn truth(&self) -> AxisTruth {
        let mut inner = self.lock();
        inner.truth.settle(self.clock.now());
        inner.truth.clone()
    }

    /// Force the device into a state, bypassing everything (test setup only).
    pub fn set_truth(&self, f: impl FnOnce(&mut AxisTruth)) {
        f(&mut self.lock().truth);
    }

    pub fn execute_calls(&self) -> u64 {
        self.executes.load(Ordering::SeqCst)
    }
}

impl SimControl for AxisHandle {
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
        &["jam"]
    }

    fn inject_device_fault(&self, name: &str) -> Result<(), String> {
        match name {
            "jam" => {
                let now = self.clock.now();
                let mut inner = self.lock();
                inner.truth.settle(now);
                inner.truth.halt(now);
                inner.truth.fault = true;
                Ok(())
            }
            other => Err(format!("unknown fault '{other}'")),
        }
    }

    fn clear_device_faults(&self) {
        self.lock().truth.fault = false;
    }
}

pub struct VirtualAxis {
    handle: AxisHandle,
}

impl VirtualAxis {
    pub fn new(clock: Arc<dyn Clock>) -> (Self, AxisHandle) {
        let handle = AxisHandle {
            inner: Arc::new(Mutex::new(Inner {
                truth: AxisTruth {
                    position_mm: POWER_ON_POSITION_MM,
                    homed: false,
                    fault: false,
                    motion: None,
                    applied_count: 0,
                },
                faults: Faults::default(),
            })),
            clock,
            executes: Arc::new(AtomicU64::new(0)),
        };
        (
            Self {
                handle: handle.clone(),
            },
            handle,
        )
    }

    fn now(&self) -> Timestamp {
        self.handle.clock.now()
    }

    /// Start a motion on the device. `Ok(None)` means it started; `Ok(Some(report))` is
    /// the device refusing.
    fn start(&self, command: &AdapterCommand) -> Result<Option<ExecuteReport>, AdapterError> {
        let now = self.now();
        let mut inner = self.handle.lock();
        let t = &mut inner.truth;
        t.settle(now);
        let reject = |reason: &str| {
            Ok(Some(ExecuteReport::Rejected {
                reason: reason.into(),
            }))
        };
        match command.capability.as_str() {
            "axis.home" => {
                if t.fault {
                    return reject("axis is in fault");
                }
                t.motion = Some(Motion {
                    kind: MotionKind::Home,
                    from_mm: t.position(now),
                    to_mm: 0.0,
                    start: now,
                    duration_ms: HOMING_MS,
                });
            }
            "axis.move_absolute" => {
                if t.fault {
                    return reject("axis is in fault");
                }
                if !t.homed {
                    return reject("axis is not homed");
                }
                if t.motion.is_some() {
                    return reject("axis is already moving");
                }
                let target = mm(command.params.get("position"), "mm")?;
                let speed = mm(command.params.get("speed"), "mm/s")?;
                if speed <= 0.0 {
                    return Err(AdapterError::new(
                        AdapterErrorKind::NotSent,
                        "speed must be positive",
                    ));
                }
                let distance = (target - t.position_mm).abs();
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let duration_ms = (distance / speed * 1000.0).ceil() as u64;
                t.motion = Some(Motion {
                    kind: MotionKind::Move,
                    from_mm: t.position_mm,
                    to_mm: target,
                    start: now,
                    duration_ms,
                });
            }
            "axis.stop" => t.halt(now),
            other => {
                return Err(AdapterError::new(
                    AdapterErrorKind::Unsupported,
                    format!("cannot execute '{other}'"),
                ));
            }
        }
        Ok(None)
    }

    /// Wait until the current motion finishes, or fail if the axis faults on the way.
    async fn wait_for_motion(&self) -> Result<f64, AdapterError> {
        loop {
            let now = self.now();
            let (remaining, position) = {
                let mut inner = self.handle.lock();
                inner.truth.settle(now);
                let t = &inner.truth;
                if t.fault {
                    return Err(AdapterError::new(
                        AdapterErrorKind::DeviceFailure,
                        format!("axis faulted at {:.1} mm", t.position(now)),
                    ));
                }
                (t.motion.map(|m| now.ms_until(m.end())), t.position_mm)
            };
            match remaining {
                None => return Ok(position),
                Some(ms) => {
                    let wait = u64::try_from(ms.max(1)).unwrap_or(1).min(POLL_MS);
                    tokio::time::sleep(Duration::from_millis(wait)).await;
                }
            }
        }
    }
}

fn length(v: f64) -> Value {
    Value::Quantity(Quantity {
        value: v,
        unit: "mm".into(),
    })
}

fn mm(v: Option<&Value>, unit: &str) -> Result<f64, AdapterError> {
    match v {
        Some(Value::Quantity(q)) if q.unit == unit => Ok(q.value),
        other => Err(AdapterError::new(
            AdapterErrorKind::NotSent,
            format!("expected a quantity in {unit}, got {other:?}"),
        )),
    }
}

#[async_trait]
impl HardwareAdapter for VirtualAxis {
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
        let inner = self.handle.lock();
        match inner.faults.link() {
            Err(e) => Health::Disconnected(e.message),
            Ok(()) if inner.truth.fault => Health::Degraded("axis in fault".into()),
            Ok(()) => Health::Healthy,
        }
    }

    async fn read(&self, capability: &str) -> Result<StateValue, AdapterError> {
        let now = self.now();
        let mut inner = self.handle.lock();
        inner.faults.link()?;
        inner.truth.settle(now);
        let t = &inner.truth;
        let value = match capability {
            "axis.position" => length(t.position(now) + inner.faults.readback_offset),
            "axis.homed" => Value::Bool(t.homed),
            "axis.fault" => Value::Bool(t.fault),
            "axis.motion.state" => Value::Text(t.state_name().into()),
            other => {
                return Err(AdapterError::new(
                    AdapterErrorKind::Unsupported,
                    format!("no readable capability '{other}'"),
                ));
            }
        };
        Ok(inner.faults.reading(self.handle.clock.as_ref(), value))
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
        if let Some(report) = self.start(&command)? {
            return Ok(report);
        }
        progress.accepted();
        let final_position = self.wait_for_motion().await?;
        self.handle.lock().truth.applied_count += 1;

        let mut result = BTreeMap::new();
        if command.capability == "axis.move_absolute" {
            result.insert("final_position".to_owned(), length(final_position));
        }
        after_apply(fault, result).await
    }

    async fn shutdown(&self) -> Result<(), AdapterError> {
        let now = self.now();
        self.handle.lock().truth.halt(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmhs_adapter::TokioClock;
    use openmhs_types::descriptor::Operation;

    fn invoke(capability: &str, params: &[(&str, f64, &str)]) -> AdapterCommand {
        AdapterCommand {
            command_id: "c1".into(),
            capability: capability.into(),
            operation: Operation::Invoke,
            value: None,
            params: params
                .iter()
                .map(|(k, v, u)| {
                    (
                        (*k).to_owned(),
                        Value::Quantity(Quantity {
                            value: *v,
                            unit: (*u).into(),
                        }),
                    )
                })
                .collect(),
            idempotent: true,
            deadline: Timestamp::from_millis(i64::MAX),
        }
    }

    fn axis() -> (VirtualAxis, AxisHandle) {
        VirtualAxis::new(Arc::new(TokioClock::starting_at(Timestamp::from_millis(0))))
    }

    #[tokio::test(start_paused = true)]
    async fn descriptor_is_valid_and_matches_adapter() {
        let (a, _) = axis();
        let d = openmhs_types::validate_descriptor(&a.describe().await.unwrap()).unwrap();
        assert_eq!(d.adapter.id, a.adapter_id());
        assert_eq!(d.device.id, DEVICE_ID);
    }

    #[tokio::test(start_paused = true)]
    async fn move_requires_homing() {
        let (a, h) = axis();
        let (p, _rx) = Progress::channel();
        let r = a
            .execute(
                invoke(
                    "axis.move_absolute",
                    &[("position", 50.0, "mm"), ("speed", 10.0, "mm/s")],
                ),
                p,
            )
            .await
            .unwrap();
        assert!(matches!(r, ExecuteReport::Rejected { .. }));
        assert_eq!(h.truth().position_mm, POWER_ON_POSITION_MM);
    }

    #[tokio::test(start_paused = true)]
    async fn home_then_move_takes_time_and_reports_final_position() {
        let (a, h) = axis();
        let (p, _rx) = Progress::channel();
        a.execute(invoke("axis.home", &[]), p.clone())
            .await
            .unwrap();
        assert!(h.truth().homed);
        assert_eq!(h.truth().position_mm, 0.0);

        let start = tokio::time::Instant::now();
        let r = a
            .execute(
                invoke(
                    "axis.move_absolute",
                    &[("position", 100.0, "mm"), ("speed", 50.0, "mm/s")],
                ),
                p,
            )
            .await
            .unwrap();
        assert!(start.elapsed() >= Duration::from_millis(2_000));
        let ExecuteReport::Applied { result } = r else {
            panic!("expected applied")
        };
        assert_eq!(result.get("final_position"), Some(&length(100.0)));
        assert_eq!(h.truth().position_mm, 100.0);
    }

    #[tokio::test(start_paused = true)]
    async fn motion_continues_when_the_call_is_abandoned_and_reads_interpolate() {
        let (a, h) = axis();
        h.set_truth(|t| {
            t.homed = true;
            t.position_mm = 0.0;
        });
        let a = Arc::new(a);
        let mover = a.clone();
        let task = tokio::spawn(async move {
            let (p, _rx) = Progress::channel();
            mover
                .execute(
                    invoke(
                        "axis.move_absolute",
                        &[("position", 200.0, "mm"), ("speed", 50.0, "mm/s")],
                    ),
                    p,
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(2_000)).await;
        task.abort();
        // Halfway, less the command latency before motion started.
        let Value::Quantity(mid) = a.read("axis.position").await.unwrap().value else {
            panic!("expected a quantity")
        };
        assert!((99.0..=100.0).contains(&mid.value), "{}", mid.value);
        assert_eq!(
            a.read("axis.motion.state").await.unwrap().value,
            Value::Text("moving".into())
        );
        tokio::time::sleep(Duration::from_millis(2_500)).await;
        assert_eq!(a.read("axis.position").await.unwrap().value, length(200.0));
    }

    #[tokio::test(start_paused = true)]
    async fn jam_during_move_is_a_device_failure() {
        let (a, h) = axis();
        h.set_truth(|t| {
            t.homed = true;
            t.position_mm = 0.0;
        });
        let jammer = h.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            jammer.inject_device_fault("jam").unwrap();
        });
        let (p, _rx) = Progress::channel();
        let err = a
            .execute(
                invoke(
                    "axis.move_absolute",
                    &[("position", 100.0, "mm"), ("speed", 10.0, "mm/s")],
                ),
                p,
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind, AdapterErrorKind::DeviceFailure);
        let t = h.truth();
        assert!(t.fault && t.motion.is_none());
        assert!((t.position_mm - 5.0).abs() < 0.5, "{}", t.position_mm);
    }
}
