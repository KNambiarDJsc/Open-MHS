//! Deliberately broken adapters fail with specific, actionable reasons (M5 acceptance).
//!
//! Each adapter below wraps a correct one and breaks exactly one rule. The tests check
//! that the report names the requirement, says what went wrong, and caps the level.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use openmhs_adapter::{
    AdapterCommand, AdapterError, AdapterErrorKind, ExecuteReport, HardwareAdapter, Health,
    Progress, TokioClock,
};
use openmhs_conformance::{
    ConformanceLevel, Fault, Fixture, Options, Report, Status, render, sim::SimFixture,
};
use openmhs_sim::{SimDevice, VirtualPsu};
use openmhs_types::schema::Schema;
use openmhs_types::{Clock, Quality, Quantity, Source, StateValue, Timestamp, Value};
use serde_json::json;

fn clock() -> Arc<dyn Clock> {
    Arc::new(TokioClock::starting_at(Timestamp::from_millis(
        1_790_000_000_000,
    )))
}

fn psu(clock: &Arc<dyn Clock>) -> SimDevice {
    let (adapter, control) = VirtualPsu::new(clock.clone(), 42);
    SimDevice {
        adapter: Arc::new(adapter),
        control: Arc::new(control),
    }
}

/// The checks with `id` that failed, as "capability: message".
fn failures<'a>(report: &'a Report, id: &str) -> Vec<&'a str> {
    report
        .checks
        .iter()
        .filter(|c| c.id == id && c.status == Status::Fail)
        .map(|c| c.message.as_str())
        .collect()
}

fn assert_fails(report: &Report, id: &str, phrase: &str) {
    let found = failures(report, id);
    assert!(
        found.iter().any(|m| m.contains(phrase)),
        "expected {id} to fail with '{phrase}', got {found:?}\n{}",
        render(report)
    );
    assert!(!report.passed, "{}", render(report));
    // Broken subjects still produce schema-valid reports.
    let json = serde_json::to_value(report).expect("a report serializes");
    let issues = openmhs_types::schema::validate(Schema::ConformanceReport, &json);
    assert!(issues.is_empty(), "{issues:?}");
    // The rendering a human sees names the requirement and the reason.
    let text = render(report);
    assert!(text.contains(id) && text.contains(phrase), "{text}");
}

// ------------------------------------------------------------- broken wrappers

#[derive(Clone, Copy, PartialEq, Eq)]
enum Flaw {
    /// Reports success whatever the device said, after a short internal timeout.
    SwallowsErrors,
    /// Serves the last good reading, fresh and GOOD, when the link is down.
    CachedReadings,
    /// Reports volts in millivolts.
    WrongUnit,
    /// Turns a device refusal into a device failure.
    RefusalAsFailure,
    /// Says it is healthy no matter what.
    AlwaysHealthy,
    /// Claims to be a different adapter than its descriptor names.
    WrongId,
    /// Declares a capability it never implemented.
    Unimplemented,
}

/// The capability `Flaw::Unimplemented` leaves out.
const MISSING: &str = "power.output.current.limit";

fn unsupported(capability: &str) -> AdapterError {
    AdapterError::new(
        AdapterErrorKind::Unsupported,
        format!("no capability '{capability}'"),
    )
}

struct Broken {
    inner: Arc<dyn HardwareAdapter>,
    flaw: Flaw,
    clock: Arc<dyn Clock>,
    cache: Mutex<HashMap<String, StateValue>>,
}

