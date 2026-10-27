//! Journal quota and critical reserve (ADR-0021): at its limit the journal refuses new
//! physical commands before anything is sent, keeps room for a stop, and never lets a
//! command reach a device without its dispatch recorded.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;

use common::*;
use openmhs_engine::{Engine, EngineError, StorageLimits};
use openmhs_policy::{Caller, Effect, Grant, Policy};
use openmhs_sim::{AxisHandle, PsuHandle, VirtualAxis, VirtualPsu};
use openmhs_types::CommandState;
use openmhs_types::descriptor::Operation;
use serde_json::json;

const MIB: u64 = 1024 * 1024;
const AXIS: &str = "sim.axis-01";

fn policy() -> Policy {
    let grant = |id: &str, device: &str, ops: &[Operation]| Grant {
        id: id.into(),
        principal: AGENT.into(),
        device: device.into(),
        capability: "*".into(),
        operations: ops.to_vec(),
        effect: Effect::Allow,
        max_side_effect: None,
        reason: None,
    };
    Policy {
        grants: vec![
            grant(
                "agent.psu",
                PSU,
                &[Operation::Read, Operation::Write, Operation::Invoke],
            ),
            grant("agent.axis", AXIS, &[Operation::Read, Operation::Invoke]),
        ],
        approvers: vec![OPERATOR.into()],
    }
}

async fn lab() -> (Engine, PsuHandle, AxisHandle) {
    let clock = clock();
    let (psu, psu_handle) = VirtualPsu::new(clock.clone(), 42);
    let (axis, axis_handle) = VirtualAxis::new(clock.clone());
    let engine = Engine::in_memory(clock, policy()).unwrap();
    engine.register(Arc::new(psu)).await.unwrap();
    engine.register(Arc::new(axis)).await.unwrap();
    (engine, psu_handle, axis_handle)
}

fn agent() -> Caller {
    Caller::in_process(AGENT)
}

fn stop() -> serde_json::Value {
    json!({
        "target": { "device": AXIS, "capability": "axis.stop" },
        "operation": "invoke",
        "params": {}
    })
}

fn audit_count(engine: &Engine) -> usize {
    engine.journal().audit_records().unwrap().len()
}

/// Limits that put the journal exactly at its soft limit, with room in the reserve.
fn at_the_soft_limit(engine: &Engine) -> StorageLimits {
    let used = engine.journal().size_bytes().unwrap();
    StorageLimits {
        max_bytes: used + MIB,
        reserve_bytes: MIB,
    }
}

#[tokio::test(start_paused = true)]
async fn within_the_reserve_new_commands_and_reads_are_refused_and_a_stop_is_not() {
    let (engine, psu, axis) = lab().await;
    engine
        .set_storage_limits(at_the_soft_limit(&engine))
        .unwrap();

    // A write is denied before anything reaches the device.
    let r = engine
        .submit(&agent(), &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(r.state, CommandState::Denied, "{r:?}");
    assert_eq!(codes(&r), ["storage.exhausted"]);
    assert_schema_valid(&r);
    assert_eq!(psu.execute_calls(), 0);

    // A preflight says so, and does not spend the reserve on a record of itself.
    let before = audit_count(&engine);
    let checked = engine
        .preflight(&agent(), &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(checked.errors[0].code, "storage.exhausted");
    assert_eq!(audit_count(&engine), before);

    // So does an audited read, without touching the device or writing a record.
    let reads = psu.read_calls();
    let read = engine
        .read(&agent(), PSU, "power.output.voltage.measured")
        .await;
    assert!(
        matches!(&read, Err(EngineError::Denied(e)) if e.code == "storage.exhausted"),
        "{read:?}"
    );
    assert_eq!(psu.read_calls(), reads);

    // A stop is accepted, and its records fit in the reserve.
    let stopped = engine.submit(&agent(), &stop()).await.unwrap();
    assert_ne!(stopped.state, CommandState::Denied, "{stopped:?}");
    assert!(codes(&stopped).is_empty(), "{stopped:?}");
    assert_eq!(axis.execute_calls(), 1);
    assert!(engine.verify_audit().unwrap().is_valid());

    // Raising the limit restores service.
    let used = engine.journal().size_bytes().unwrap();
    engine
        .set_storage_limits(StorageLimits {
            max_bytes: used + 64 * MIB,
            reserve_bytes: MIB,
        })
        .unwrap();
    let ok = engine
        .submit(&agent(), &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(ok.state, CommandState::Verified, "{ok:?}");
}

#[tokio::test(start_paused = true)]
async fn at_the_hard_limit_nothing_reaches_a_device_without_its_dispatch_recorded() {
    let (engine, psu, _axis) = lab().await;
    let used = engine.journal().size_bytes().unwrap();
    // No reserve, so the soft limit only trips when the journal is exactly full; a command
    // that starts with less room than it needs runs out of journal under it.
    engine
        .set_storage_limits(StorageLimits {
            max_bytes: used + 64 * 1024,
            reserve_bytes: 0,
        })
        .unwrap();

    let mut ran_out_mid_command = false;
    let mut refused_whole = false;
    for i in 0..200 {
        let volts_now = if i % 2 == 0 { 1.0 } else { 2.0 };
        match engine
            .submit(&agent(), &write(SETPOINT, volts(volts_now)))
            .await
        {
            Err(_) => ran_out_mid_command = true,
            Ok(r) if codes(&r) == ["storage.exhausted"] => refused_whole = true,
            Ok(_) => {}
        }
        // Clear the supply's slew window so each request is judged on storage alone.
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        if ran_out_mid_command && refused_whole {
            break;
        }
    }
    assert!(
        ran_out_mid_command,
        "the journal never ran out under a command: the test proves nothing"
    );

    // The property: everything that reached the device has its dispatch on record.
    let sent = psu.execute_calls();
    let recorded = engine
        .commands(None, &[], 500)
        .unwrap()
        .iter()
        .filter(|r| {
            r.transitions
                .iter()
                .any(|t| t.state == CommandState::Dispatched)
        })
        .count();
    assert!(
        usize::try_from(sent).unwrap() <= recorded,
        "{sent} commands reached the device but only {recorded} dispatches are recorded"
    );
}

#[tokio::test(start_paused = true)]
async fn limits_that_cannot_work_are_refused() {
    let (engine, _psu, _axis) = lab().await;
    let used = engine.journal().size_bytes().unwrap();

    let reserve_too_big = engine.set_storage_limits(StorageLimits {
        max_bytes: used + MIB,
        reserve_bytes: used + MIB,
    });
    assert!(matches!(
        reserve_too_big,
        Err(EngineError::InvalidRequest(_))
    ));

    let below_current = engine.set_storage_limits(StorageLimits {
        max_bytes: used - 1,
        reserve_bytes: 0,
    });
    assert!(
        matches!(&below_current, Err(EngineError::InvalidRequest(e)) if e[0].code == "storage.limits"),
        "{below_current:?}"
    );

    // Without limits nothing changes.
    let r = engine
        .submit(&agent(), &write(SETPOINT, volts(1.0)))
        .await
        .unwrap();
    assert_eq!(r.state, CommandState::Verified, "{r:?}");
}
