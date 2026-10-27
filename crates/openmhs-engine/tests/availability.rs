//! Device availability (ADR-0019): a device that does not answer is registered OFFLINE
//! instead of stopping the gateway, refused before anything is sent, and used again once
//! it answers; stops are never refused for it; status shows busy devices and leases.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use openmhs_engine::{Engine, LeaseRequest};
use openmhs_policy::{Caller, Effect, Grant, Policy};
use openmhs_sim::{Faults, PsuHandle, SimControl, VirtualAxis, VirtualPsu};
use openmhs_types::descriptor::Operation;
use openmhs_types::{Availability, CommandState};
use serde_json::json;

fn link_down() -> Faults {
    Faults {
        disconnected: true,
        ..Faults::default()
    }
}

/// An engine with the simulated supply registered while it is switched off.
async fn offline_psu() -> (Engine, PsuHandle) {
    let clock = clock();
    let (psu, handle) = VirtualPsu::new(clock.clone(), 42);
    handle.set_faults(link_down());
    let engine = Engine::in_memory(clock, allow_all()).unwrap();
    engine.register_allow_offline(Arc::new(psu)).await.unwrap();
    (engine, handle)
}

fn agent() -> Caller {
    Caller::in_process(AGENT)
}

fn availability(engine: &Engine) -> Availability {
    engine.device_status(PSU).unwrap().unwrap().availability
}

fn audit_kinds(engine: &Engine, kind: &str) -> Vec<String> {
    engine
        .journal()
        .audit_records()
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == kind)
        .map(|r| r.data)
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_device_that_does_not_answer_is_registered_offline() {
    let (engine, _psu) = offline_psu().await;
    let status = engine.device_status(PSU).unwrap().unwrap();
    assert_eq!(status.availability, Availability::Offline);
    assert_eq!(status.reason.as_deref(), Some("simulated disconnect"));
    assert!(!status.busy);
    let registered = audit_kinds(&engine, "device.registered");
    assert!(registered[0].contains("\"OFFLINE\""), "{registered:?}");

    // The strict path still refuses it, for embedders who want that.
    let (psu, handle) = VirtualPsu::new(clock(), 42);
    handle.set_faults(link_down());
    let strict = Engine::in_memory(clock(), allow_all()).unwrap();
    assert!(strict.register(Arc::new(psu)).await.is_err());
    assert!(strict.devices().is_empty());
}