impl Broken {
    fn new(inner: Arc<dyn HardwareAdapter>, flaw: Flaw, clock: Arc<dyn Clock>) -> Self {
        Self {
            inner,
            flaw,
            clock,
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn cache(&self) -> MutexGuard<'_, HashMap<String, StateValue>> {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait]
impl HardwareAdapter for Broken {
    fn adapter_id(&self) -> &str {
        if self.flaw == Flaw::WrongId {
            "vendor.psu-driver"
        } else {
            self.inner.adapter_id()
        }
    }

    async fn describe(&self) -> Result<serde_json::Value, AdapterError> {
        self.inner.describe().await
    }

    async fn connect(&self) -> Result<(), AdapterError> {
        self.inner.connect().await
    }

    async fn health(&self) -> Health {
        if self.flaw == Flaw::AlwaysHealthy {
            Health::Healthy
        } else {
            self.inner.health().await
        }
    }

    async fn read(&self, capability: &str) -> Result<StateValue, AdapterError> {
        if self.flaw == Flaw::Unimplemented && capability == MISSING {
            return Err(unsupported(capability));
        }
        match (self.flaw, self.inner.read(capability).await) {
            (Flaw::CachedReadings, Ok(sv)) => {
                self.cache().insert(capability.to_owned(), sv.clone());
                Ok(sv)
            }
            (Flaw::CachedReadings, Err(e)) => match self.cache().get(capability) {
                Some(sv) => Ok(StateValue {
                    timestamp: self.clock.now(),
                    ..sv.clone()
                }),
                None => Err(e),
            },
            (Flaw::WrongUnit, Ok(mut sv)) => {
                if let Value::Quantity(q) = &mut sv.value
                    && q.unit == "V"
                {
                    *q = Quantity {
                        value: q.value * 1000.0,
                        unit: "mV".into(),
                    };
                }
                Ok(sv)
            }
            (_, r) => r,
        }
    }

    async fn execute(
        &self,
        command: AdapterCommand,
        progress: Progress,
    ) -> Result<ExecuteReport, AdapterError> {
        match self.flaw {
            Flaw::SwallowsErrors => {
                let r = tokio::time::timeout(
                    Duration::from_millis(200),
                    self.inner.execute(command, progress),
                )
                .await;
                match r {
                    Ok(Ok(report)) => Ok(report),
                    _ => Ok(ExecuteReport::Applied {
                        result: BTreeMap::new(),
                    }),
                }
            }
            Flaw::RefusalAsFailure => match self.inner.execute(command, progress).await? {
                ExecuteReport::Rejected { reason } => {
                    Err(AdapterError::new(AdapterErrorKind::DeviceFailure, reason))
                }
                other => Ok(other),
            },
            Flaw::Unimplemented if command.capability == MISSING => {
                Err(unsupported(&command.capability))
            }
            _ => self.inner.execute(command, progress).await,
        }
    }

    async fn shutdown(&self) -> Result<(), AdapterError> {
        self.inner.shutdown().await
    }
}

async fn run_broken(flaw: Flaw) -> Report {
    let clock = clock();
    let device = psu(&clock);
    let adapter = Arc::new(Broken::new(device.adapter.clone(), flaw, clock.clone()));
    openmhs_conformance::run(
        adapter,
        Arc::new(SimFixture::new(device.control.clone())),
        Options::new(clock),
    )
    .await
}

#[tokio::test(start_paused = true)]
async fn an_adapter_that_swallows_errors_fails_l2() {
    let r = run_broken(Flaw::SwallowsErrors).await;
    assert_fails(
        &r,
        "L2.malformed",
        "unparseable, but the adapter reported success",
    );
    assert_fails(
        &r,
        "L2.timeout",
        "no answer came, yet the adapter reported success",
    );
    assert_eq!(r.achieved, Some(ConformanceLevel::L1));
}

#[tokio::test(start_paused = true)]
async fn an_adapter_that_serves_cached_readings_when_disconnected_fails_l1() {
    let r = run_broken(Flaw::CachedReadings).await;
    assert_fails(&r, "L1.disconnect.read", "returned a GOOD reading");
    assert_eq!(
        failures(&r, "L1.disconnect.read").len(),
        6,
        "every readable property"
    );
    assert_eq!(r.achieved, Some(ConformanceLevel::L0));
}

#[tokio::test(start_paused = true)]
async fn an_adapter_that_reports_the_wrong_unit_fails_l1() {
    let r = run_broken(Flaw::WrongUnit).await;
    assert_fails(
        &r,
        "L1.read.value",
        "reported in 'mV'; readings must use the declared unit 'V'",
    );
    assert_eq!(r.achieved, Some(ConformanceLevel::L0));
}

#[tokio::test(start_paused = true)]
async fn an_adapter_that_turns_refusals_into_failures_fails_l2() {
    let r = run_broken(Flaw::RefusalAsFailure).await;
    assert_fails(
        &r,
        "L2.reject",
        "ended FAILED with adapter.device_failure; a refusal is returned as ExecuteReport::Rejected",
    );
    assert_eq!(r.achieved, Some(ConformanceLevel::L1));
}

#[tokio::test(start_paused = true)]
async fn an_adapter_that_is_always_healthy_fails_l1() {
    let r = run_broken(Flaw::AlwaysHealthy).await;
    assert_fails(
        &r,
        "L1.disconnect.health",
        "reports healthy while the link is down",
    );
}

#[tokio::test(start_paused = true)]
async fn an_adapter_with_the_wrong_id_fails_l0_and_stops() {
    let r = run_broken(Flaw::WrongId).await;
    assert_fails(
        &r,
        "L0.adapter_id",
        "returns 'vendor.psu-driver' but the descriptor names 'openmhs.sim.psu'",
    );
    assert_eq!(r.achieved, None);
    assert!(
        r.checks
            .iter()
            .all(|c| c.level == ConformanceLevel::L0 || c.level == ConformanceLevel::L3)
    );
}

/// One missing capability is reported once per root cause, at the capability, and does
/// not turn into misleading advice from the fault tests that depend on it.
#[tokio::test(start_paused = true)]
async fn an_unimplemented_capability_is_blamed_once_not_cascaded() {
    let r = run_broken(Flaw::Unimplemented).await;
    assert_fails(
        &r,
        "L1.read",
        "[adapter.unsupported] no capability 'power.output.current.limit'",
    );
    assert_fails(&r, "L2.command", "");
    let failed: Vec<(&str, Option<&str>)> = r
        .checks
        .iter()
        .filter(|c| c.status == Status::Fail)
        .map(|c| (c.id.as_str(), c.capability.as_deref()))
        .collect();
    assert_eq!(
        failed,
        [("L1.read", Some(MISSING)), ("L2.command", Some(MISSING))],
        "{}",
        render(&r)
    );
    for id in [
        "L2.reject",
        "L2.malformed",
        "L2.timeout",
        "L2.lost_ack",
        "L2.no_retry",
    ] {
        let c = r
            .checks
            .iter()
            .find(|c| c.id == id && c.capability.as_deref() == Some(MISSING))
            .unwrap();
        assert_eq!(c.status, Status::Untested, "{id}: {}", c.message);
        assert!(c.message.contains("see L2.command"), "{id}: {}", c.message);
    }
    // The outage checks still ran, on the properties that do work.
    let reconnect = r.checks.iter().find(|c| c.id == "L1.reconnect").unwrap();
    assert_eq!(reconnect.status, Status::Pass, "{}", reconnect.message);
    assert_eq!(r.achieved, Some(ConformanceLevel::L0));
}

#[tokio::test(start_paused = true)]
async fn without_a_fixture_nothing_above_l0_is_assumed() {
    let clock = clock();
    let device = psu(&clock);
    let r = openmhs_conformance::run(
        device.adapter.clone(),
        Arc::new(openmhs_conformance::NoFixture),
        Options::new(clock),
    )
    .await;
    assert_eq!(r.achieved, Some(ConformanceLevel::L0), "{}", render(&r));
    assert!(!r.passed);
    let untested: Vec<_> = r
        .checks
        .iter()
        .filter(|c| c.status == Status::Untested)
        .collect();
    assert!(
        untested
            .iter()
            .any(|c| c.id == "L1.disconnect.read" && c.message.contains("cannot inject link_down"))
    );
    assert!(
        untested
            .iter()
            .any(|c| c.id == "L2.timeout" && c.message.contains("cannot inject no_response"))
    );
}

#[tokio::test(start_paused = true)]
async fn hazardous_capabilities_are_untested_when_switched_off() {
    let clock = clock();
    let lab = openmhs_sim::virtual_lab(clock.clone(), 42);
    let axis = lab
        .iter()
        .find(|d| d.control.device_id() == "sim.axis-01")
        .unwrap();
    let mut options = Options::new(clock);
    options.hazardous = false;
    let r = openmhs_conformance::sim::run(axis, options).await;
    let skipped: Vec<_> = r
        .checks
        .iter()
        .filter(|c| c.capability.as_deref() == Some("axis.move_absolute"))
        .collect();
    assert_eq!(skipped.len(), 7);
    assert!(
        skipped
            .iter()
            .all(|c| c.status == Status::Untested && c.message.contains("hazardous"))
    );
    assert_eq!(r.achieved, Some(ConformanceLevel::L1));
}

// ---------------------------------------------------- a non-idempotent device

/// A counter whose `increment` is not idempotent: doing it twice counts twice. It is its
/// own fixture, and counts every request that reaches it.
struct Counter {
    state: Mutex<CounterState>,
    clock: Arc<dyn Clock>,
}

#[derive(Default)]
struct CounterState {
    value: i64,
    link_down: bool,
    fault: Option<Fault>,
    deliveries: u64,
}

impl Counter {
    fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            state: Mutex::new(CounterState::default()),
            clock,
        }
    }

    fn lock(&self) -> MutexGuard<'_, CounterState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn link(&self) -> Result<(), AdapterError> {
        if self.lock().link_down {
            Err(AdapterError::new(
                AdapterErrorKind::Disconnected,
                "link down",
            ))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl HardwareAdapter for Counter {
    fn adapter_id(&self) -> &str {
        "test.counter"
    }

    async fn describe(&self) -> Result<serde_json::Value, AdapterError> {
        Ok(json!({
            "openmhs": "0.1",
            "device": { "id": "test.counter-01", "manufacturer": "OpenMHS tests", "model": "COUNTER" },
            "adapter": { "id": "test.counter", "version": "0.1.0", "conformance": "L2" },
            "capabilities": [
                {
                    "name": "counter.value",
                    "kind": "property",
                    "operations": ["read"],
                    "value": { "type": "integer", "range": { "min": 0 } },
                    "side_effect": "none"
                },
                {
                    "name": "counter.increment",
                    "kind": "action",
                    "operations": ["invoke"],
                    "params": {},
                    "side_effect": "irreversible",
                    "idempotent": false,
                    "timing": { "timeout_ms": 1000 },
                    "verification": { "mode": "none", "reason": "an increment has no absolute value to read back" }
                }
            ]
        }))
    }

    async fn connect(&self) -> Result<(), AdapterError> {
        self.link()
    }

    async fn health(&self) -> Health {
        match self.link() {
            Ok(()) => Health::Healthy,
            Err(e) => Health::Disconnected(e.message),
        }
    }

    async fn read(&self, capability: &str) -> Result<StateValue, AdapterError> {
        self.link()?;
        if capability != "counter.value" {
            return Err(AdapterError::new(AdapterErrorKind::Unsupported, capability));
        }
        Ok(StateValue {
            value: Value::Integer(self.lock().value),
            timestamp: self.clock.now(),
            source: Source::Simulator,
            quality: Quality::Good,
        })
    }

    async fn execute(
        &self,
        _command: AdapterCommand,
        _progress: Progress,
    ) -> Result<ExecuteReport, AdapterError> {
        self.link()?;
        let fault = {
            let mut s = self.lock();
            s.deliveries += 1;
            if !matches!(s.fault, Some(Fault::NoResponse | Fault::Refuse)) {
                s.value += 1;
            }
            s.fault
        };
        match fault {
            Some(Fault::NoResponse | Fault::LostAck) => std::future::pending().await,
            Some(Fault::Garbled) => Err(AdapterError::new(
                AdapterErrorKind::MalformedResponse,
                "garbled answer",
            )),
            Some(Fault::Refuse) => Ok(ExecuteReport::Rejected {
                reason: "refused".into(),
            }),
            _ => Ok(ExecuteReport::Applied {
                result: BTreeMap::new(),
            }),
        }
    }

    async fn shutdown(&self) -> Result<(), AdapterError> {
        Ok(())
    }
}

#[async_trait]
impl Fixture for Counter {
    fn supports(&self, _fault: Fault) -> bool {
        true
    }

    async fn inject(&self, fault: Fault) -> Result<(), String> {
        let mut s = self.lock();
        if fault == Fault::LinkDown {
            s.link_down = true;
        } else {
            s.fault = Some(fault);
        }
        Ok(())
    }

    async fn clear(&self) -> Result<(), String> {
        let mut s = self.lock();
        s.link_down = false;
        s.fault = None;
        Ok(())
    }

    fn deliveries(&self) -> Option<u64> {
        Some(self.lock().deliveries)
    }
}

/// Re-sends a command that has not answered within 100 ms.
struct Retrier(Arc<Counter>);

#[async_trait]
impl HardwareAdapter for Retrier {
    fn adapter_id(&self) -> &str {
        self.0.adapter_id()
    }
    async fn describe(&self) -> Result<serde_json::Value, AdapterError> {
        self.0.describe().await
    }
    async fn connect(&self) -> Result<(), AdapterError> {
        self.0.connect().await
    }
    async fn health(&self) -> Health {
        self.0.health().await
    }
    async fn read(&self, capability: &str) -> Result<StateValue, AdapterError> {
        self.0.read(capability).await
    }
    async fn execute(
        &self,
        command: AdapterCommand,
        progress: Progress,
    ) -> Result<ExecuteReport, AdapterError> {
        for _ in 0..3 {
            let attempt = self.0.execute(command.clone(), progress.clone());
            if let Ok(r) = tokio::time::timeout(Duration::from_millis(100), attempt).await {
                return r;
            }
        }
        Err(AdapterError::new(
            AdapterErrorKind::Timeout,
            "no answer after 3 tries",
        ))
    }
    async fn shutdown(&self) -> Result<(), AdapterError> {
        self.0.shutdown().await
    }
}

#[tokio::test(start_paused = true)]
async fn a_correct_non_idempotent_adapter_reaches_l2() {
    let clock = clock();
    let counter = Arc::new(Counter::new(clock.clone()));
    let r = openmhs_conformance::run(counter.clone(), counter, Options::new(clock)).await;
    assert!(r.passed, "{}", render(&r));
    let no_retry = r.checks.iter().find(|c| c.id == "L2.no_retry").unwrap();
    assert_eq!(no_retry.status, Status::Pass, "{}", no_retry.message);
    assert!(
        no_retry.message.contains("no_response: 1"),
        "{}",
        no_retry.message
    );
}

#[tokio::test(start_paused = true)]
async fn an_adapter_that_retries_a_non_idempotent_command_fails_l2() {
    let clock = clock();
    let counter = Arc::new(Counter::new(clock.clone()));
    let r = openmhs_conformance::run(
        Arc::new(Retrier(counter.clone())),
        counter,
        Options::new(clock),
    )
    .await;
    assert_fails(
        &r,
        "L2.no_retry",
        "under no_response one command reached the device 3 times: the adapter retried a non-idempotent command",
    );
    assert_eq!(r.achieved, Some(ConformanceLevel::L1));
}
