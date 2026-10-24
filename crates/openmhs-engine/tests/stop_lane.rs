//! M9: the stop lane (ADR-0018, amending ADR-0010). An action declared `stop` does not
//! wait for the command running on the device and is not blocked by someone else's lease,
//! but it is still authorized and validated like any other command.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use openmhs_engine::{Engine, LeaseRequest};
use openmhs_policy::{Caller, Effect, Grant, Policy};
use openmhs_sim::{AxisHandle, VirtualAxis};
use openmhs_types::CommandState;
use openmhs_types::descriptor::Operation;
use serde_json::{Value, json};

const A: &str = "agent.a";
const B: &str = "agent.b";
const AXIS: &str = "sim.axis-01";

fn grant_all(principal: &str, ops: &[Operation]) -> Grant {
    Grant {
        id: format!("{principal}.axis"),
        principal: principal.into(),
        device: AXIS.into(),
        capability: "*".into(),
        operations: ops.to_vec(),
        effect: Effect::Allow,
        max_side_effect: None,
        reason: None,
    }
}

fn policy() -> Policy {
    let operate = [Operation::Read, Operation::Invoke];
    Policy {
        grants: vec![
            grant_all(A, &operate),
            grant_all(B, &operate),
            grant_all("agent.reader", &[Operation::Read]),
        ],
        approvers: vec![OPERATOR.into()],
    }
}

async fn stage() -> (Arc<Engine>, AxisHandle) {
    let clock = clock();
    let (axis, handle) = VirtualAxis::new(clock.clone());
    let engine = Engine::in_memory(clock, policy()).unwrap();
    engine.register(Arc::new(axis)).await.unwrap();
    (Arc::new(engine), handle)
}

fn invoke(capability: &str, params: Value) -> Value {
    json!({
        "target": { "device": AXIS, "capability": capability },
        "operation": "invoke",
        "params": params
    })
}

fn move_to(mm: f64) -> Value {
    invoke(
        "axis.move_absolute",
        json!({
            "position": { "value": mm, "unit": "mm" },
            "speed": { "value": 20, "unit": "mm/s" }
        }),
    )
}

fn stop() -> Value {
    invoke("axis.stop", json!({}))
}

async fn homed(engine: &Engine) {
    let r = engine
        .submit(&Caller::in_process(A), &invoke("axis.home", json!({})))
        .await
        .unwrap();
    assert_eq!(r.state, CommandState::Verified, "{r:?}");
}

/// The case ADR-0010 recorded as a limitation: a stop sent during a long move used to
/// wait for the move to finish. Now it halts the carriage mid-travel.
#[tokio::test(start_paused = true)]
async fn a_stop_interrupts_a_running_move() {
    let (engine, axis) = stage().await;
    homed(&engine).await;

    let mover = engine.clone();
    let running = tokio::spawn(async move {
        mover
            .submit(&Caller::in_process(A), &move_to(200.0))
            .await
            .unwrap()
    });
    // 200 mm at 20 mm/s is a ten-second move; stop it two seconds in.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(axis.truth().motion.is_some(), "the move is under way");

    let stopped = engine
        .submit(&Caller::in_process(A), &stop())
        .await
        .unwrap();
    assert_eq!(stopped.state, CommandState::Verified, "{stopped:?}");
    let truth = axis.truth();
    assert!(truth.motion.is_none());
    assert!(
        truth.position_mm > 10.0 && truth.position_mm < 100.0,
        "halted mid-travel at {} mm",
        truth.position_mm
    );

    // The interrupted move is reported as what it is: it did not get there.
    let moved = running.await.unwrap();
    assert_eq!(moved.state, CommandState::Failed, "{moved:?}");
    let detail = format!("{moved:?}");
    assert!(detail.contains("stopped at"), "{detail}");
}

/// Stopping must not depend on who holds the device: another holder's lease refuses
/// everything else, but not a stop.
#[tokio::test(start_paused = true)]
async fn a_stop_passes_another_holders_lease() {
    let (engine, _axis) = stage().await;
    homed(&engine).await;
    engine
        .acquire_lease(
            &Caller::in_process(A),
            LeaseRequest {
                devices: vec![AXIS.into()],
                ttl_ms: 60_000,
                ..LeaseRequest::default()
            },
        )
        .unwrap();
    let b = Caller::in_process(B);
    let refused = engine.submit(&b, &move_to(50.0)).await.unwrap();
    assert_eq!(codes(&refused), ["lease.held"]);
    let stopped = engine.submit(&b, &stop()).await.unwrap();
    assert_eq!(stopped.state, CommandState::Verified, "{stopped:?}");
}

/// The lane skips waiting, not checking: a caller with no right to invoke the stop is
/// still refused, and so is a malformed stop.
#[tokio::test(start_paused = true)]
async fn a_stop_is_still_authorized_and_validated() {
    let (engine, axis) = stage().await;
    let reader = Caller::in_process("agent.reader");
    let denied = engine.submit(&reader, &stop()).await.unwrap();
    assert_eq!(denied.state, CommandState::Denied);
    assert!(!codes(&denied).is_empty());

    let bad = invoke(
        "axis.stop",
        json!({ "speed": { "value": 1, "unit": "mm/s" } }),
    );
    let rejected = engine.submit(&Caller::in_process(A), &bad).await.unwrap();
    assert_eq!(rejected.state, CommandState::Denied, "{rejected:?}");
    assert_eq!(axis.execute_calls(), 0, "nothing reached the device");
}

/// Ordinary commands still queue behind the running one (ADR-0010 is unchanged for them).
#[tokio::test(start_paused = true)]
async fn other_commands_still_wait_for_the_device() {
    let (engine, axis) = stage().await;
    homed(&engine).await;
    let mover = engine.clone();
    let running = tokio::spawn(async move {
        mover
            .submit(&Caller::in_process(A), &move_to(40.0))
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    // A second move waits for the device instead of being refused as "already moving".
    let second = engine
        .submit(&Caller::in_process(B), &move_to(10.0))
        .await
        .unwrap();
    let first = running.await.unwrap();
    assert_eq!(first.state, CommandState::Verified, "{first:?}");
    assert_eq!(second.state, CommandState::Verified, "{second:?}");
    assert!((axis.truth().position_mm - 10.0).abs() < 1e-6);
}