#[tokio::test(start_paused = true)]
async fn commands_to_an_offline_device_are_refused_before_anything_is_sent() {
    let (engine, psu) = offline_psu().await;
    let request = write(SETPOINT, volts(1.0));

    let checked = engine.preflight(&agent(), &request).await.unwrap();
    assert_eq!(checked.errors[0].code, "device.unavailable");

    let r = engine.submit(&agent(), &request).await.unwrap();
    assert_eq!(r.state, CommandState::Denied, "{r:?}");
    assert_eq!(codes(&r), ["device.unavailable"]);
    assert!(r.policy.errors[0].message.contains("nothing was sent"));
    assert_schema_valid(&r);
    assert_eq!(psu.execute_calls(), 0, "nothing reached the device");
    // No UNKNOWN_OUTCOME was left behind to hold the next command.
    assert!(
        engine
            .commands(None, &[CommandState::UnknownOutcome], 10)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(start_paused = true)]
async fn a_device_that_comes_back_is_used_without_a_restart() {
    let (engine, psu) = offline_psu().await;
    psu.clear_faults();

    // Still recorded OFFLINE: the command probes once more instead of refusing on old news.
    assert_eq!(availability(&engine), Availability::Offline);
    let r = engine
        .submit(&agent(), &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(r.state, CommandState::Verified, "{r:?}");
    assert_eq!(availability(&engine), Availability::Online);

    let changes = audit_kinds(&engine, "device.availability");
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert!(
        changes[0].contains("\"from\":\"OFFLINE\"") && changes[0].contains("\"to\":\"ONLINE\"")
    );
}

#[tokio::test(start_paused = true)]
async fn a_disconnect_seen_in_use_marks_the_device_offline() {
    let (engine, psu) = setup(allow_all()).await;
    assert_eq!(availability(&engine), Availability::Online);

    psu.set_faults(link_down());
    let read = engine
        .read(&agent(), PSU, "power.output.voltage.measured")
        .await;
    assert!(read.is_err());
    assert_eq!(availability(&engine), Availability::Offline);

    let before = psu.execute_calls();
    let r = engine
        .submit(&agent(), &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(codes(&r), ["device.unavailable"]);
    assert_eq!(psu.execute_calls(), before);
}

#[tokio::test(start_paused = true)]
async fn refresh_finds_changes_and_audits_each_once() {
    let (engine, psu) = setup(allow_all()).await;

    psu.set_faults(link_down());
    assert_eq!(engine.refresh_availability(0).await.unwrap(), 1);
    assert_eq!(availability(&engine), Availability::Offline);
    engine.refresh_availability(0).await.unwrap();
    assert_eq!(audit_kinds(&engine, "device.availability").len(), 1);

    // Checked a moment ago: not probed again.
    assert_eq!(engine.refresh_availability(60_000).await.unwrap(), 0);

    psu.clear_faults();
    tokio::time::sleep(Duration::from_millis(10)).await;
    engine.refresh_availability(0).await.unwrap();
    assert_eq!(availability(&engine), Availability::Online);
    assert_eq!(audit_kinds(&engine, "device.availability").len(), 2);
    assert!(engine.verify_audit().unwrap().is_valid());
}

#[tokio::test(start_paused = true)]
async fn a_stop_is_never_refused_for_availability() {
    let clock = clock();
    let (axis, handle) = VirtualAxis::new(clock.clone());
    handle.set_faults(link_down());
    let policy = Policy {
        grants: vec![Grant {
            id: "agent.axis".into(),
            principal: AGENT.into(),
            device: "sim.axis-01".into(),
            capability: "*".into(),
            operations: vec![Operation::Read, Operation::Invoke],
            effect: Effect::Allow,
            max_side_effect: None,
            reason: None,
        }],
        approvers: vec![OPERATOR.into()],
    };
    let engine = Engine::in_memory(clock, policy).unwrap();
    engine.register_allow_offline(Arc::new(axis)).await.unwrap();
    let invoke = |capability: &str| {
        json!({
            "target": { "device": "sim.axis-01", "capability": capability },
            "operation": "invoke",
            "params": {}
        })
    };

    let home = engine.submit(&agent(), &invoke("axis.home")).await.unwrap();
    assert_eq!(codes(&home), ["device.unavailable"]);
    assert_eq!(handle.execute_calls(), 0);

    // The stop is sent although the device is marked OFFLINE. It fails on the dead link,
    // as it would without any status: availability never stands in a stop's way.
    let stop = engine.submit(&agent(), &invoke("axis.stop")).await.unwrap();
    assert_ne!(stop.state, CommandState::Denied, "{stop:?}");
    assert_eq!(handle.execute_calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn status_shows_a_running_command_and_the_lease_on_a_device() {
    let (engine, psu) = setup(allow_all()).await;
    let engine = Arc::new(engine);
    psu.set_faults(Faults {
        execute_latency_ms: 2_000,
        ..Faults::default()
    });

    let running = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .submit(&agent(), &write(SETPOINT, volts(1.0)))
                .await
                .unwrap()
        })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(engine.device_status(PSU).unwrap().unwrap().busy);
    // A probe stays out of a running command's way.
    assert_eq!(engine.refresh_availability(0).await.unwrap(), 0);
    let done = running.await.unwrap();
    assert_eq!(done.state, CommandState::Verified, "{done:?}");
    assert!(!engine.device_status(PSU).unwrap().unwrap().busy);

    let lease = engine
        .acquire_lease(
            &agent(),
            LeaseRequest {
                devices: vec![PSU.into()],
                resources: vec![],
                ttl_ms: 60_000,
                reason: None,
                plan_id: None,
            },
        )
        .unwrap();
    let held = engine.device_status(PSU).unwrap().unwrap().lease.unwrap();
    assert_eq!((held.id, held.holder), (lease.id, AGENT.to_owned()));
    assert_eq!(engine.device_statuses().unwrap().len(), 1);
}

// ------------------------------------------------------------------ fleet scale

mod fleet {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use async_trait::async_trait;
    use openmhs_adapter::{
        AdapterCommand, AdapterError, AdapterErrorKind, ExecuteReport, HardwareAdapter, Health,
        Progress,
    };
    use openmhs_types::StateValue;
    use serde_json::Value;

    use super::*;

    /// What a fleet of stub devices shares: how many probes run at once, and in total.
    #[derive(Default)]
    pub struct Counters {
        pub running: AtomicUsize,
        pub peak: AtomicUsize,
        pub calls: AtomicUsize,
    }

    /// A device that answers `connect` and `health` after `delay_ms`, or never (`hang`), or
    /// refuses (`up` false).
    pub struct Stub {
        pub id: String,
        pub up: Arc<AtomicBool>,
        pub delay_ms: u64,
        pub hang: bool,
        pub counters: Arc<Counters>,
    }

    impl Stub {
        async fn answer(&self) -> Result<(), AdapterError> {
            self.counters.calls.fetch_add(1, Ordering::SeqCst);
            let now = self.counters.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.counters.peak.fetch_max(now, Ordering::SeqCst);
            if self.hang {
                tokio::time::sleep(Duration::from_secs(3_600)).await;
            }
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            self.counters.running.fetch_sub(1, Ordering::SeqCst);
            if self.up.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(AdapterError::new(
                    AdapterErrorKind::Disconnected,
                    "connection refused",
                ))
            }
        }
    }

    #[async_trait]
    impl HardwareAdapter for Stub {
        fn adapter_id(&self) -> &str {
            "openmhs.sim.psu"
        }
        async fn describe(&self) -> Result<Value, AdapterError> {
            let mut d: Value = serde_json::from_str(include_str!(
                "../../../spec/examples/valid/power-supply.json"
            ))
            .unwrap();
            d["device"]["id"] = self.id.clone().into();
            Ok(d)
        }
        async fn connect(&self) -> Result<(), AdapterError> {
            self.answer().await
        }
        async fn health(&self) -> Health {
            match self.answer().await {
                Ok(()) => Health::Healthy,
                Err(e) => Health::Disconnected(e.message),
            }
        }
        async fn read(&self, _capability: &str) -> Result<StateValue, AdapterError> {
            Err(AdapterError::new(AdapterErrorKind::Disconnected, "stub"))
        }
        async fn execute(
            &self,
            _command: AdapterCommand,
            _progress: Progress,
        ) -> Result<ExecuteReport, AdapterError> {
            panic!("stubs are never commanded");
        }
        async fn shutdown(&self) -> Result<(), AdapterError> {
            Ok(())
        }
    }
}

/// 1,000 devices, all switched off at start: registration does not block on any of them
/// (each is registered `OFFLINE`), and when they come up one sweep reaches all of them with bounded concurrency, in about
/// ceil(1000 / limit) probe times instead of 1000. Time is virtual, so this measures the
/// schedule, not the machine.
#[tokio::test(start_paused = true)]
async fn a_thousand_offline_devices_start_and_one_sweep_brings_them_all_online() {
    const N: usize = 1_000;
    const PROBE_MS: u64 = 100;
    let counters = Arc::new(fleet::Counters::default());
    let up = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let engine = Engine::in_memory(clock(), allow_all()).unwrap();

    let started = tokio::time::Instant::now();
    for i in 0..N {
        let stub = fleet::Stub {
            id: format!("fleet.psu-{i:04}"),
            up: up.clone(),
            delay_ms: 0,
            hang: false,
            counters: counters.clone(),
        };
        engine.register_allow_offline(Arc::new(stub)).await.unwrap();
    }
    assert_eq!(engine.devices().len(), N);
    assert!(
        engine
            .device_statuses()
            .unwrap()
            .iter()
            .all(|s| s.availability == Availability::Offline)
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "no waiting on any of them"
    );

    // The same fleet again, this time answering slowly once switched on, so that the sweep
    // has probe time to schedule.
    let engine = Engine::in_memory(clock(), allow_all()).unwrap();
    for i in 0..N {
        let stub = fleet::Stub {
            id: format!("fleet.psu-{i:04}"),
            up: up.clone(),
            delay_ms: PROBE_MS,
            hang: false,
            counters: counters.clone(),
        };
        engine.register_allow_offline(Arc::new(stub)).await.unwrap();
    }
    up.store(true, std::sync::atomic::Ordering::SeqCst);
    counters.peak.store(0, std::sync::atomic::Ordering::SeqCst);

    let sweep = tokio::time::Instant::now();
    assert_eq!(engine.refresh_availability(0).await.unwrap(), N);
    let took = sweep.elapsed();

    let peak = counters.peak.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        peak <= openmhs_engine::MAX_CONCURRENT_PROBES,
        "{peak} probes at once"
    );
    let batches = N.div_ceil(openmhs_engine::MAX_CONCURRENT_PROBES) as u32;
    assert!(
        took <= Duration::from_millis(PROBE_MS) * (batches + 1),
        "a sweep of {N} took {took:?}, expected about {batches} x {PROBE_MS} ms"
    );
    assert!(
        engine
            .device_statuses()
            .unwrap()
            .iter()
            .all(|s| s.availability == Availability::Online)
    );
    // One availability record per device came online; nothing else was written per probe.
    assert_eq!(audit_kinds(&engine, "device.availability").len(), N);
}

/// A device whose adapter never answers is registered `OFFLINE` after one probe timeout,
/// and is not probed again while that call is outstanding, however often a sweep runs.
#[tokio::test(start_paused = true)]
async fn a_hung_device_has_at_most_one_outstanding_probe() {
    let counters = Arc::new(fleet::Counters::default());
    let engine = Engine::in_memory(clock(), allow_all()).unwrap();
    let hung = fleet::Stub {
        id: "fleet.hung".into(),
        up: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        delay_ms: 0,
        hang: true,
        counters: counters.clone(),
    };

    let started = tokio::time::Instant::now();
    engine.register_allow_offline(Arc::new(hung)).await.unwrap();
    assert!(
        started.elapsed() <= Duration::from_millis(2_100),
        "bounded by the probe timeout"
    );
    let status = engine.device_status("fleet.hung").unwrap().unwrap();
    assert_eq!(status.availability, Availability::Offline);
    assert!(status.reason.unwrap().contains("no answer"));

    for _ in 0..5 {
        engine.refresh_availability(0).await.unwrap();
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    assert_eq!(
        counters.calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the abandoned call still holds the device's only probe slot"
    );
}
