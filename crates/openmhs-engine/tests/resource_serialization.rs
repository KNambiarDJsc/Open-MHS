//! State-changing commands serialize across devices that share a declared resource.

#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use common::{AGENT, OPERATOR, allow_all, clock, volts};
use openmhs_adapter::{
    AdapterCommand, AdapterError, ExecuteReport, HardwareAdapter, Health, Progress,
};
use openmhs_engine::Engine;
use openmhs_policy::Caller;
use openmhs_sim::VirtualPsu;
use openmhs_types::{CommandState, StateValue, Topology};
use serde_json::{Value, json};

#[derive(Default)]
struct Counters {
    running: AtomicUsize,
    peak: AtomicUsize,
}

struct CountingPsu {
    id: &'static str,
    inner: VirtualPsu,
    counters: Arc<Counters>,
}

#[async_trait]
impl HardwareAdapter for CountingPsu {
    fn adapter_id(&self) -> &str {
        self.inner.adapter_id()
    }

    async fn describe(&self) -> Result<Value, AdapterError> {
        let mut descriptor = self.inner.describe().await?;
        descriptor["device"]["id"] = self.id.into();
        Ok(descriptor)
    }

    async fn connect(&self) -> Result<(), AdapterError> {
        self.inner.connect().await
    }

    async fn health(&self) -> Health {
        self.inner.health().await
    }

    async fn read(&self, capability: &str) -> Result<StateValue, AdapterError> {
        self.inner.read(capability).await
    }

    async fn execute(
        &self,
        command: AdapterCommand,
        progress: Progress,
    ) -> Result<ExecuteReport, AdapterError> {
        let running = self.counters.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.counters.peak.fetch_max(running, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(30)).await;
        let result = self.inner.execute(command, progress).await;
        self.counters.running.fetch_sub(1, Ordering::SeqCst);
        result
    }

    async fn shutdown(&self) -> Result<(), AdapterError> {
        self.inner.shutdown().await
    }
}

async fn peak_concurrency(shared: bool) -> usize {
    const FIRST: &str = "sim.psu-a";
    const SECOND: &str = "sim.psu-b";
    const SETPOINT: &str = "power.output.voltage.setpoint";

    let clock = clock();
    let counters = Arc::new(Counters::default());
    let engine = Engine::in_memory(clock.clone(), allow_all()).unwrap();
    for id in [FIRST, SECOND] {
        let (inner, _) = VirtualPsu::new(clock.clone(), 42);
        engine
            .register(Arc::new(CountingPsu {
                id,
                inner,
                counters: counters.clone(),
            }))
            .await
            .unwrap();
    }
    if shared {
        engine
            .set_topology(
                Topology::parse(&format!(
                    r#"{{"openmhs_topology":"0.1","relationships":[
                        {{"from":"{FIRST}","kind":"shares_resource","resource":"bench.shared"}},
                        {{"from":"{SECOND}","kind":"shares_resource","resource":"bench.shared"}}
                    ]}}"#
                ))
                .unwrap(),
            )
            .unwrap();
    }

    let caller = Caller::in_process(AGENT);
    let request = |device: &str| {
        json!({
            "target": {"device": device, "capability": SETPOINT},
            "operation": "write",
            "value": volts(1.0)
        })
    };
    let first_request = request(FIRST);
    let second_request = request(SECOND);
    let (a, b) = tokio::join!(
        engine.submit(&caller, &first_request),
        engine.submit(&caller, &second_request),
    );
    assert_eq!(a.unwrap().state, CommandState::Verified);
    assert_eq!(b.unwrap().state, CommandState::Verified);
    counters.peak.load(Ordering::SeqCst)
}

#[tokio::test]
async fn shared_resources_serialize_across_devices() {
    assert_eq!(peak_concurrency(true).await, 1);
}

#[tokio::test]
async fn unrelated_resources_remain_concurrent() {
    assert_eq!(peak_concurrency(false).await, 2);
}

#[tokio::test]
async fn an_unknown_outcome_holds_the_shared_resource_until_resolved() {
    const FIRST: &str = "sim.psu-a";
    const SECOND: &str = "sim.psu-b";
    const SETPOINT: &str = "power.output.voltage.setpoint";

    let clock = clock();
    let counters = Arc::new(Counters::default());
    let engine = Engine::in_memory(clock.clone(), allow_all()).unwrap();
    let (first, _) = VirtualPsu::new(clock.clone(), 42);
    let (second, _) = VirtualPsu::new(clock.clone(), 43);
    engine
        .register(Arc::new(CountingPsu {
            id: FIRST,
            inner: first,
            counters: counters.clone(),
        }))
        .await
        .unwrap();
    engine
        .register(Arc::new(CountingPsu {
            id: SECOND,
            inner: second,
            counters,
        }))
        .await
        .unwrap();
    engine
        .set_topology(
            Topology::parse(&format!(
                r#"{{"openmhs_topology":"0.1","relationships":[
                    {{"from":"{FIRST}","kind":"shares_resource","resource":"bench.shared"}},
                    {{"from":"{SECOND}","kind":"shares_resource","resource":"bench.shared"}}
                ]}}"#
            ))
            .unwrap(),
        )
        .unwrap();

    let agent = Caller::in_process(AGENT);
    let slow_request = json!({
        "target": {"device": FIRST, "capability": SETPOINT},
        "operation": "write",
        "value": volts(1.0),
        "deadline_ms": 10
    });
    let uncertain = engine.submit(&agent, &slow_request).await.unwrap();
    assert_eq!(uncertain.state, CommandState::UnknownOutcome);

    let other_write = json!({
        "target": {"device": SECOND, "capability": SETPOINT},
        "operation": "write",
        "value": volts(1.0)
    });
    let refused = engine.submit(&agent, &other_write).await.unwrap();
    assert_eq!(refused.policy.errors[0].code, "command.outcome_unresolved");

    engine
        .resolve(
            &Caller::in_process(OPERATOR),
            &uncertain.command_id,
            false,
            "test confirms the delayed simulator command never reached apply",
        )
        .unwrap();
    let after_resolution = engine.submit(&agent, &other_write).await.unwrap();
    assert_eq!(after_resolution.state, CommandState::Verified);
}
