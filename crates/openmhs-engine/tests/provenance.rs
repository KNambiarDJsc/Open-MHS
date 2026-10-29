//! What the audit trail records about where a command and a device contract came from.
//!
//! The chain proves the records it holds are in order and unaltered, so what is *in* a record
//! is what can be proved later. A device's own reference for the work (a LabThings invocation
//! id, a job number) and the hash of the capability contract in force are the two facts that
//! link a command here to the device's own records and to the exact description it was
//! checked against. Both belong in the chain, not only in the mutable receipt.

#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use common::*;
use openmhs_adapter::{
    AdapterCommand, AdapterError, ExecuteReport, HardwareAdapter, Health, Progress,
};
use openmhs_engine::Engine;
use openmhs_sim::VirtualPsu;
use openmhs_types::StateValue;
use serde_json::Value as Json;

/// A supply that reports a reference of its own when it accepts a command.
struct Referencing {
    inner: VirtualPsu,
    reference: Option<&'static str>,
}

#[async_trait]
impl HardwareAdapter for Referencing {
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
        self.inner.read(capability).await
    }
    async fn execute(
        &self,
        command: AdapterCommand,
        progress: Progress,
    ) -> Result<ExecuteReport, AdapterError> {
        match self.reference {
            Some(r) => progress.accepted_with_ref(r),
            None => progress.accepted(),
        }
        self.inner.execute(command, progress).await
    }
    async fn shutdown(&self) -> Result<(), AdapterError> {
        self.inner.shutdown().await
    }
}

async fn engine_with(reference: Option<&'static str>) -> Engine {
    let clock = clock();
    let (psu, _handle) = VirtualPsu::new(clock.clone(), 42);
    let engine = Engine::in_memory(clock, allow_all()).unwrap();
    engine
        .register(Arc::new(Referencing {
            inner: psu,
            reference,
        }))
        .await
        .unwrap();
    engine
}

fn audit(engine: &Engine, kind: &str) -> Vec<Json> {
    engine
        .journal()
        .audit_records()
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == kind)
        .map(|r| serde_json::from_str(&r.data).unwrap())
        .collect()
}

fn agent() -> openmhs_policy::Caller {
    openmhs_policy::Caller::in_process(AGENT)
}

#[tokio::test(start_paused = true)]
async fn the_devices_own_reference_is_in_the_hash_chained_audit_record() {
    let engine = engine_with(Some("labthings:7f3a")).await;
    let r = engine
        .submit(&agent(), &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(
        r.adapter_result.unwrap().external_ref.as_deref(),
        Some("labthings:7f3a")
    );
    let accepted: Vec<_> = audit(&engine, "command.transition")
        .into_iter()
        .filter(|d| d["to"] == "ACCEPTED")
        .collect();
    assert_eq!(accepted.len(), 1);
    assert_eq!(
        accepted[0]["detail"], "device reference labthings:7f3a",
        "the reference must be in the chained record, not only in the receipt"
    );
    assert_eq!(engine.journal().verify_audit().unwrap().first_invalid, None);
}

#[tokio::test(start_paused = true)]
async fn no_reference_means_no_detail_rather_than_an_invented_one() {
    let engine = engine_with(None).await;
    engine
        .submit(&agent(), &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    let accepted: Vec<_> = audit(&engine, "command.transition")
        .into_iter()
        .filter(|d| d["to"] == "ACCEPTED")
        .collect();
    assert_eq!(accepted.len(), 1);
    assert!(accepted[0]["detail"].is_null(), "{:?}", accepted[0]);
}

#[tokio::test(start_paused = true)]
async fn registration_records_a_stable_hash_of_the_contract() {
    let a = audit(&engine_with(None).await, "device.registered");
    let b = audit(&engine_with(Some("x")).await, "device.registered");
    let (ha, hb) = (
        a[0]["descriptor_sha256"].as_str().unwrap(),
        b[0]["descriptor_sha256"].as_str().unwrap(),
    );
    assert_eq!(ha.len(), 64, "a sha256 in hex");
    assert_eq!(ha, hb, "the same contract hashes the same, however it is served");
}
