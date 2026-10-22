//! Shared test support. Not every test binary uses every helper.
#![allow(dead_code, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use openmhs_adapter::{
    AdapterCommand, AdapterError, AdapterErrorKind, ExecuteReport, HardwareAdapter, Health,
    Progress, TokioClock,
};
use openmhs_engine::Engine;
use openmhs_policy::{Effect, Grant, Policy};
use openmhs_sim::{PsuHandle, VirtualPsu};
use openmhs_types::descriptor::Operation;
use openmhs_types::schema::{self, Schema};
use openmhs_types::{Clock, CommandState, Receipt, StateValue, Timestamp};
use serde_json::{Value as Json, json};

pub const PSU: &str = "sim.psu-01";
pub const SETPOINT: &str = "power.output.voltage.setpoint";
pub const ENABLED: &str = "power.output.enabled";
pub const AGENT: &str = "agent.test";
pub const OPERATOR: &str = "human.operator";

pub fn clock() -> Arc<dyn Clock> {
    Arc::new(TokioClock::starting_at(Timestamp::from_millis(
        1_800_000_000_000,
    )))
}

pub fn grant(id: &str, capability: &str, ops: &[Operation], effect: Effect) -> Grant {
    Grant {
        id: id.into(),
        principal: AGENT.into(),
        device: "sim.psu-*".into(),
        capability: capability.into(),
        operations: ops.to_vec(),
        effect,
        max_side_effect: None,
        reason: None,
    }
}

/// The agent may read, write and invoke everything on the simulated supply.
pub fn allow_all() -> Policy {
    Policy {
        grants: vec![grant(
            "grant.agent.psu",
            "*",
            &[Operation::Read, Operation::Write, Operation::Invoke],
            Effect::Allow,
        )],
        approvers: vec![OPERATOR.into()],
    }
}

pub async fn setup(policy: Policy) -> (Engine, PsuHandle) {
    let clock = clock();
    let (psu, handle) = VirtualPsu::new(clock.clone(), 42);
    let engine = Engine::in_memory(clock, policy).unwrap();
    engine.register(Arc::new(psu)).await.unwrap();
    (engine, handle)
}

pub fn write(capability: &str, value: Json) -> Json {
    json!({ "target": { "device": PSU, "capability": capability }, "operation": "write", "value": value })
}

pub fn volts(v: f64) -> Json {
    json!({ "value": v, "unit": "V" })
}

pub fn with(mut req: Json, key: &str, value: Json) -> Json {
    req[key] = value;
    req
}

pub fn codes(r: &Receipt) -> Vec<String> {
    r.policy.errors.iter().map(|e| e.code.clone()).collect()
}

pub fn states(r: &Receipt) -> Vec<CommandState> {
    r.transitions.iter().map(|t| t.state).collect()
}

/// Every receipt the engine produces must match the published schema.
pub fn assert_schema_valid(r: &Receipt) {
    let json = serde_json::to_value(r).unwrap();
    let issues = schema::validate(Schema::Receipt, &json);
    assert!(
        issues.is_empty(),
        "receipt does not match receipt.schema.json: {issues:?}\n{json:#}"
    );
}

/// An adapter that serves the simulated supply's descriptor but only counts calls.
/// Used to prove which requests never reach a device.
pub struct TrapAdapter {
    pub reads: Arc<AtomicU64>,
    pub executes: Arc<AtomicU64>,
}

impl TrapAdapter {
    pub fn new() -> (Self, Arc<AtomicU64>, Arc<AtomicU64>) {
        let reads = Arc::new(AtomicU64::new(0));
        let executes = Arc::new(AtomicU64::new(0));
        (
            Self {
                reads: reads.clone(),
                executes: executes.clone(),
            },
            reads,
            executes,
        )
    }
}

#[async_trait]
impl HardwareAdapter for TrapAdapter {
    fn adapter_id(&self) -> &str {
        "openmhs.sim.psu"
    }
    async fn describe(&self) -> Result<Json, AdapterError> {
        Ok(serde_json::from_str(include_str!(
            "../../../../spec/examples/valid/power-supply.json"
        ))
        .unwrap())
    }
    async fn connect(&self) -> Result<(), AdapterError> {
        Ok(())
    }
    async fn health(&self) -> Health {
        Health::Healthy
    }
    async fn read(&self, _capability: &str) -> Result<StateValue, AdapterError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Err(AdapterError::new(
            AdapterErrorKind::Internal,
            "trap: read must not be called",
        ))
    }
    async fn execute(
        &self,
        _c: AdapterCommand,
        _p: Progress,
    ) -> Result<ExecuteReport, AdapterError> {
        self.executes.fetch_add(1, Ordering::SeqCst);
        Err(AdapterError::new(
            AdapterErrorKind::Internal,
            "trap: execute must not be called",
        ))
    }
    async fn shutdown(&self) -> Result<(), AdapterError> {
        Ok(())
    }
}
