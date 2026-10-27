//! A reading taken before a command was sent cannot show that the command took effect.
//! Readback verification and reconciliation accept only readings measured at or after
//! dispatch (within an operator-set clock allowance). Otherwise a cached or retained value
//! could verify a command that did nothing, or, worse, reconcile one that did take effect
//! as `RECONCILED_NOT_APPLIED`, inviting the resend the lifecycle exists to prevent.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use common::*;
use openmhs_adapter::{
    AdapterCommand, AdapterError, AdapterErrorKind, ExecuteReport, HardwareAdapter, Health,
    Progress,
};
use openmhs_engine::{Engine, EngineError, EngineOptions, Journal};
use openmhs_sim::VirtualPsu;
use openmhs_types::{CommandState, StateValue};
use serde_json::Value as Json;

/// A supply whose setpoint reads come from a cache after a command: the value the setpoint
/// had when the command arrived, stamped `age_ms` before it. Like a retained MQTT message,
/// or a node that hung after it last published.
struct Cached {
    inner: VirtualPsu,
    /// Serve the cache instead of the device.
    serving: Arc<AtomicBool>,
    /// Report a timeout after the device applied the command.
    time_out: bool,
    age_ms: u64,
    cache: Mutex<Option<StateValue>>,
}

#[async_trait]
impl HardwareAdapter for Cached {
    fn adapter_id(&self) -> &str {
        self.inner.adapter_id()
    }
    async fn describe(&self) -> Result<Json, AdapterError> {
        self.inner.describe().await
    }
    async fn connect(&self) -> Result<(), AdapterError> {
        self.inner.connect().await
    }
    async fn health(&self) -> Health {
        self.inner.health().await
    }
    async fn read(&self, capability: &str) -> Result<StateValue, AdapterError> {
        if capability == SETPOINT && self.serving.load(Ordering::SeqCst) {
            let cached = self.cache.lock().unwrap().clone();
            if let Some(v) = cached {
                return Ok(v);
            }
        }
        self.inner.read(capability).await
    }
    async fn execute(
        &self,
        command: AdapterCommand,
        progress: Progress,
    ) -> Result<ExecuteReport, AdapterError> {
        let mut before = self.inner.read(SETPOINT).await?;
        before.timestamp = before.timestamp.minus_ms(self.age_ms);
        *self.cache.lock().unwrap() = Some(before);
        let applied = self.inner.execute(command, progress).await;
        self.serving.store(true, Ordering::SeqCst);
        if self.time_out {
            return Err(AdapterError::new(
                AdapterErrorKind::Timeout,
                "no answer after the device applied it",
            ));
        }
        applied
    }
    async fn shutdown(&self) -> Result<(), AdapterError> {
        self.inner.shutdown().await
    }
}

async fn setup_cached(time_out: bool, age_ms: u64, skew_ms: u64) -> (Engine, Arc<AtomicBool>) {
    let clock = clock();
    let (psu, _handle) = VirtualPsu::new(clock.clone(), 42);
    let serving = Arc::new(AtomicBool::new(false));
    let adapter = Cached {
        inner: psu,
        serving: serving.clone(),
        time_out,
        age_ms,
        cache: Mutex::new(None),
    };
    let engine = Engine::with_journal(
        Journal::open_in_memory().unwrap(),
        clock,
        allow_all(),
        EngineOptions {
            readback_skew_ms: skew_ms,
            ..EngineOptions::default()
        },
    );
    engine.register(Arc::new(adapter)).await.unwrap();
    (engine, serving)
}

fn agent() -> openmhs_policy::Caller {
    openmhs_policy::Caller::in_process(AGENT)
}

#[tokio::test(start_paused = true)]
async fn an_older_value_that_contradicts_the_command_does_not_fail_it() {
    // The supply applied 1 V; the cache still says 0 V from before. Reading that as the
    // result would call a working device faulty.
    let (engine, _) = setup_cached(false, 10_000, 0).await;
    let r = engine
        .submit(&agent(), &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(r.state, CommandState::Unverifiable, "{r:?}");
    let v = r.verification.unwrap();
    assert_eq!(v.matched, None);
    assert!(
        v.reason.unwrap().contains("older than the command"),
        "the receipt says why nothing was confirmed"
    );
}

#[tokio::test(start_paused = true)]
async fn an_older_value_that_happens_to_match_does_not_verify() {
    // Set 1 V for real, then ask for 1 V again. The cache matches, but it was measured
    // before this command, so it shows nothing about it.
    let (engine, serving) = setup_cached(false, 10_000, 0).await;
    let first = engine
        .submit(&agent(), &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(first.state, CommandState::Unverifiable);
    serving.store(false, Ordering::SeqCst);
    // The device answers for itself again: the same request now verifies.
    let again = engine
        .submit(&agent(), &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    // The second command's own cache is the 1 V from the first, stamped 10 s old.
    assert_eq!(again.state, CommandState::Unverifiable, "{again:?}");
}

#[tokio::test(start_paused = true)]
async fn reconciliation_does_not_conclude_from_a_reading_older_than_the_command() {
    // The device applied 1 V and the answer was lost. The only reading on offer is the
    // cached 0 V from before. Calling that RECONCILED_NOT_APPLIED would tell the agent
    // it may send again, and the device would apply it twice.
    let (engine, serving) = setup_cached(true, 10_000, 0).await;
    let r = engine
        .submit(&agent(), &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(r.state, CommandState::UnknownOutcome, "{r:?}");

    let refused = engine.reconcile(&r.command_id).await;
    assert!(
        matches!(&refused, Err(EngineError::NotReconcilable(m)) if m.contains("older than")),
        "{refused:?}"
    );
    assert_eq!(
        engine.receipt(&r.command_id).unwrap().unwrap().state,
        CommandState::UnknownOutcome
    );

    // Once the device answers for itself, a reading taken after dispatch settles it.
    serving.store(false, Ordering::SeqCst);
    let settled = engine.reconcile(&r.command_id).await.unwrap();
    assert_eq!(
        settled.state,
        CommandState::ReconciledApplied,
        "{settled:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_clock_allowance_admits_a_reading_slightly_older_than_dispatch() {
    // A device clock 500 ms behind the gateway's, with a 1 s allowance configured: the
    // reading is accepted and the command verifies. Without the allowance it would not.
    let (engine, _) = setup_cached(false, 500, 1_000).await;
    let r = engine
        .submit(&agent(), &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    // The cached value is still 0 V, so it is accepted as fresh and contradicts the
    // command: the allowance admits the reading, it does not make it match.
    assert_eq!(r.state, CommandState::VerificationFailed, "{r:?}");

    let (engine, _) = setup_cached(false, 500, 0).await;
    let r = engine
        .submit(&agent(), &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(r.state, CommandState::Unverifiable, "{r:?}");
}
